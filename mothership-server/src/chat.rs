use chrono::Utc;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use mothership_common::{
    protocol::{ApiResponse, ChatMessage, SyncMessage},
    RiftId,
};
use serde::Deserialize;
use std::fmt;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::ServerConfig;
use crate::database::Database;
use crate::AppState;

/// Chat limits resolved from the server config at startup
#[derive(Clone, Copy, Debug)]
pub struct ChatSettings {
    pub enabled: bool,
    pub max_message_length: usize,
    pub store_history: bool,
    pub max_history: usize,
}

impl ChatSettings {
    pub fn from_config(config: &ServerConfig) -> Self {
        Self {
            enabled: config.features.chat_enabled,
            max_message_length: config.collaboration.max_chat_message_length,
            store_history: config.collaboration.store_chat_history,
            max_history: config.collaboration.max_chat_history,
        }
    }
}

#[derive(Debug)]
pub enum ChatError {
    Disabled,
    Forbidden,
    NotFound,
    Invalid(String),
    Internal(anyhow::Error),
}

impl fmt::Display for ChatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChatError::Disabled => write!(f, "chat is disabled on this server"),
            ChatError::Forbidden => write!(f, "user does not have access to this rift"),
            ChatError::NotFound => write!(f, "rift not found"),
            ChatError::Invalid(reason) => write!(f, "{}", reason),
            ChatError::Internal(e) => write!(f, "internal error: {}", e),
        }
    }
}

impl From<anyhow::Error> for ChatError {
    fn from(e: anyhow::Error) -> Self {
        ChatError::Internal(e)
    }
}

/// Trims the message and enforces non-empty and length limits (counted in characters)
pub fn validate_content(settings: &ChatSettings, content: &str) -> Result<String, ChatError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(ChatError::Invalid("chat message is empty".to_string()));
    }
    if trimmed.chars().count() > settings.max_message_length {
        return Err(ChatError::Invalid(format!(
            "chat message exceeds {} characters",
            settings.max_message_length
        )));
    }
    Ok(trimmed.to_string())
}

/// Checks that the user may read and write chat in the rift. Only rift collaborators have access;
/// project membership alone does not, and there is no project-level chat.
pub async fn ensure_rift_access(db: &Database, rift_id: RiftId, user_id: Uuid) -> Result<(), ChatError> {
    let rift = db
        .get_rift(rift_id)
        .await?
        .ok_or(ChatError::NotFound)?;

    if !rift.collaborators.contains(&user_id) {
        return Err(ChatError::Forbidden);
    }
    Ok(())
}

/// Validates, stores, and broadcasts a chat message to everyone connected to the rift
pub async fn post_chat_message(
    db: &Database,
    broadcaster: &broadcast::Sender<(String, SyncMessage)>,
    settings: &ChatSettings,
    rift_id: RiftId,
    user_id: Uuid,
    content: &str,
) -> Result<ChatMessage, ChatError> {
    if !settings.enabled {
        return Err(ChatError::Disabled);
    }

    let content = validate_content(settings, content)?;
    ensure_rift_access(db, rift_id, user_id).await?;

    let user = db
        .get_user(user_id)
        .await?
        .ok_or(ChatError::Forbidden)?;

    let message = if settings.store_history {
        let stored = db
            .insert_chat_message(rift_id, user_id, &user.username, &content)
            .await?;
        if let Err(e) = db
            .prune_chat_messages(rift_id, settings.max_history as i64)
            .await
        {
            warn!("Failed to prune chat history for rift {}: {}", rift_id, e);
        }
        stored
    } else {
        ChatMessage {
            id: Uuid::new_v4(),
            rift_id,
            author_id: user_id,
            author_username: user.username.clone(),
            content: content.to_string(),
            timestamp: Utc::now(),
        }
    };

    info!("Chat message in rift {} from {}", rift_id, user.username);

    let channel = format!("rift_{}", rift_id);
    let _ = broadcaster.send((
        channel,
        SyncMessage::ChatMessageReceived {
            rift_id,
            message: message.clone(),
        },
    ));

    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(max_len: usize) -> ChatSettings {
        ChatSettings {
            enabled: true,
            max_message_length: max_len,
            store_history: true,
            max_history: 1000,
        }
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(validate_content(&settings(10), "  hi there \n").unwrap(), "hi there");
    }

    #[test]
    fn rejects_empty_and_whitespace_only() {
        assert!(matches!(validate_content(&settings(10), ""), Err(ChatError::Invalid(_))));
        assert!(matches!(validate_content(&settings(10), "   \t\n"), Err(ChatError::Invalid(_))));
    }

    #[test]
    fn accepts_message_at_limit_and_rejects_over() {
        assert!(validate_content(&settings(5), "abcde").is_ok());
        assert!(matches!(validate_content(&settings(5), "abcdef"), Err(ChatError::Invalid(_))));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // "héllo" is 5 characters but 6 bytes, so it fits a 5-character limit
        assert!(validate_content(&settings(5), "héllo").is_ok());
        assert!(validate_content(&settings(5), "日本語テキスト").is_err());
    }
}

