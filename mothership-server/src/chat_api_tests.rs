//! End-to-end chat tests through the real router and WebSocket handler.
//!
//! These need Postgres with the root migrations applied (they create the `rifts` table):
//! set TEST_DATABASE_URL and run `cargo test -p mothership-server chat -- --ignored`
//! with SQLX_OFFLINE=true.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use mothership_common::{
    auth::Claims,
    protocol::{ChatMessage, SyncMessage},
    RiftId, User, UserRole,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat, chat::ChatSettings, config::ServerConfig, database::Database,
    oauth::OAuthService, storage::StorageEngine, sync::SyncState, AppState,
};

const TEST_SECRET: &str = "chat-api-test-secret";

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Fixture {
    state: AppState,
    owner: User,
    member: User,
    outsider: User,
    rift_id: RiftId,
}

fn default_settings() -> ChatSettings {
    ChatSettings {
        enabled: true,
        max_message_length: 1000,
        store_history: true,
        max_history: 1000,
    }
}

/// Creates a project with an owner and a member, plus a rift both are collaborators on.
/// The outsider belongs to neither.
async fn fixture(chat: ChatSettings) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let make_user = |label: &str| {
        let name = format!("{}-{}", label, suffix);
        (name.clone(), format!("{}@chat-test.local", name))
    };
    let (name, email) = make_user("owner");
    let owner = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("member");
    let member = db.create_user(name, email, UserRole::User).await.unwrap();
    let (name, email) = make_user("outsider");
    let outsider = db.create_user(name, email, UserRole::User).await.unwrap();

    let project = db
        .create_project(
            format!("chat-test-{}", suffix),
            "chat API test".to_string(),
            vec![owner.id, member.id],
        )
        .await
        .unwrap();
    let rift = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();

    // The WebSocket upgrade checks rift collaborators, and create_rift only adds the creator
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("INSERT INTO rift_collaborators (rift_id, user_id) VALUES ($1, $2)")
        .bind(rift.id)
        .bind(member.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let storage_root = std::env::temp_dir().join(format!("mothership-chat-test-{}", suffix));
    let storage = Arc::new(StorageEngine::new(storage_root).await.unwrap());

    let state = AppState {
        db: db.clone(),
        auth: AuthService::new(TEST_SECRET.to_string()),
        oauth: OAuthService::new().unwrap(),
        sync: SyncState::new(db, storage, chat),
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
        rift_id: rift.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = chrono::Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "chat-api-test".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::hours(1)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    state.auth.encode_token(&claims).unwrap()
}

/// Same routes as the production router for chat and the rift socket
fn app(state: &AppState) -> Router {
    Router::new()
        .route(
            "/api/rifts/:id/chat",
            get(chat::get_chat_history).post(chat::post_chat),
        )
        .route("/ws/:rift_id", get(crate::websocket_handler))
        .with_state(state.clone())
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
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn chat_uri(rift_id: RiftId, query: &str) -> String {
    format!("/api/rifts/{}/chat{}", rift_id, query)
}

async fn post_as(app: &Router, token: &str, rift_id: RiftId, content: &str) -> (StatusCode, Value) {
    send(
        app,
        Method::POST,
        chat_uri(rift_id, ""),
        Some(token),
        Some(json!({ "content": content })),
    )
    .await
}

#[tokio::test]
#[ignore]
async fn rejects_requests_without_a_valid_token() {
    let fx = fixture(default_settings()).await;
    let app = app(&fx.state);

    let (status, _) = send(&app, Method::GET, chat_uri(fx.rift_id, ""), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &app,
        Method::POST,
        chat_uri(fx.rift_id, ""),
        Some("not-a-real-token"),
        Some(json!({ "content": "hi" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn rejects_users_outside_the_project() {
    let fx = fixture(default_settings()).await;
    let app = app(&fx.state);
    let outsider_token = token_for(&fx.state, &fx.outsider);

    let (status, _) = send(
        &app,
        Method::GET,
        chat_uri(fx.rift_id, ""),
        Some(&outsider_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = post_as(&app, &outsider_token, fx.rift_id, "let me in").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The rejected post must not have been stored
    let owner_token = token_for(&fx.state, &fx.owner);
    let (status, body) = send(
        &app,
        Method::GET,
        chat_uri(fx.rift_id, ""),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
#[ignore]
async fn disabled_chat_returns_not_found() {
    let fx = fixture(ChatSettings {
        enabled: false,
        ..default_settings()
    })
    .await;
    let app = app(&fx.state);
    let token = token_for(&fx.state, &fx.owner);

    let (status, _) = send(&app, Method::GET, chat_uri(fx.rift_id, ""), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = post_as(&app, &token, fx.rift_id, "hello").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn enforces_non_empty_and_length_limits() {
    let fx = fixture(ChatSettings {
        max_message_length: 5,
        ..default_settings()
    })
    .await;
    let app = app(&fx.state);
    let token = token_for(&fx.state, &fx.owner);

    let (status, _) = post_as(&app, &token, fx.rift_id, "   \n").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = post_as(&app, &token, fx.rift_id, "abcdef").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = post_as(&app, &token, fx.rift_id, "abcde").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["content"], "abcde");
}

#[tokio::test]
#[ignore]
async fn stores_trimmed_messages_and_returns_newest_oldest_first() {
    let fx = fixture(default_settings()).await;
    let app = app(&fx.state);
    let token = token_for(&fx.state, &fx.owner);

    let (status, body) = post_as(&app, &token, fx.rift_id, "  one  ").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["content"], "one");
    assert_eq!(body["data"]["author_username"], fx.owner.username.as_str());
    post_as(&app, &token, fx.rift_id, "two").await;
    post_as(&app, &token, fx.rift_id, "three").await;

    let contents = |body: &Value| -> Vec<String> {
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap().to_string())
            .collect()
    };

    let (_, body) = send(
        &app,
        Method::GET,
        chat_uri(fx.rift_id, "?limit=2"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(contents(&body), vec!["two", "three"]);

    let (_, body) = send(&app, Method::GET, chat_uri(fx.rift_id, ""), Some(&token), None).await;
    assert_eq!(contents(&body), vec!["one", "two", "three"]);

    // A limit below 1 is clamped up to 1, which returns only the newest message
    let (_, body) = send(
        &app,
        Method::GET,
        chat_uri(fx.rift_id, "?limit=0"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(contents(&body), vec!["three"]);
}

async fn next_chat_message(ws: &mut WsStream) -> ChatMessage {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = ws
                .next()
                .await
                .expect("socket closed before a chat message arrived")
                .expect("socket error");
            if let Message::Text(text) = frame {
                if let Ok(SyncMessage::ChatMessageReceived { message, .. }) =
                    serde_json::from_str::<SyncMessage>(&text)
                {
                    return message;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for a chat broadcast")
}

#[tokio::test]
#[ignore]
async fn broadcasts_chat_to_everyone_in_the_rift() {
    let fx = fixture(default_settings()).await;
    let router = app(&fx.state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_router = router.clone();
    tokio::spawn(async move {
        axum::serve(listener, serve_router).await.unwrap();
    });

    let owner_token = token_for(&fx.state, &fx.owner);
    let member_token = token_for(&fx.state, &fx.member);
    let outsider_token = token_for(&fx.state, &fx.outsider);
    let ws_url = |token: &str| format!("ws://{}/ws/{}?token={}", addr, fx.rift_id, token);

    // A non-member cannot open the socket at all
    assert!(tokio_tungstenite::connect_async(ws_url(&outsider_token))
        .await
        .is_err());

    let (mut owner_ws, _) = tokio_tungstenite::connect_async(ws_url(&owner_token))
        .await
        .expect("owner connects");
    let (mut member_ws, _) = tokio_tungstenite::connect_async(ws_url(&member_token))
        .await
        .expect("member connects");

    // Give both sockets a moment to subscribe to the rift channel before sending
    tokio::time::sleep(Duration::from_millis(300)).await;

    let outgoing = SyncMessage::SendChatMessage {
        rift_id: fx.rift_id,
        content: "hello from the owner".to_string(),
    };
    owner_ws
        .send(Message::Text(serde_json::to_string(&outgoing).unwrap()))
        .await
        .unwrap();

    let for_member = next_chat_message(&mut member_ws).await;
    assert_eq!(for_member.content, "hello from the owner");
    assert_eq!(for_member.author_id, fx.owner.id);
    assert_eq!(for_member.author_username, fx.owner.username);

    let for_owner = next_chat_message(&mut owner_ws).await;
    assert_eq!(for_owner.id, for_member.id);

    // Broadcast messages are also persisted, so history shows them
    let (status, body) = send(
        &router,
        Method::GET,
        chat_uri(fx.rift_id, ""),
        Some(&owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stored: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(stored, vec!["hello from the owner"]);
}
