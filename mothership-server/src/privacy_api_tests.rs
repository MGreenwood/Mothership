//! End-to-end tests for private rifts and wormholes, rift access requests, and rift member management.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server privacy -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use mothership_common::{auth::Claims, RiftId, User, UserRole};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat::ChatSettings, config::ServerConfig, database::Database,
    oauth::OAuthService, storage::StorageEngine, sync::SyncState, AppState,
};

const TEST_SECRET: &str = "privacy-api-test-secret";

struct Fixture {
    state: AppState,
    owner: User,
    member: User,
    outsider: User,
    project_id: Uuid,
    private_rift: RiftId,
    public_rift: RiftId,
}

/// A project with an owner, a project member who collaborates on the public rift, and an outsider.
/// The owner creates a public `main` rift and a private `secret` rift.
async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let make_user = |label: &str| {
        let name = format!("{}-{}", label, suffix);
        (name.clone(), format!("{}@privacy-test.local", name))
    };
    let (name, email) = make_user("owner");
    let owner = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("member");
    let member = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("outsider");
    let outsider = db.create_user(name, email, UserRole::User).await.unwrap();

    let project = db
        .create_project(
            format!("privacy-test-{}", suffix),
            "privacy API test".to_string(),
            vec![owner.id, member.id],
        )
        .await
        .unwrap();

    let public_rift = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();
    let private_rift = db
        .create_rift_with_privacy(project.id, owner.id, Some("secret".to_string()), true)
        .await
        .unwrap();

    // create_rift only adds the creator as a collaborator, so add the member to `main` directly
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("INSERT INTO rift_collaborators (rift_id, user_id) VALUES ($1, $2)")
        .bind(public_rift.id)
        .bind(member.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let storage_root = std::env::temp_dir().join(format!("mothership-privacy-test-{}", suffix));
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
    };

    Fixture {
        state,
        owner,
        member,
        outsider,
        project_id: project.id,
        private_rift: private_rift.id,
        public_rift: public_rift.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = chrono::Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "privacy-api-test".to_string(),
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
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token));
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

async fn switch(app: &Router, token: &str, project_id: Uuid, rift_name: &str) -> StatusCode {
    send(
        app,
        Method::POST,
        "/api/rifts/switch".to_string(),
        token,
        Some(json!({ "rift_name": rift_name, "project_id": project_id })),
    )
    .await
    .0
}

