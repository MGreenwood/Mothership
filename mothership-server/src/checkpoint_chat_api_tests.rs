//! HTTP tests for the chat shown with each checkpoint, through the real router.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server checkpoint_chat -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::Utc;
use mothership_common::{auth::Claims, RiftId, User, UserRole};
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat::ChatSettings, config::ServerConfig, create_api_router,
    database::Database, oauth::OAuthService, storage::StorageEngine, sync::SyncState, AppState,
};

const TEST_SECRET: &str = "checkpoint-chat-api-test-secret";

struct Fixture {
    state: AppState,
    owner: User,
    outsider: User,
    main: RiftId,
    feature: RiftId,
}

/// A project with an owner who collaborates on two rifts, `main` and `feature`. The outsider is in
/// no rift.
async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("owner-{}", suffix);
    let owner = db
        .create_user(name.clone(), format!("{}@checkpoint-chat-test.local", name), UserRole::User)
        .await
        .unwrap();
    let name = format!("outsider-{}", suffix);
    let outsider = db
        .create_user(name.clone(), format!("{}@checkpoint-chat-test.local", name), UserRole::User)
        .await
        .unwrap();
    let project = db
        .create_project(
            format!("checkpoint-chat-test-{}", suffix),
            "checkpoint chat API test".to_string(),
            vec![owner.id],
        )
        .await
        .unwrap();
    let main = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();
    let feature = db
        .create_rift(project.id, owner.id, Some("feature".to_string()))
        .await
        .unwrap();

    let storage_root = std::env::temp_dir().join(format!("mothership-checkpoint-chat-{}", suffix));
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
        outsider,
        main: main.id,
        feature: feature.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "checkpoint-chat-api-test".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::hours(1)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    state.auth.encode_token(&claims).unwrap()
}

async fn get(app: &Router, uri: String, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn checkpoint_chat_uri(rift_id: RiftId, checkpoint_id: Uuid) -> String {
    format!("/api/rifts/{}/checkpoints/{}/chat", rift_id, checkpoint_id)
}

fn contents(body: &Value) -> Vec<&str> {
    body["data"]
        .as_array()
        .expect("data is a list")
        .iter()
        .map(|message| message["content"].as_str().unwrap())
        .collect()
}

/// Checkpoints and chat both use the wall clock, so separate them by a few milliseconds
async fn tick() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
#[ignore]
async fn checkpoint_chat_is_the_chat_since_the_previous_checkpoint() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);
    let db = &fx.state.db;
    let storage = &fx.state.sync.storage;
    let post = |content: &'static str| {
        db.insert_chat_message(fx.main, fx.owner.id, &fx.owner.username, content)
    };

    post("before first").await.unwrap();
    tick().await;
    let first = storage
        .create_checkpoint(fx.main, fx.owner.id, Some("first".into()), false, None)
        .await
        .unwrap();
    tick().await;
    post("between").await.unwrap();
    tick().await;
    let second = storage
        .create_checkpoint(fx.main, fx.owner.id, Some("second".into()), false, None)
        .await
        .unwrap();
    tick().await;
    post("after last").await.unwrap();

    let (status, body) = get(&app, checkpoint_chat_uri(fx.main, first.id), Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(contents(&body), vec!["before first"]);

    let (status, body) = get(&app, checkpoint_chat_uri(fx.main, second.id), Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(contents(&body), vec!["between"]);
}

#[tokio::test]
#[ignore]
async fn checkpoint_chat_requires_rift_access_and_a_checkpoint_in_that_rift() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let owner_token = token_for(&fx.state, &fx.owner);
    let outsider_token = token_for(&fx.state, &fx.outsider);

    let feature_checkpoint = fx
        .state
        .sync
        .storage
        .create_checkpoint(fx.feature, fx.owner.id, None, false, None)
        .await
        .unwrap();

    let (status, _) = get(&app, checkpoint_chat_uri(fx.main, feature_checkpoint.id), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(&app, checkpoint_chat_uri(fx.main, feature_checkpoint.id), Some(&outsider_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A checkpoint from another rift is not found through this rift
    let (status, _) = get(&app, checkpoint_chat_uri(fx.main, feature_checkpoint.id), Some(&owner_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get(&app, checkpoint_chat_uri(fx.main, Uuid::new_v4()), Some(&owner_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
