//! End-to-end tests for the rift event log: the HTTP read path, long-polling, and the hooks that write events.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server events_api -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::Utc;
use futures_util::SinkExt;
use mothership_common::{auth::Claims, protocol::SyncMessage, RiftId, User, UserRole};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat::ChatSettings, config::ServerConfig, create_api_router,
    database::Database, events, oauth::OAuthService, storage::StorageEngine, sync::SyncState,
    AppState,
};
use axum::routing::get;

const TEST_SECRET: &str = "events-api-test-secret";

/// The router these tests call. It is the API router, not the combined one: `create_combined_router`
/// panics on startup because it registers `/auth/finalize` and also merges the web UI routes, which register it again.
fn create_combined_router(state: AppState) -> Router {
    create_api_router(state)
}

struct Fixture {
    state: AppState,
    owner: User,
    member: User,
    outsider: User,
    project_id: Uuid,
    /// Public rift both owner and member collaborate on
    main: RiftId,
    /// Private rift only the owner collaborates on
    secret: RiftId,
}

/// A project with an owner, a member who collaborates on `main`, and an outsider in no rift.
async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let make_user = |label: &str| {
        let name = format!("{}-{}", label, suffix);
        (name.clone(), format!("{}@events-test.local", name))
    };
    let (name, email) = make_user("owner");
    let owner = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("member");
    let member = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("outsider");
    let outsider = db.create_user(name, email, UserRole::User).await.unwrap();

    let project = db
        .create_project(
            format!("events-test-{}", suffix),
            "events API test".to_string(),
            vec![owner.id, member.id],
        )
        .await
        .unwrap();
    let main = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();
    let secret = db
        .create_rift_with_privacy(project.id, owner.id, Some("secret".to_string()), true)
        .await
        .unwrap();

    // create_rift only adds the creator as a collaborator, so add the member to `main` directly
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("INSERT INTO rift_collaborators (rift_id, user_id) VALUES ($1, $2)")
        .bind(main.id)
        .bind(member.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let storage_root = std::env::temp_dir().join(format!("mothership-events-test-{}", suffix));
    let storage = Arc::new(StorageEngine::new(storage_root).await.unwrap());

    let state = AppState {
        db: db.clone(),
        auth: AuthService::new(TEST_SECRET.to_string()),
        oauth: OAuthService::new().unwrap(),
        sync: SyncState::new(db, storage, ChatSettings {
            enabled: true,
            max_message_length: 1000,
            store_history: true,
            max_history: 1000,
        }),
        config: ServerConfig::default(),
        whitelist: None,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        temp_tokens: Arc::new(RwLock::new(HashMap::new())),
        ghostid: None,
    };

    Fixture {
        state,
        owner,
        member,
        outsider,
        project_id: project.id,
        main: main.id,
        secret: secret.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "events-api-test".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::hours(1)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    state.auth.encode_token(&claims).unwrap()
}

async fn send(
    app: &Router,
    method: Method,
    uri: String,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
    }
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn events_uri(rift: RiftId, query: &str) -> String {
    format!("/api/rifts/{}/events{}", rift, query)
}

fn chat_uri(rift: RiftId) -> String {
    format!("/api/rifts/{}/chat", rift)
}

async fn post_chat(app: &Router, token: &str, rift: RiftId, content: &str) {
    let (status, _) = send(app, Method::POST, chat_uri(rift), Some(token), Some(json!({ "content": content }))).await;
    assert_eq!(status, StatusCode::OK, "chat post should succeed");
}

async fn read_events(app: &Router, token: &str, rift: RiftId, query: &str) -> (StatusCode, Vec<Value>) {
    let (status, body) = send(app, Method::GET, events_uri(rift, query), Some(token), None).await;
    (status, body["data"].as_array().cloned().unwrap_or_default())
}

/// Events are written by spawned tasks, so poll until `count` events of `kind` appear
async fn wait_for(app: &Router, token: &str, rift: RiftId, kind: &str, count: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, events) = read_events(app, token, rift, "").await;
            let matching: Vec<Value> = events
                .into_iter()
                .filter(|event| event["event_type"] == kind)
                .collect();
            if matching.len() >= count {
                return matching;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {count} '{kind}' events"))
}

#[tokio::test]
#[ignore]
async fn rejects_requests_without_a_valid_token() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());

    let (status, _) = send(&app, Method::GET, events_uri(fx.main, ""), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&app, Method::GET, events_uri(fx.main, ""), Some("not-a-real-token"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn only_rift_collaborators_can_read_the_log() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let owner_token = token_for(&fx.state, &fx.owner);
    let member_token = token_for(&fx.state, &fx.member);
    let outsider_token = token_for(&fx.state, &fx.outsider);

    // A project member who is not on the private rift, and a stranger, are both refused
    let (status, _) = read_events(&app, &member_token, fx.secret, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = read_events(&app, &outsider_token, fx.main, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = read_events(&app, &owner_token, fx.secret, "").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn chat_and_access_changes_are_logged_with_their_actor() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let owner_token = token_for(&fx.state, &fx.owner);
    let member_token = token_for(&fx.state, &fx.member);

    post_chat(&app, &member_token, fx.main, "hello timeline").await;
    let chat = wait_for(&app, &member_token, fx.main, events::CHAT, 1).await;
    assert_eq!(chat[0]["actor_username"], fx.member.username);
    assert_eq!(chat[0]["payload"]["content"], "hello timeline");

    // The owner grants access to the private rift, then revokes it. Wait between the two so
    // the log order is deterministic.
    let membership = |remove: bool| {
        json!({
            "project_id": fx.project_id,
            "rift_name": "secret",
            "username": fx.member.username,
            "remove": remove,
        })
    };
    let (status, _) = send(&app, Method::POST, "/api/rifts/members".into(), Some(&owner_token), Some(membership(false))).await;
    assert_eq!(status, StatusCode::OK);
    wait_for(&app, &owner_token, fx.secret, events::ACCESS_CHANGE, 1).await;
    let (status, _) = send(&app, Method::POST, "/api/rifts/members".into(), Some(&owner_token), Some(membership(true))).await;
    assert_eq!(status, StatusCode::OK);

    let changes = wait_for(&app, &owner_token, fx.secret, events::ACCESS_CHANGE, 2).await;
    assert_eq!(changes[0]["payload"]["action"], "add");
    assert_eq!(changes[1]["payload"]["action"], "remove");
    assert_eq!(changes[0]["payload"]["target_username"], fx.member.username);
    assert_eq!(changes[0]["actor_username"], fx.owner.username);
}

#[tokio::test]
#[ignore]
async fn cursor_and_paging_walk_the_log_in_order() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);

    // Post one at a time so the log order matches the post order
    for (i, content) in ["one", "two", "three"].iter().enumerate() {
        post_chat(&app, &member_token, fx.main, content).await;
        wait_for(&app, &member_token, fx.main, events::CHAT, i + 1).await;
    }
    let contents = |events: &[Value]| -> Vec<String> {
        events.iter().map(|e| e["payload"]["content"].as_str().unwrap().to_string()).collect()
    };

    let (_, all) = read_events(&app, &member_token, fx.main, "?after=0").await;
    assert_eq!(contents(&all), vec!["one", "two", "three"]);

    let (_, after_first) = read_events(&app, &member_token, fx.main, &format!("?after={}", all[0]["id"])).await;
    assert_eq!(contents(&after_first), vec!["two", "three"]);

    let (_, newest) = read_events(&app, &member_token, fx.main, "?latest=true&limit=2").await;
    assert_eq!(contents(&newest), vec!["two", "three"]);

    let (_, older) = read_events(&app, &member_token, fx.main, &format!("?before={}&limit=10", newest[0]["id"])).await;
    assert_eq!(contents(&older), vec!["one"]);
}

#[tokio::test]
#[ignore]
async fn long_poll_returns_as_soon_as_an_event_is_written() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);

    post_chat(&app, &member_token, fx.main, "before").await;
    let cursor = wait_for(&app, &member_token, fx.main, events::CHAT, 1).await[0]["id"].clone();

    let start = Instant::now();
    let writer = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        post_chat(&app, &member_token, fx.main, "wake up").await;
    };
    let poll = send(
        &app,
        Method::GET,
        events_uri(fx.main, &format!("?after={}&wait=10", cursor)),
        Some(&member_token),
        None,
    );
    let ((status, body), ()) = tokio::join!(poll, writer);

    assert_eq!(status, StatusCode::OK);
    assert!(start.elapsed() < Duration::from_secs(5), "the poll should return once the event lands");
    let events = body["data"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"]["content"], "wake up");
}

