//! HTTP tests for recording check results, through the real router.
//!
//! These need Postgres with the root migrations applied: set TEST_DATABASE_URL and run
//! `cargo test -p mothership-server verification_api -- --ignored --test-threads=1` with SQLX_OFFLINE=true.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::Utc;
use mothership_common::{
    auth::Claims,
    verify::{CheckResult, VerificationReport, VerificationResult},
    ProjectId, RiftId, User, UserRole,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    auth::AuthService, chat::ChatSettings, config::ServerConfig, create_api_router,
    database::Database, events, oauth::OAuthService, storage::StorageEngine, sync::SyncState,
    AppState,
};

const TEST_SECRET: &str = "verification-api-test-secret";

struct Fixture {
    state: AppState,
    project: ProjectId,
    owner: User,
    member: User,
    outsider: User,
    owner_rift: RiftId,
    member_rift: RiftId,
}

/// A project with an owner and a member, who each have their own rift. The outsider is not in the project.
async fn fixture() -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let db = Database::new(&url).await.expect("connect to test database");
    db.ensure_schema().await.expect("create schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let make_user = |label: &str| {
        let name = format!("{}-{}", label, suffix);
        db.create_user(name.clone(), format!("{}@verification-test.local", name), UserRole::User)
    };
    let owner = make_user("owner").await.unwrap();
    let member = make_user("member").await.unwrap();
    let outsider = make_user("outsider").await.unwrap();

    let project = db
        .create_project(
            format!("verification-test-{}", suffix),
            "verification API test".to_string(),
            vec![owner.id, member.id],
        )
        .await
        .unwrap();
    let owner_rift = db
        .create_rift(project.id, owner.id, Some("main".to_string()))
        .await
        .unwrap();
    let member_rift = db
        .create_rift(project.id, member.id, Some("member-work".to_string()))
        .await
        .unwrap();

    let storage_root = std::env::temp_dir().join(format!("mothership-verification-{}", suffix));
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
        project: project.id,
        owner,
        member,
        outsider,
        owner_rift: owner_rift.id,
        member_rift: member_rift.id,
    }
}

fn token_for(state: &AppState, user: &User) -> String {
    let now = Utc::now();
    let claims = Claims {
        sub: user.id.to_string(),
        machine_id: "verification-api-test".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::hours(1)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    state.auth.encode_token(&claims).unwrap()
}

async fn post(app: &Router, uri: String, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
    }
    let request = builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn verifications_uri(project: ProjectId) -> String {
    format!("/projects/{}/verifications", project)
}

fn failing_result() -> VerificationResult {
    VerificationResult {
        passed: false,
        checks: vec![
            CheckResult { name: "unit tests".into(), passed: true, exit_code: Some(0) },
            CheckResult { name: "lint".into(), passed: false, exit_code: Some(2) },
        ],
        ran_at: Utc::now(),
    }
}

/// Events are written by spawned tasks, so poll until the verification event shows up
async fn verification_events(db: &Database, rift: RiftId) -> Vec<Value> {
    for _ in 0..100 {
        let rows = db.list_rift_events(rift, 0, 1000).await.unwrap();
        let found: Vec<Value> = rows
            .iter()
            .filter(|row| row.event_type == events::VERIFICATION)
            .map(|row| serde_json::from_str(&row.payload).unwrap())
            .collect();
        if !found.is_empty() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Vec::new()
}

#[tokio::test]
#[ignore]
async fn result_is_attached_to_the_checkpoint_and_logged() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    let checkpoint = fx
        .state
        .sync
        .storage
        .create_checkpoint(fx.owner_rift, fx.owner.id, Some("base".into()), false, None)
        .await
        .unwrap();
    assert!(checkpoint.verification.is_none());

    let report = VerificationReport {
        checkpoint_id: Some(checkpoint.id),
        result: failing_result(),
    };
    let (status, _) = post(
        &app,
        verifications_uri(fx.project),
        Some(&token),
        serde_json::to_value(&report).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let stored = fx.state.sync.storage.load_checkpoint(checkpoint.id).await.unwrap().unwrap();
    let verification = stored.verification.expect("verification is attached");
    assert!(!verification.passed);
    assert_eq!(verification.checks.len(), 2);

    let logged = verification_events(&fx.state.db, fx.owner_rift).await;
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["passed"], json!(false));
    assert_eq!(logged[0]["checkpoint_id"], json!(checkpoint.id));
}

#[tokio::test]
#[ignore]
async fn a_manual_run_is_logged_without_a_checkpoint() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    let report = VerificationReport {
        checkpoint_id: None,
        result: VerificationResult {
            passed: true,
            checks: vec![CheckResult { name: "unit tests".into(), passed: true, exit_code: Some(0) }],
            ran_at: Utc::now(),
        },
    };
    let (status, _) = post(
        &app,
        verifications_uri(fx.project),
        Some(&token),
        serde_json::to_value(&report).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let logged = verification_events(&fx.state.db, fx.owner_rift).await;
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["passed"], json!(true));
    assert!(logged[0]["checkpoint_id"].is_null());
    assert!(fx.state.sync.storage.list_checkpoints(fx.owner_rift).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn checkpoints_from_another_rift_and_unknown_ids_are_not_found() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let token = token_for(&fx.state, &fx.owner);

    let member_checkpoint = fx
        .state
        .sync
        .storage
        .create_checkpoint(fx.member_rift, fx.member.id, None, false, None)
        .await
        .unwrap();

    for checkpoint_id in [member_checkpoint.id, Uuid::new_v4()] {
        let report = VerificationReport {
            checkpoint_id: Some(checkpoint_id),
            result: failing_result(),
        };
        let (status, _) = post(
            &app,
            verifications_uri(fx.project),
            Some(&token),
            serde_json::to_value(&report).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let stored = fx.state.sync.storage.load_checkpoint(member_checkpoint.id).await.unwrap().unwrap();
    assert!(stored.verification.is_none());
}

#[tokio::test]
#[ignore]
async fn only_project_members_can_record_results() {
    let fx = fixture().await;
    let app = create_api_router(fx.state.clone());
    let body = serde_json::to_value(VerificationReport {
        checkpoint_id: None,
        result: failing_result(),
    })
    .unwrap();

    let (status, _) = post(&app, verifications_uri(fx.project), None, body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let outsider_token = token_for(&fx.state, &fx.outsider);
    let (status, _) = post(&app, verifications_uri(fx.project), Some(&outsider_token), body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
