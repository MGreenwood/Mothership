//! Rift event log: records what happens in a rift and serves it as a timeline.
//!
//! Writes are fire-and-forget so file saves are never slowed down by logging.
//! Readers page through events with an `after` cursor (the event id).

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use mothership_common::{client::RiftEvent, protocol::ApiResponse, RiftId};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{chat, database::{Database, RiftEventRow}, AppState};

pub const FILE_SAVED: &str = "file_saved";
pub const CHECKPOINT: &str = "checkpoint";
pub const RESTORE: &str = "restore";
pub const CHAT: &str = "chat";
pub const JOIN: &str = "join";
pub const LEAVE: &str = "leave";
pub const ACCESS_CHANGE: &str = "access_change";
pub const WORMHOLE: &str = "wormhole";

const RETENTION_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Append an event to a rift's log without blocking the caller
pub fn record(db: &Database, rift_id: RiftId, actor: Option<Uuid>, event_type: &'static str, payload: serde_json::Value) {
    let db = db.clone();
    let payload = payload.to_string();
    tokio::spawn(async move {
        if let Err(e) = db.insert_rift_event(rift_id, actor, event_type, &payload).await {
            warn!("Failed to record {} event for rift {}: {}", event_type, rift_id, e);
        }
    });
}

/// Periodically drop the oldest events beyond `keep` per rift. A negative `keep` keeps everything.
pub fn spawn_retention_task(db: Database, keep: i64) {
    if keep < 0 {
        info!("Rift event retention: keeping all events");
        return;
    }
    info!("Rift event retention: keeping the newest {} events per rift", keep);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_INTERVAL);
        loop {
            interval.tick().await;
            match db.prune_rift_events(keep).await {
                Ok(0) => {}
                Ok(removed) => info!("Rift event retention removed {} old events", removed),
                Err(e) => warn!("Rift event retention failed: {}", e),
            }
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    /// Return events with an id greater than this (0 or missing returns from the start)
    #[serde(default)]
    pub after: i64,
    /// Return the `limit` events just before this id, oldest first (for paging back through history)
    pub before: Option<i64>,
    pub limit: Option<i64>,
    /// Return the newest `limit` events instead of the ones after `after`
    #[serde(default)]
    pub latest: bool,
    /// Seconds to hold the request open when there is nothing new yet (capped at MAX_WAIT_SECS).
    /// Only applies to forward reads; ignored with `latest` or `before`.
    #[serde(default)]
    pub wait: u64,
}

const MAX_WAIT_SECS: u64 = 30;

/// GET /api/rifts/:id/events. Only rift collaborators may read it.
/// With `wait`, an empty forward read is held open until the rift gets a new event or the wait ends.
pub async fn list_rift_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rift_id): Path<RiftId>,
    Query(query): Query<EventQuery>,
) -> Result<Json<ApiResponse<Vec<RiftEvent>>>, StatusCode> {
    let user_id = chat::authenticated_user(&state, &headers)?;

    chat::ensure_rift_access(&state.db, rift_id, user_id)
        .await
        .map_err(|e| chat::status_for(&e))?;

    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let forward = !query.latest && query.before.is_none();
    let wait = if forward { query.wait.min(MAX_WAIT_SECS) } else { 0 };

    // Subscribe before the first read so an event written in between is caught by the wait
    let mut notifications = state.db.subscribe_rift_events();
    let mut rows = read_events(&state.db, rift_id, &query, limit).await?;

    if rows.is_empty() && wait > 0 {
        let _ = tokio::time::timeout(Duration::from_secs(wait), wait_for_rift(&mut notifications, rift_id)).await;
        rows = read_events(&state.db, rift_id, &query, limit).await?;
    }

    let events = rows.into_iter().map(to_view).collect();
    Ok(Json(ApiResponse::success(events)))
}