/// Needs a Postgres instance: set TEST_DATABASE_URL and run with `cargo test -- --ignored`
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::database::Database;
    use crate::events::{self, FILE_SAVED};
    use mothership_common::{RiftId, UserId, UserRole};
    use std::time::Duration;
    use uuid::Uuid;

    /// A database with one rift that has a real owner. Chat events reference rifts, so tests need one.
    async fn setup() -> (Database, RiftId, UserId) {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        let db = Database::new(&url).await.expect("connect to test database");
        db.ensure_schema().await.expect("create schema");

        let suffix = Uuid::new_v4().simple().to_string();
        let user = db
            .create_user(format!("chat-{}", suffix), format!("chat-{}@chat-test.local", suffix), UserRole::User)
            .await
            .unwrap();
        let project = db
            .create_project(format!("chat-db-{}", suffix), "chat db test".to_string(), vec![user.id])
            .await
            .unwrap();
        let rift = db
            .create_rift(project.id, user.id, Some("main".to_string()))
            .await
            .unwrap();
        (db, rift.id, user.id)
    }

    fn settings(store_history: bool) -> ChatSettings {
        ChatSettings {
            enabled: true,
            max_message_length: 1000,
            store_history,
            max_history: 1000,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn stores_lists_oldest_first_and_prunes_to_newest() {
        let (db, rift_id, author) = setup().await;
        let (other_db, other_rift, other_author) = setup().await;
        assert!(other_db.list_chat_messages(other_rift, 1).await.unwrap().is_empty());

        for i in 0..5 {
            db.insert_chat_message(rift_id, author, "alice", &format!("msg {}", i))
                .await
                .unwrap();
        }

        // Limit returns the newest N, but in chronological order
        let recent = db.list_chat_messages(rift_id, 3).await.unwrap();
        let contents: Vec<&str> = recent.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, vec!["msg 2", "msg 3", "msg 4"]);
        assert!(recent.iter().all(|m| m.rift_id == rift_id && m.author_username == "alice"));

        // Pruning keeps only the newest two and leaves other rifts alone
        db.insert_chat_message(other_rift, other_author, "bob", "elsewhere").await.unwrap();
        db.prune_chat_messages(rift_id, 2).await.unwrap();

        let kept = db.list_chat_messages(rift_id, 100).await.unwrap();
        let kept: Vec<&str> = kept.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(kept, vec!["msg 3", "msg 4"]);

        let other = db.list_chat_messages(other_rift, 100).await.unwrap();
        assert_eq!(other.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn chat_is_stored_as_a_chat_event_and_not_in_a_separate_table() {
        let (db, rift_id, author) = setup().await;
        let stored = db.insert_chat_message(rift_id, author, "alice", "hello").await.unwrap();

        let rows = db.list_rift_events(rift_id, 0, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, events::CHAT);
        assert!(rows[0].payload.contains(&stored.id.to_string()));

        let read = db.list_chat_messages(rift_id, 10).await.unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, stored.id);
        assert_eq!(read[0].content, "hello");
        assert_eq!(read[0].timestamp, stored.timestamp);
    }

    #[tokio::test]
    #[ignore]
    async fn retention_prunes_other_events_but_not_chat() {
        let (db, rift_id, author) = setup().await;
        for i in 0..3 {
            db.insert_chat_message(rift_id, author, "alice", &format!("chat {}", i)).await.unwrap();
        }
        for _ in 0..5 {
            db.insert_rift_event(rift_id, Some(author), FILE_SAVED, "{}").await.unwrap();
        }

        db.prune_rift_events(2).await.unwrap();

        let rows = db.list_rift_events(rift_id, 0, 100).await.unwrap();
        let files = rows.iter().filter(|row| row.event_type == FILE_SAVED).count();
        let chats = rows.iter().filter(|row| row.event_type == events::CHAT).count();
        assert_eq!(files, 2);
        assert_eq!(chats, 3);
    }

    #[tokio::test]
    #[ignore]
    async fn window_returns_chat_between_two_points_in_time() {
        let (db, rift_id, author) = setup().await;
        db.insert_chat_message(rift_id, author, "alice", "first").await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let boundary = Utc::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        db.insert_chat_message(rift_id, author, "alice", "second").await.unwrap();
        let end = Utc::now();

        let before = db.list_chat_messages_between(rift_id, None, boundary).await.unwrap();
        let before: Vec<&str> = before.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(before, vec!["first"]);

        let after = db.list_chat_messages_between(rift_id, Some(boundary), end).await.unwrap();
        let after: Vec<&str> = after.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(after, vec!["second"]);
    }

    #[tokio::test]
    #[ignore]
    async fn history_off_broadcasts_without_logging() {
        let (db, rift_id, author) = setup().await;
        let (tx, mut rx) = broadcast::channel(16);

        let message = post_chat_message(&db, &tx, &settings(false), rift_id, author, "not kept")
            .await
            .unwrap();
        assert_eq!(message.content, "not kept");

        let broadcast = rx.recv().await.unwrap();
        assert!(matches!(broadcast.1, SyncMessage::ChatMessageReceived { .. }));
        assert!(db.list_chat_messages(rift_id, 10).await.unwrap().is_empty());
        assert!(db.list_rift_events(rift_id, 0, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn backfill_moves_legacy_chat_rows_into_the_event_log_once() {
        let (db, rift_id, author) = setup().await;
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();

        // Recreate the legacy table as it used to be, with one row for a real rift and one orphan
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chat_messages (\
                id UUID PRIMARY KEY, rift_id UUID NOT NULL, author_id UUID NOT NULL, \
                author_username VARCHAR(255) NOT NULL, content TEXT NOT NULL, \
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(&pool)
        .await
        .unwrap();
        let legacy_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO chat_messages (id, rift_id, author_id, author_username, content, created_at) \
             VALUES ($1, $2, $3, 'alice', 'legacy hello', '2020-01-02T03:04:05Z'), \
                    ($4, $5, $3, 'alice', 'orphan', NOW())",
        )
        .bind(legacy_id)
        .bind(rift_id)
        .bind(author)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        db.ensure_schema().await.unwrap();
        db.ensure_schema().await.unwrap();

        let read = db.list_chat_messages(rift_id, 10).await.unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, legacy_id);
        assert_eq!(read[0].content, "legacy hello");
        assert_eq!(read[0].timestamp.to_rfc3339(), "2020-01-02T03:04:05+00:00");
        assert_eq!(db.list_rift_events(rift_id, 0, 10).await.unwrap().len(), 1);
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SendChatRequest {
    pub content: String,
}

pub fn status_for(err: &ChatError) -> StatusCode {
    match err {
        ChatError::Disabled => StatusCode::NOT_FOUND,
        ChatError::Forbidden => StatusCode::FORBIDDEN,
        ChatError::NotFound => StatusCode::NOT_FOUND,
        ChatError::Invalid(_) => StatusCode::BAD_REQUEST,
        ChatError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub fn authenticated_user(state: &AppState, headers: &HeaderMap) -> Result<Uuid, StatusCode> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = state
        .auth
        .verify_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Uuid::parse_str(&claims.sub).map_err(|_| StatusCode::UNAUTHORIZED)
}

/// GET /api/rifts/:id/chat?limit=N, returns the newest messages, oldest first
pub async fn get_chat_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rift_id): Path<RiftId>,
    Query(query): Query<ChatHistoryQuery>,
) -> Result<Json<ApiResponse<Vec<ChatMessage>>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    if !state.sync.chat.enabled {
        return Err(StatusCode::NOT_FOUND);
    }

    ensure_rift_access(&state.db, rift_id, user_id)
        .await
        .map_err(|e| status_for(&e))?;

    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, state.sync.chat.max_history.max(1) as i64);

    let messages = state
        .db
        .list_chat_messages(rift_id, limit)
        .await
        .map_err(|e| {
            warn!("Failed to load chat history for rift {}: {}", rift_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ApiResponse::success(messages)))
}

/// GET /api/rifts/:id/checkpoints/:checkpoint_id/chat. Returns the chat posted after the rift's previous
/// checkpoint and up to this one. Nothing is copied: the messages are read from the event log.
pub async fn get_checkpoint_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((rift_id, checkpoint_id)): Path<(RiftId, Uuid)>,
) -> Result<Json<ApiResponse<Vec<ChatMessage>>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    if !state.sync.chat.enabled {
        return Err(StatusCode::NOT_FOUND);
    }

    ensure_rift_access(&state.db, rift_id, user_id)
        .await
        .map_err(|e| status_for(&e))?;

    let checkpoint = match state.sync.storage.load_checkpoint(checkpoint_id).await {
        Ok(Some(checkpoint)) if checkpoint.rift_id == rift_id => checkpoint,
        Ok(_) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            warn!("Failed to load checkpoint {}: {}", checkpoint_id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let checkpoints = state.sync.storage.list_checkpoints(rift_id).await.map_err(|e| {
        warn!("Failed to list checkpoints for rift {}: {}", rift_id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let previous = checkpoints
        .iter()
        .map(|cp| cp.timestamp)
        .filter(|timestamp| *timestamp < checkpoint.timestamp)
        .max();

    let messages = state
        .db
        .list_chat_messages_between(rift_id, previous, checkpoint.timestamp)
        .await
        .map_err(|e| {
            warn!("Failed to load chat for checkpoint {}: {}", checkpoint_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ApiResponse::success(messages)))
}

pub async fn post_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rift_id): Path<RiftId>,
    Json(req): Json<SendChatRequest>,
) -> Result<Json<ApiResponse<ChatMessage>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    let message = post_chat_message(
        &state.db,
        &state.sync.broadcaster,
        &state.sync.chat,
        rift_id,
        user_id,
        &req.content,
    )
    .await
    .map_err(|e| status_for(&e))?;

    Ok(Json(ApiResponse::success(message)))
}
