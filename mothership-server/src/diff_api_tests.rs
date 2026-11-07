//! End-to-end tests for diffs between rifts and checkpoints, through the real router.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server diff_api -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

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

const TEST_SECRET: &str = "diff-api-test-secret";

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
        .create_user(name.clone(), format!("{}@diff-test.local", name), UserRole::User)
        .await
        .unwrap();
    let name = format!("outsider-{}", suffix);
    let outsider = db
        .create_user(name.clone(), format!("{}@diff-test.local", name), UserRole::User)
        .await
        .unwrap();
    let project = db
        .create_project(format!("diff-test-{}", suffix), "diff API test".to_string(), vec![owner.id])
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

    let storage_root = std::env::temp_dir().join(format!("mothership-diff-api-{}", suffix));
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
        machine_id: "diff-api-test".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::hours(1)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    state.auth.encode_token(&claims).unwrap()
}

async fn get(app: &Router, uri: String, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn set_live(fx: &Fixture, rift: RiftId, files: &[(&str, &str)]) {
    for (path, content) in files {
        fx.state
            .sync
            .storage
            .update_live_state(rift, PathBuf::from(path), content.to_string())
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn rift_to_rift_diff_lists_each_changed_file_with_line_counts() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    set_live(&fx, fx.main, &[("README.md", "a\nb\nc\n"), ("old.txt", "gone")]).await;
    set_live(&fx, fx.feature, &[("README.md", "a\nB\nc\nd\n"), ("new.txt", "hi")]).await;

    let (status, body) = get(&app, "/api/rifts/diff?from=main&to=feature".into(), &token).await;
    assert_eq!(status, StatusCode::OK);
    let diffs = body["data"].as_array().unwrap();

    let readme = diffs.iter().find(|d| d["path"] == "README.md").expect("README.md changed");
    assert_eq!(readme["status"], "modified");
    assert_eq!((readme["added"].as_u64(), readme["removed"].as_u64()), (Some(2), Some(1)));
    assert!(readme.get("unified").is_none(), "summaries omit unified text unless full is set");

    assert_eq!(diffs.iter().find(|d| d["path"] == "old.txt").unwrap()["status"], "removed");
    assert_eq!(diffs.iter().find(|d| d["path"] == "new.txt").unwrap()["status"], "added");
}

#[tokio::test]
#[ignore]
async fn checkpoint_to_current_rift_diff_shows_what_changed_since() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    set_live(&fx, fx.main, &[("README.md", "v1\n")]).await;
    let checkpoint = fx
        .state
        .sync
        .storage
        .create_checkpoint(fx.main, fx.owner.id, Some("base".into()), false, None)
        .await
        .unwrap();

    set_live(&fx, fx.main, &[("README.md", "v2\n"), ("added.txt", "new")]).await;

    let uri = format!("/api/rifts/diff?from=checkpoint:{}&to=main&full=true", checkpoint.id);
    let (status, body) = get(&app, uri, &token).await;
    assert_eq!(status, StatusCode::OK);
    let diffs = body["data"].as_array().unwrap();
    assert_eq!(diffs.len(), 2);

    let readme = diffs.iter().find(|d| d["path"] == "README.md").unwrap();
    let unified = readme["unified"].as_str().expect("full diffs carry unified text");
    assert!(unified.contains("-v1") && unified.contains("+v2"), "{unified}");
    assert_eq!(diffs.iter().find(|d| d["path"] == "added.txt").unwrap()["status"], "added");
}

#[tokio::test]
#[ignore]
async fn single_file_diff_returns_its_unified_text_and_404s_when_unchanged() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    set_live(&fx, fx.main, &[("src/main.rs", "fn a() {}\n")]).await;
    set_live(&fx, fx.feature, &[("src/main.rs", "fn b() {}\n")]).await;

    let (status, body) = get(&app, "/api/rifts/diff/file?from=main&to=feature&path=src/main.rs".into(), &token).await;
    assert_eq!(status, StatusCode::OK);
    let unified = body["data"]["unified"].as_str().unwrap();
    assert!(unified.contains("-fn a() {}") && unified.contains("+fn b() {}"), "{unified}");

    // A file that is the same on both sides has no diff
    set_live(&fx, fx.feature, &[("src/main.rs", "fn a() {}\n")]).await;
    let (status, _) = get(&app, "/api/rifts/diff/file?from=main&to=feature&path=src/main.rs".into(), &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn a_checkpoint_side_needs_access_to_its_rift() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());

    set_live(&fx, fx.main, &[("README.md", "v1\n")]).await;
    let checkpoint = fx
        .state
        .sync
        .storage
        .create_checkpoint(fx.main, fx.owner.id, None, false, None)
        .await
        .unwrap();

    let uri = format!("/api/rifts/diff?from=checkpoint:{}&to=feature", checkpoint.id);
    let (status, _) = get(&app, uri, &token_for(&fx.state, &fx.outsider)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = get(&app, "/api/rifts/diff?from=checkpoint:not-a-uuid&to=main".into(), &token_for(&fx.state, &fx.owner)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