async fn read_events(
    db: &Database,
    rift_id: RiftId,
    query: &EventQuery,
    limit: i64,
) -> Result<Vec<RiftEventRow>, StatusCode> {
    let result = if query.latest {
        db.list_latest_rift_events(rift_id, limit).await
    } else if let Some(before) = query.before {
        db.list_rift_events_before(rift_id, before, limit).await
    } else {
        db.list_rift_events(rift_id, query.after.max(0), limit).await
    };
    result.map_err(|e| {
        warn!("Failed to load events for rift {}: {}", rift_id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Returns once an event for `rift_id` is announced. A lagged receiver also returns, so the caller re-reads.
async fn wait_for_rift(notifications: &mut Receiver<RiftId>, rift_id: RiftId) {
    loop {
        match notifications.recv().await {
            Ok(id) if id == rift_id => return,
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) | Err(RecvError::Closed) => return,
        }
    }
}

fn to_view(row: RiftEventRow) -> RiftEvent {
    RiftEvent {
        id: row.id,
        event_type: row.event_type,
        actor_id: row.actor_id,
        actor_username: row.actor_username,
        payload: serde_json::from_str(&row.payload).unwrap_or(serde_json::Value::Null),
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::chat::{self, ChatSettings};
    use mothership_common::UserRole;

    /// Events are written by spawned tasks, so poll until the expected count shows up
    async fn wait_for_events(db: &Database, rift_id: RiftId, expected: usize) -> Vec<RiftEventRow> {
        for _ in 0..100 {
            let rows = db.list_rift_events(rift_id, 0, 1000).await.unwrap();
            if rows.len() >= expected {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        db.list_rift_events(rift_id, 0, 1000).await.unwrap()
    }

    async fn setup() -> (Database, mothership_common::User, RiftId) {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        let db = Database::new(&url).await.expect("connect to test database");
        db.ensure_schema().await.expect("create schema");

        let suffix = Uuid::new_v4().simple().to_string();
        let user = db
            .create_user(
                format!("events-{}", suffix),
                format!("events-{}@events-test.local", suffix),
                UserRole::User,
            )
            .await
            .unwrap();
        let project = db
            .create_project(format!("events-test-{}", suffix), "event log test".to_string(), vec![user.id])
            .await
            .unwrap();
        let rift = db
            .create_rift(project.id, user.id, Some("main".to_string()))
            .await
            .unwrap();
        (db, user, rift.id)
    }

    #[tokio::test]
    #[ignore]
    async fn chat_post_is_logged_with_actor_and_content() {
        let (db, user, rift_id) = setup().await;
        let settings = ChatSettings {
            enabled: true,
            max_message_length: 1000,
            store_history: true,
            max_history: 1000,
        };
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        chat::post_chat_message(&db, &tx, &settings, rift_id, user.id, "hello timeline")
            .await
            .unwrap();

        let rows = wait_for_events(&db, rift_id, 1).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, CHAT);
        assert_eq!(rows[0].actor_id, Some(user.id));
        assert_eq!(rows[0].actor_username.as_deref(), Some(user.username.as_str()));
        assert!(rows[0].payload.contains("hello timeline"));
    }

    #[tokio::test]
    #[ignore]
    async fn cursor_returns_only_newer_events_in_order() {
        let (db, user, rift_id) = setup().await;
        for kind in [FILE_SAVED, CHECKPOINT, JOIN] {
            db.insert_rift_event(rift_id, Some(user.id), kind, "{}").await.unwrap();
        }

        let all = db.list_rift_events(rift_id, 0, 1000).await.unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|pair| pair[0].id < pair[1].id));

        let newer = db.list_rift_events(rift_id, all[0].id, 1000).await.unwrap();
        let kinds: Vec<&str> = newer.iter().map(|row| row.event_type.as_str()).collect();
        assert_eq!(kinds, vec![CHECKPOINT, JOIN]);
    }

    #[tokio::test]
    #[ignore]
    async fn retention_keeps_only_the_newest_events_per_rift() {
        let (db, user, rift_id) = setup().await;
        for _ in 0..5 {
            db.insert_rift_event(rift_id, Some(user.id), FILE_SAVED, "{}").await.unwrap();
        }

        db.prune_rift_events(2).await.unwrap();

        let remaining = db.list_rift_events(rift_id, 0, 1000).await.unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn latest_returns_newest_in_order_and_writes_notify() {
        let (db, user, rift_id) = setup().await;
        let mut notifications = db.subscribe_rift_events();
        for kind in [FILE_SAVED, CHECKPOINT, JOIN] {
            db.insert_rift_event(rift_id, Some(user.id), kind, "{}").await.unwrap();
        }

        // Other rifts may be writing concurrently, so skip their notifications
        loop {
            if notifications.recv().await.unwrap() == rift_id {
                break;
            }
        }

        let latest = db.list_latest_rift_events(rift_id, 2).await.unwrap();
        let kinds: Vec<&str> = latest.iter().map(|row| row.event_type.as_str()).collect();
        assert_eq!(kinds, vec![CHECKPOINT, JOIN]);

        let older = db.list_rift_events_before(rift_id, latest[0].id, 10).await.unwrap();
        let kinds: Vec<&str> = older.iter().map(|row| row.event_type.as_str()).collect();
        assert_eq!(kinds, vec![FILE_SAVED]);
    }

    #[tokio::test]
    #[ignore]
    async fn wait_returns_when_the_rift_gets_an_event() {
        let (db, user, rift_id) = setup().await;
        let mut notifications = db.subscribe_rift_events();

        let writer = db.clone();
        let user_id = user.id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            writer.insert_rift_event(rift_id, Some(user_id), CHAT, "{}").await.unwrap();
        });

        let waited = tokio::time::timeout(Duration::from_secs(5), wait_for_rift(&mut notifications, rift_id)).await;
        assert!(waited.is_ok(), "wait should return once the event is written");
    }
}