#[tokio::test]
#[ignore]
async fn long_poll_returns_empty_after_the_wait_when_nothing_happens() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);

    post_chat(&app, &member_token, fx.main, "quiet").await;
    let cursor = wait_for(&app, &member_token, fx.main, events::CHAT, 1).await[0]["id"].clone();

    let start = Instant::now();
    let (status, events) = read_events(&app, &member_token, fx.main, &format!("?after={}&wait=1", cursor)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(events.is_empty());
    assert!(start.elapsed() >= Duration::from_secs(1), "the poll should hold for the full wait");
}

#[tokio::test]
#[ignore]
async fn checkpoint_restore_and_wormhole_actions_are_logged() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let owner_token = token_for(&fx.state, &fx.owner);

    // Checkpoints and restores are written to the owner's rift
    let owner_rift = fx
        .state
        .db
        .get_user_rift(fx.project_id, fx.owner.id)
        .await
        .unwrap()
        .expect("owner should have a rift")
        .id;

    let (status, body) = send(
        &app,
        Method::POST,
        format!("/projects/{}/checkpoints", fx.project_id),
        Some(&owner_token),
        Some(json!({ "message": "before refactor", "timestamp": Utc::now() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let checkpoint_id = body["data"]["checkpoint_id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        Method::POST,
        format!("/projects/{}/checkpoints/{}/restore", fx.project_id, checkpoint_id),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let checkpoints = wait_for(&app, &owner_token, owner_rift, events::CHECKPOINT, 1).await;
    assert_eq!(checkpoints[0]["payload"]["message"], "before refactor");
    assert_eq!(checkpoints[0]["payload"]["checkpoint_id"], checkpoint_id.as_str());
    let restores = wait_for(&app, &owner_token, owner_rift, events::RESTORE, 1).await;
    assert_eq!(restores[0]["payload"]["checkpoint_id"], checkpoint_id.as_str());

    // A wormhole over both rifts is logged to each of them
    let wormhole_name = format!("events-wh-{}", Uuid::new_v4().simple());
    let (status, body) = send(
        &app,
        Method::POST,
        "/api/wormholes".into(),
        Some(&owner_token),
        Some(json!({
            "project_id": fx.project_id,
            "name": wormhole_name,
            "rift_ids": [fx.main, fx.secret],
            "is_private": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wormhole create failed: {body}");
    let wormhole_id = body["data"]["id"].as_str().unwrap().to_string();

    for rift in [fx.main, fx.secret] {
        let created = wait_for(&app, &owner_token, rift, events::WORMHOLE, 1).await;
        assert_eq!(created[0]["payload"]["action"], "create");
        assert_eq!(created[0]["payload"]["wormhole_id"], wormhole_id.as_str());
    }

    let (status, _) = send(
        &app,
        Method::POST,
        format!("/api/wormholes/{}/dissolve", wormhole_id),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for rift in [fx.main, fx.secret] {
        let actions = wait_for(&app, &owner_token, rift, events::WORMHOLE, 2).await;
        assert_eq!(actions[1]["payload"]["action"], "dissolve");
    }
}

#[tokio::test]
#[ignore]
async fn joins_leaves_and_file_saves_are_logged_over_the_socket() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });

    let url = format!("ws://{}/ws/{}?token={}", addr, fx.main, member_token);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("member connects");

    let send_sync = |message: SyncMessage| Message::Text(serde_json::to_string(&message).unwrap());
    ws.send(send_sync(SyncMessage::JoinRift { rift_id: fx.main, last_checkpoint: None }))
        .await
        .unwrap();
    // Let the join register before the file change arrives
    tokio::time::sleep(Duration::from_millis(300)).await;

    ws.send(send_sync(SyncMessage::FileChanged {
        rift_id: fx.main,
        path: PathBuf::from("notes.txt"),
        content: "hello".to_string(),
        timestamp: Utc::now(),
        base_hash: None,
    }))
    .await
    .unwrap();

    let joins = wait_for(&app, &member_token, fx.main, events::JOIN, 1).await;
    assert_eq!(joins[0]["actor_username"], fx.member.username);
    let saves = wait_for(&app, &member_token, fx.main, events::FILE_SAVED, 1).await;
    assert_eq!(saves[0]["payload"]["path"], "notes.txt");

    ws.close(None).await.unwrap();
    let leaves = wait_for(&app, &member_token, fx.main, events::LEAVE, 1).await;
    assert_eq!(leaves[0]["actor_username"], fx.member.username);
}

#[tokio::test]
#[ignore]
async fn chat_anchors_are_attached_to_the_event() {
    let fx = fixture().await;
    let app = create_combined_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);

    post_chat(&app, &member_token, fx.main, "check scripts/launcher.py:136 and scripts/").await;
    let chat = wait_for(&app, &member_token, fx.main, events::CHAT, 1).await;

    let anchors = chat[0]["payload"]["anchors"].as_array().unwrap();
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0]["path"], "scripts/launcher.py");
    assert_eq!(anchors[0]["line"], 136);
    assert_eq!(anchors[1]["path"], "scripts");
    assert_eq!(anchors[1]["folder"], true);
}
