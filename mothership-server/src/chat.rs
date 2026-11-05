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
    use crate::database::Database;
    use mothership_common::RiftId;
    use uuid::Uuid;

    async fn test_db() -> Database {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        let db = Database::new(&url).await.expect("connect to test database");
        db.ensure_schema().await.expect("create schema");
        db
    }

    #[tokio::test]
    #[ignore]
    async fn stores_lists_oldest_first_and_prunes_to_newest() {
        let db = test_db().await;
        let rift_id: RiftId = Uuid::new_v4();
        let author = Uuid::new_v4();

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
        let other_rift: RiftId = Uuid::new_v4();
        db.insert_chat_message(other_rift, author, "bob", "elsewhere").await.unwrap();
        db.prune_chat_messages(rift_id, 2).await.unwrap();

        let kept = db.list_chat_messages(rift_id, 100).await.unwrap();
        let kept: Vec<&str> = kept.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(kept, vec!["msg 3", "msg 4"]);

        let other = db.list_chat_messages(other_rift, 100).await.unwrap();
        assert_eq!(other.len(), 1);
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

/// POST /api/rifts/:id/chat, stores and broadcasts a message
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