fn rift_names(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .expect("rift list data")
        .iter()
        .map(|rift| rift["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore]
async fn private_rift_hidden_from_non_collaborators() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);
    let owner_token = token_for(&fx.state, &fx.owner);

    let (status, body) = send(&app, Method::GET, "/api/rifts".to_string(), &member_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let names = rift_names(&body);
    assert!(names.contains(&"main".to_string()), "member sees public rift: {names:?}");
    assert!(!names.contains(&"secret".to_string()), "member must not see private rift: {names:?}");

    let (status, body) = send(&app, Method::GET, "/api/rifts".to_string(), &owner_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let names = rift_names(&body);
    assert!(names.contains(&"secret".to_string()), "owner sees private rift: {names:?}");
}

#[tokio::test]
#[ignore]
async fn switch_enforces_rift_membership() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);
    let owner_token = token_for(&fx.state, &fx.owner);
    let outsider_token = token_for(&fx.state, &fx.outsider);

    assert_eq!(switch(&app, &member_token, fx.project_id, "main").await, StatusCode::OK);
    // Project member who is not a collaborator on the rift: forbidden, prompts a request
    assert_eq!(switch(&app, &member_token, fx.project_id, "secret").await, StatusCode::FORBIDDEN);
    assert_eq!(switch(&app, &owner_token, fx.project_id, "secret").await, StatusCode::OK);
    // Outsider has no project access, so the rift is simply not found
    assert_eq!(switch(&app, &outsider_token, fx.project_id, "main").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn access_request_flow_and_owner_listing() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);
    let owner_token = token_for(&fx.state, &fx.owner);
    let outsider_token = token_for(&fx.state, &fx.outsider);
    let request_uri = "/api/rifts/access-requests".to_string();
    let request_body = json!({ "rift_name": "secret", "project_id": fx.project_id });

    let (status, _) = send(&app, Method::POST, request_uri.clone(), &member_token, Some(request_body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    // Repeated requests are a no-op
    let (status, _) = send(&app, Method::POST, request_uri.clone(), &member_token, Some(request_body.clone())).await;
    assert_eq!(status, StatusCode::OK);

    // Outsiders cannot request access to a project they are not in
    let (status, _) = send(&app, Method::POST, request_uri.clone(), &outsider_token, Some(request_body.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let list_uri = format!("{}?rift_name=secret&project_id={}", request_uri, fx.project_id);
    let (status, body) = send(&app, Method::GET, list_uri.clone(), &owner_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let requests = body["data"].as_array().expect("request list");
    assert_eq!(requests.len(), 1, "one pending request: {body}");
    assert_eq!(requests[0]["username"], fx.member.username);

    // Only the owner can see the request list
    let (status, _) = send(&app, Method::GET, list_uri, &member_token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore]
async fn owner_manages_members_and_approval_clears_request() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);
    let owner_token = token_for(&fx.state, &fx.owner);
    let outsider_token = token_for(&fx.state, &fx.outsider);
    let members_uri = "/api/rifts/members".to_string();
    let add = json!({ "rift_name": "secret", "project_id": fx.project_id, "username": fx.member.username, "remove": false });
    let remove = json!({ "rift_name": "secret", "project_id": fx.project_id, "username": fx.member.username, "remove": true });

    let (status, _) = send(
        &app,
        Method::POST,
        "/api/rifts/access-requests".to_string(),
        &member_token,
        Some(json!({ "rift_name": "secret", "project_id": fx.project_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Non-owners cannot manage members
    let (status, _) = send(&app, Method::POST, members_uri.clone(), &member_token, Some(add.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, Method::POST, members_uri.clone(), &outsider_token, Some(add.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Owner approves: the member can switch, and the pending request is cleared
    let (status, _) = send(&app, Method::POST, members_uri.clone(), &owner_token, Some(add)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(switch(&app, &member_token, fx.project_id, "secret").await, StatusCode::OK);

    let list_uri = format!("/api/rifts/access-requests?rift_name=secret&project_id={}", fx.project_id);
    let (_, body) = send(&app, Method::GET, list_uri, &owner_token, None).await;
    assert_eq!(body["data"].as_array().map(Vec::len), Some(0), "request cleared on add: {body}");

    // Owner removes: access is revoked again
    let (status, _) = send(&app, Method::POST, members_uri, &owner_token, Some(remove)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(switch(&app, &member_token, fx.project_id, "secret").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore]
async fn private_wormhole_visible_only_to_creator() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());
    let member_token = token_for(&fx.state, &fx.member);
    let owner_token = token_for(&fx.state, &fx.owner);
    let list_uri = format!("/api/wormholes?project_id={}", fx.project_id);

    // The member collaborates on `main`, which is one of the wormhole's rifts
    let create = |name: &str, is_private: bool| {
        json!({
            "project_id": fx.project_id,
            "name": name,
            "rift_ids": [fx.public_rift, fx.private_rift],
            "is_private": is_private,
        })
    };
    let (status, body) = send(&app, Method::POST, "/api/wormholes".to_string(), &owner_token, Some(create("hidden-wh", true))).await;
    assert_eq!(status, StatusCode::OK, "create private wormhole: {body}");
    let private_id = body["data"]["id"].as_str().expect("wormhole id").to_string();

    let (status, body) = send(&app, Method::POST, "/api/wormholes".to_string(), &owner_token, Some(create("open-wh", false))).await;
    assert_eq!(status, StatusCode::OK, "create public wormhole: {body}");
    let public_id = body["data"]["id"].as_str().expect("wormhole id").to_string();

    // Owner (creator) sees both
    let (_, body) = send(&app, Method::GET, list_uri.clone(), &owner_token, None).await;
    let ids: Vec<String> = body["data"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&private_id) && ids.contains(&public_id), "owner sees both: {ids:?}");

    // Rift collaborator who did not create it sees only the public wormhole
    let (_, body) = send(&app, Method::GET, list_uri, &member_token, None).await;
    let ids: Vec<String> = body["data"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap().to_string()).collect();
    assert!(!ids.contains(&private_id), "member must not list private wormhole: {ids:?}");
    assert!(ids.contains(&public_id), "member lists public wormhole: {ids:?}");

    let (status, _) = send(&app, Method::GET, format!("/api/wormholes/{private_id}"), &member_token, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "member cannot fetch private wormhole");
    let (status, _) = send(&app, Method::GET, format!("/api/wormholes/{private_id}"), &owner_token, None).await;
    assert_eq!(status, StatusCode::OK, "creator can fetch private wormhole");
}

#[tokio::test]
#[ignore]
async fn token_for_missing_user_is_rejected_not_recreated() {
    let fx = fixture().await;
    let app = crate::create_api_router(fx.state.clone());

    // Validly signed token whose subject was never stored (e.g. after a DB wipe)
    let ghost = User {
        id: Uuid::new_v4(),
        username: format!("ghost-{}", Uuid::new_v4().simple()),
        email: "ghost@privacy-test.local".to_string(),
        role: UserRole::User,
        created_at: chrono::Utc::now(),
    };
    let ghost_token = token_for(&fx.state, &ghost);

    let (status, _) = send(&app, Method::GET, "/auth/check".to_string(), &ghost_token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(fx.state.db.get_user(ghost.id).await.unwrap().is_none(), "token must not recreate the user");
}

#[tokio::test]
#[ignore]
async fn legacy_ownerless_rifts_backfill_earliest_collaborator() {
    let fx = fixture().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();

    // Legacy rift: no owner, three collaborators, the member joined first
    let legacy = fx.state.db
        .create_rift(fx.project_id, fx.owner.id, Some("legacy".to_string()))
        .await
        .unwrap();
    sqlx::query("UPDATE rifts SET created_by = NULL WHERE id = $1").bind(legacy.id).execute(&pool).await.unwrap();
    for (user, joined) in [
        (fx.member.id, "NOW() - INTERVAL '2 days'"),
        (fx.outsider.id, "NOW() - INTERVAL '1 day'"),
        (fx.owner.id, "NOW()"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO rift_collaborators (rift_id, user_id, joined_at) VALUES ($1, $2, {joined}) \
             ON CONFLICT (rift_id, user_id) DO UPDATE SET joined_at = EXCLUDED.joined_at"
        ))
        .bind(legacy.id)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Legacy rift with no collaborators at all: stays ownerless
    let orphan = fx.state.db
        .create_rift(fx.project_id, fx.owner.id, Some("orphan".to_string()))
        .await
        .unwrap();
    sqlx::query("UPDATE rifts SET created_by = NULL WHERE id = $1").bind(orphan.id).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rift_collaborators WHERE rift_id = $1").bind(orphan.id).execute(&pool).await.unwrap();

    fx.state.db.ensure_schema().await.expect("schema rerun");
    // Running the backfill again must not change an already-assigned owner
    fx.state.db.ensure_schema().await.expect("schema rerun");

    let legacy_owner: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM rifts WHERE id = $1")
        .bind(legacy.id).fetch_one(&pool).await.unwrap();
    assert_eq!(legacy_owner, Some(fx.member.id), "earliest collaborator becomes owner");

    let orphan_owner: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM rifts WHERE id = $1")
        .bind(orphan.id).fetch_one(&pool).await.unwrap();
    assert_eq!(orphan_owner, None, "rift with no collaborators stays ownerless");
    pool.close().await;
}

