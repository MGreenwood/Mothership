//! End-to-end tests for scoped checkpoints: partial captures, restores of the folded state, and the
//! rules that refuse invalid requests.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server checkpoint_api -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::Utc;
use mothership_common::{auth::Claims, RiftId, User, UserRole};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat::ChatSettings, config::ServerConfig, create_api_router,
    database::Database, events, oauth::OAuthService, storage::StorageEngine, sync::SyncState,
    AppState,
};

const TEST_SECRET: &str = "checkpoint-api-test-secret";

struct Fixture {
    state: AppState,
    owner: User,
    project_id: Uuid,
    rift: RiftId,
}

/// A project with one owner, who has a single rift
async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("owner-{}", suffix);
    let owner = db
        .create_user(name.clone(), format!("{}@checkpoint-test.local", name), UserRole::User)
        .await
        .unwrap();
    let project = db
        .create_project(format!("checkpoint-test-{}", suffix), "checkpoint API test".to_string(), vec![owner.id])
        .await
        .unwrap();
    let rift = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();

    let storage_root = std::env::temp_dir().join(format!("mothership-checkpoint-api-{}", suffix));
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
        project_id: project.id,
        rift: rift.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "checkpoint-api-test".to_string(),
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn set_live(fx: &Fixture, files: &[(&str, &str)]) {
    for (path, content) in files {
        fx.state
            .sync
            .storage
            .update_live_state(fx.rift, PathBuf::from(path), content.to_string())
            .await
            .unwrap();
    }
}

async fn checkpoint(app: &Router, fx: &Fixture, token: &str, paths: Option<Value>) -> (StatusCode, Value) {
    let mut body = json!({ "message": "test", "timestamp": Utc::now() });
    if let Some(paths) = paths {
        body["paths"] = paths;
    }
    send(app, Method::POST, format!("/projects/{}/checkpoints", fx.project_id), token, Some(body)).await
}

#[tokio::test]
#[ignore]
async fn scoped_checkpoint_records_the_selection_and_restores_the_folded_state() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    set_live(&fx, &[("README.md", "v1"), ("src/main.rs", "v1")]).await;
    let (status, _) = checkpoint(&app, &fx, &token, None).await;
    assert_eq!(status, StatusCode::OK);

    set_live(&fx, &[("README.md", "v2"), ("src/main.rs", "v2")]).await;
    let (status, body) = checkpoint(&app, &fx, &token, Some(json!(["src/"]))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["file_count"], 1, "only src/ is captured");
    let partial_id = body["data"]["checkpoint_id"].as_str().unwrap().to_string();

    // Restoring the partial checkpoint gives the whole rift as it was then
    let (status, body) = send(
        &app,
        Method::POST,
        format!("/projects/{}/checkpoints/{}/restore", fx.project_id, partial_id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["files"]["src/main.rs"], "v2");
    assert_eq!(body["data"]["files"]["README.md"], "v1", "unselected files keep their earlier version");

    // The checkpoint event says it was partial
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let events = fx.state.db.list_rift_events(fx.rift, 0, 100).await.unwrap();
        let partial_event = events
            .iter()
            .any(|e| e.event_type == events::CHECKPOINT && e.payload.contains("\"partial\":true"));
        if partial_event {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no partial checkpoint event was recorded");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
#[ignore]
async fn invalid_selections_are_refused_with_400() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);
    set_live(&fx, &[("README.md", "v1")]).await;

    // A partial checkpoint needs an earlier checkpoint to build on
    let (status, _) = checkpoint(&app, &fx, &token, Some(json!(["README.md"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "first checkpoint must be whole-rift");

    let (status, _) = checkpoint(&app, &fx, &token, None).await;
    assert_eq!(status, StatusCode::OK);

    // Nothing in the rift matches, and paths outside the project are never accepted
    let (status, _) = checkpoint(&app, &fx, &token, Some(json!(["missing.rs"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = checkpoint(&app, &fx, &token, Some(json!(["../secret"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore]
async fn a_checkpoint_with_partial_children_cannot_be_deleted() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    set_live(&fx, &[("README.md", "v1"), ("src/main.rs", "v1")]).await;
    let (_, base) = checkpoint(&app, &fx, &token, None).await;
    let base_id = base["data"]["checkpoint_id"].as_str().unwrap().to_string();

    set_live(&fx, &[("src/main.rs", "v2")]).await;
    let (_, partial) = checkpoint(&app, &fx, &token, Some(json!(["src/"]))).await;
    let partial_id = partial["data"]["checkpoint_id"].as_str().unwrap().to_string();

    let delete = |id: String| {
        send(
            &app,
            Method::DELETE,
            format!("/projects/{}/checkpoints/{}", fx.project_id, id),
            &token,
            None,
        )
    };

    let (status, _) = delete(base_id.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "the partial checkpoint restores through its parent");

    // Once the partial checkpoint is gone, its parent can be deleted
    let (status, _) = delete(partial_id).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = delete(base_id).await;
    assert_eq!(status, StatusCode::OK);
}
