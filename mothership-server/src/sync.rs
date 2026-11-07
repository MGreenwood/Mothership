use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use mothership_common::protocol::{ApiResponse, SyncMessage, FileDiffChange};
use mothership_common::RiftId;
use mothership_common::diff::DiffEngine;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock, Mutex};
use tracing::{error, info, warn, debug};
use uuid::Uuid;
use std::path::PathBuf;
use sha2::{Sha256, Digest};
use flate2;

use crate::chat::{self, ChatSettings};
use crate::database::Database;
use crate::events;
use crate::storage::StorageEngine;
use crate::AppState;

/// PERFORMANCE FIX: Batching state for reducing message overhead
#[derive(Default)]
struct BatchingState {
    pending_changes: HashMap<String, Vec<FileDiffChange>>, // rift_id -> changes
    last_batch_time: HashMap<String, Instant>, // rift_id -> time
    pending_author: HashMap<String, (Uuid, String)>, // rift_id -> (author, origin connection) of the pending batch
}

/// Stream manager for handling large file transfers
#[derive(Default)]
struct StreamManager {
    /// Active streams - maps stream_id to stream metadata
    active_streams: HashMap<Uuid, StreamMetadata>,
}

/// Metadata for an active stream
struct StreamMetadata {
    rift_id: Uuid,
    file_path: PathBuf,
    total_size: u64,
    chunk_size: u32,
    expected_chunks: u32,
    received_chunks: HashMap<u32, Vec<u8>>, // chunk_index -> data
    temp_file_path: PathBuf,
    is_binary: bool,
    content_hash: String,
    base_hash: Option<String>,
    started_at: Instant,
}

impl StreamMetadata {
    fn new(stream_id: Uuid, rift_id: Uuid, file_path: PathBuf, total_size: u64, chunk_size: u32, is_binary: bool, content_hash: String, base_hash: Option<String>) -> Self {
        let expected_chunks = ((total_size + chunk_size as u64 - 1) / chunk_size as u64) as u32;
        let temp_file_path = std::env::temp_dir().join(format!("mothership_stream_{}", stream_id));
        
        Self {
            rift_id,
            file_path,
            total_size,
            chunk_size,
            expected_chunks,
            received_chunks: HashMap::new(),
            temp_file_path,
            is_binary,
            content_hash,
            base_hash,
            started_at: Instant::now(),
        }
    }
    
    fn is_complete(&self) -> bool {
        self.received_chunks.len() as u32 == self.expected_chunks
    }
}

const BATCH_TIMEOUT: Duration = Duration::from_millis(100); // 100ms batching window
const MAX_BATCH_SIZE: usize = 50; // Maximum changes per batch

#[derive(Clone)]
pub struct SyncState {
    pub db: Database,
    pub storage: Arc<StorageEngine>,
    pub broadcaster: broadcast::Sender<(String, SyncMessage)>,
    pub batching_state: Arc<RwLock<BatchingState>>, // PERFORMANCE FIX: Batching support
    pub stream_manager: Arc<Mutex<StreamManager>>, // Streaming support
    // Rate limiting: track recent change timestamps per rift to detect floods
    pub recent_change_timestamps: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
    pub chat: ChatSettings,
    /// Live joined connections per rift: rift -> (connection channel -> username)
    pub presence: Arc<RwLock<HashMap<Uuid, HashMap<String, String>>>>,
}

/// Distinct usernames in a rift's connection map, sorted
fn collect_participants(connections: &HashMap<String, String>) -> Vec<String> {
    let mut names: Vec<String> = connections.values().cloned().collect();
    names.sort();
    names.dedup();
    names
}

/// Distinct usernames with a live, joined connection to the rift
pub async fn rift_participants(state: &SyncState, rift_id: Uuid) -> Vec<String> {
    let presence = state.presence.read().await;
    presence.get(&rift_id).map(collect_participants).unwrap_or_default()
}

/// GET /api/rifts/:id/presence, usernames currently connected to the rift
pub async fn get_rift_presence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rift_id): Path<RiftId>,
) -> Result<Json<ApiResponse<Vec<String>>>, StatusCode> {
    let user_id = chat::authenticated_user(&state, &headers)?;
    chat::ensure_rift_access(&state.db, rift_id, user_id)
        .await
        .map_err(|e| chat::status_for(&e))?;
    Ok(Json(ApiResponse::success(rift_participants(&state.sync, rift_id).await)))
}

/// Tells watchers of every active wormhole that includes `rift_id` to refetch.
/// Runs in a spawned task so the sync write path never waits on the database.
fn notify_wormholes(state: &SyncState, rift_id: Uuid) {
    let db = state.db.clone();
    let broadcaster = state.broadcaster.clone();
    tokio::spawn(async move {
        match db.active_wormholes_for_rift(rift_id).await {
            Ok(ids) => {
                for wormhole_id in ids {
                    let _ = broadcaster.send((
                        format!("wormhole_{}", wormhole_id),
                        SyncMessage::WormholeUpdated { wormhole_id },
                    ));
                }
            }
            Err(e) => warn!("Failed to look up wormholes for rift {}: {}", rift_id, e),
        }
    });
}

impl SyncState {
    pub fn new(db: Database, storage: Arc<StorageEngine>, chat: ChatSettings) -> Self {
        let (broadcaster, _) = broadcast::channel(1000);
        let sync_state = Self {
            db,
            storage,
            broadcaster,
            batching_state: Arc::new(RwLock::new(BatchingState::default())),
            stream_manager: Arc::new(Mutex::new(StreamManager::default())),
            recent_change_timestamps: Arc::new(RwLock::new(HashMap::new())),
            chat,
            presence: Arc::new(RwLock::new(HashMap::new())),
        };
        
        // PERFORMANCE FIX: Start background batch flusher
        Self::start_batch_flusher(sync_state.clone());
        
        sync_state
    }
    
    /// PERFORMANCE FIX: Background task to flush batched changes
    fn start_batch_flusher(state: SyncState) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                if let Err(e) = Self::flush_expired_batches(&state).await {
                    error!("Error flushing batches: {}", e);
                }
            }
        });
    }
    
    /// PERFORMANCE FIX: Flush batches that have exceeded timeout
    async fn flush_expired_batches(state: &SyncState) -> Result<()> {
        let now = Instant::now();
        let mut to_flush = Vec::new();
        
        {
            let mut batching = state.batching_state.write().await;
            // Collect expired rift IDs first to avoid borrowing conflicts
            let expired_rifts: Vec<String> = batching.last_batch_time
                .iter()
                .filter_map(|(rift_id, last_time)| {
                    if now.duration_since(*last_time) >= BATCH_TIMEOUT {
                        Some(rift_id.clone())
                    } else {
                        None
                    }
                })
                .collect();
            
            // Now remove the expired batches
            for rift_id in expired_rifts {
            let pending = batching.pending_author.remove(&rift_id);
            if let Some(changes) = batching.pending_changes.remove(&rift_id) {
                if !changes.is_empty() {
                    if let Some((author, origin)) = pending {
                        to_flush.push((rift_id.clone(), author, origin, changes));
                    }
                }
            }
            batching.last_batch_time.remove(&rift_id);
        }
        }
        
        // Send batched updates
        for (rift_id, author, origin, changes) in to_flush {
        debug!("Flushing batch for rift {} ({} changes)", rift_id, changes.len());
        Self::send_diff_batch(state, &rift_id, author, origin, changes).await?;
        }
        
        Ok(())
    }

    /// Per-rift change-rate monitor. Returns true when the rate is high.
    /// Callers must still process the change: dropping edits loses work.
    pub async fn should_throttle(&self, rift_id: &str) -> bool {
        const WINDOW_SECS: u64 = 5;
        const MAX_EVENTS: usize = 8;

        let now = Instant::now();
        let window = std::time::Duration::from_secs(WINDOW_SECS);

        let mut map = self.recent_change_timestamps.write().await;
        let entry = map.entry(rift_id.to_string()).or_insert_with(Vec::new);

        // add current event
        entry.push(now);

        // drop events outside the window
        entry.retain(|&t| now.duration_since(t) <= window);

        if entry.len() > MAX_EVENTS {
            // keep vector but signal throttling
            tracing::warn!(%rift_id, events = entry.len(), "Throttling excessive changes for rift");
            return true;
        }

        false
    }
    
    /// PERFORMANCE FIX: Send batched diff changes with compression
    async fn send_diff_batch(state: &SyncState, rift_id: &str, author: Uuid, origin: String, changes: Vec<FileDiffChange>) -> Result<()> {
        let should_compress = changes.len() > 5; // Compress if more than 5 changes
        
        let response = SyncMessage::RiftDiffUpdate {
            rift_id: rift_id.parse()?,
            diff_changes: changes,
            author,
            timestamp: chrono::Utc::now(),
            compressed: should_compress,
            origin: Some(origin),
        };
        
        let channel = format!("rift_{}", rift_id);
        let _ = state.broadcaster.send((channel.clone(), response));
        
        info!("Sent diff batch to rift channel: {} (compressed: {})", channel, should_compress);
        Ok(())
    }
    
    /// Assemble a complete file from received stream chunks
    async fn assemble_stream_file(metadata: &StreamMetadata) -> Result<PathBuf> {
        use std::io::Write;
        
        // Create output file
        let mut file = std::fs::File::create(&metadata.temp_file_path)
            .map_err(|e| anyhow::anyhow!("Failed to create temp file: {}", e))?;
        
        // Write chunks in order
        for chunk_index in 0..metadata.expected_chunks {
            if let Some(chunk_data) = metadata.received_chunks.get(&chunk_index) {
                file.write_all(chunk_data)
                    .map_err(|e| anyhow::anyhow!("Failed to write chunk {}: {}", chunk_index, e))?;
            } else {
                return Err(anyhow::anyhow!("Missing chunk {}", chunk_index));
            }
        }
        
        file.flush()
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;
        
        // Verify file size
        let file_size = std::fs::metadata(&metadata.temp_file_path)
            .map_err(|e| anyhow::anyhow!("Failed to get file metadata: {}", e))?
            .len();
            
        if file_size != metadata.total_size {
            return Err(anyhow::anyhow!("File size mismatch: expected {}, got {}", 
                metadata.total_size, file_size));
        }
        
        // TODO: Verify content hash if provided
        if !metadata.content_hash.is_empty() {
            // This would require reading the entire file to compute hash
            // For now, we'll skip this optimization
        }
        
        info!("File assembled successfully: {} bytes", file_size);
        Ok(metadata.temp_file_path.clone())
    }
}

pub async fn handle_websocket(socket: WebSocket, state: SyncState, rift_id: String, user_id: Uuid, username: String) {
    let (sender, mut receiver) = socket.split();
    let mut broadcast_receiver = state.broadcaster.subscribe();

    // SECURITY FIX: Define the specific rift channel this client should listen to
    let my_rift_channel = format!("rift_{}", rift_id);
    // Per-connection private channel so one-off replies can be targeted at this socket
    let connection_channel = format!("conn_{}", Uuid::new_v4());

    info!("\u{1F512} WebSocket client restricted to channel: {} (private: {})", my_rift_channel, connection_channel);

    // Spawn task to handle broadcasting to this client (listens to rift channel and private conn channel)
    let sender_task = {
        let mut sender = sender;
        let my_channel = my_rift_channel.clone();
        let my_conn = connection_channel.clone();
        tokio::spawn(async move {
            let mut consecutive_errors = 0;
            while let Ok((channel, message)) = broadcast_receiver.recv().await {
                // Only process messages for THIS rift or this private connection channel
                if channel != my_channel && channel != my_conn {
                    continue;
                }

                // Never echo a connection's own changes back to it
                if let SyncMessage::RiftDiffUpdate { origin: Some(ref origin), .. } = message {
                    if origin == &my_conn {
                        continue;
                    }
                }

                let json = match serde_json::to_string(&message) {
                    Ok(json) => json,
                    Err(e) => {
                        error!("Failed to serialize message for channel {}: {}", channel, e);
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            error!("Too many consecutive serialization errors, closing connection");
                            break;
                        }
                        continue;
                    }
                };

                match sender.send(Message::Text(json)).await {
                    Ok(_) => {
                        consecutive_errors = 0; // Reset on success
                        info!("\u{2705} Message sent to client on channel: {}", channel);

                        // Small delay after sending to prevent overwhelming client
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(e) => {
                        error!("Failed to send message to client on channel {}: {}", channel, e);
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            error!("Too many consecutive send errors, closing connection");
                            break;
                        }
                        // Add small delay before retry
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }
            info!("Broadcast receiver task completed for channel: {}", my_channel);
        })
    };

    // Handle incoming messages
    let mut consecutive_errors = 0;
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // Try to parse JoinRift locally so we can reply ONLY to this connection (not broadcast to whole rift)
                if let Ok(sync_msg) = serde_json::from_str::<SyncMessage>(&text) {
                    match sync_msg {
                        SyncMessage::JoinRift { rift_id: msg_rift_id, last_checkpoint } => {
                            info!("Client joining rift: {} (last checkpoint: {:?})", msg_rift_id, last_checkpoint);

                            // SECURITY CHECK: Verify client is authorized for this rift
                            let msg_rift_id_str = msg_rift_id.to_string();
                            if msg_rift_id_str != rift_id {
                                error!("\u{1F6A8} SECURITY: Client attempted to join unauthorized rift {} (authorized: {})", msg_rift_id_str, rift_id);
                                let _ = state.broadcaster.send((connection_channel.clone(), SyncMessage::Error { message: "Unauthorized rift access attempt".to_string(), error_code: None }));
                                continue;
                            }

                            // Get current live state for the rift
                            let live_files = match state.storage.get_live_state(msg_rift_id).await {
                                Ok(files) => {
                                    info!("\u{2705} Retrieved {} files from rift {}", files.len(), msg_rift_id);
                                    files
                                }
                                Err(e) => {
                                    error!("\u{274C} Failed to get live state for rift {}: {}", msg_rift_id, e);
                                    let _ = state.broadcaster.send((connection_channel.clone(), SyncMessage::Error { message: format!("Failed to get live state: {}", e), error_code: None }));
                                    continue;
                                }
                            };

                            // Register this connection as present in the rift
                            let participants = {
                                let mut presence = state.presence.write().await;
                                let connections = presence.entry(msg_rift_id).or_default();
                                connections.insert(connection_channel.clone(), username.clone());
                                events::record(&state.db, msg_rift_id, Some(user_id), events::JOIN, serde_json::json!({}));
                                collect_participants(connections)
                            };

                            let response = SyncMessage::RiftJoined {
                                rift_id: msg_rift_id,
                                current_files: live_files,
                                participants,
                                last_checkpoint,
                            };

                            // Send ONLY to this client's private channel so other rift members don't get this one-off message
                            match state.broadcaster.send((connection_channel.clone(), response)) {
                                Ok(_) => {
                                    info!("\u{2705} RiftJoined message sent to private channel: {}", connection_channel);
                                }
                                Err(e) => {
                                    error!("\u{274C} Failed to send RiftJoined message to private channel {}: {}", connection_channel, e);
                                }
                            }

                            // Don't call general handler for JoinRift
                            continue;
                        }
                        _ => {
                            // fallthrough to general handler below
                        }
                    }
                }

                match handle_sync_message(&text, &state, &rift_id, user_id, &connection_channel).await {
                    Ok(_) => {
                        consecutive_errors = 0; // Reset on success
                    }
                    Err(e) => {
                        error!("Error handling sync message: {}", e);
                        // Tell the sender its change was rejected so it can resync
                        let _ = state.broadcaster.send((connection_channel.clone(), SyncMessage::Error {
                            message: format!("Change rejected by server: {}", e),
                            error_code: Some("CHANGE_REJECTED".to_string()),
                        }));
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            error!("Too many consecutive message handling errors, closing connection");
                            break;
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket connection closed gracefully");
                break;
            }
            Ok(Message::Ping(_)) => {
                // Reset error counter on successful ping
                consecutive_errors = 0;
                // Note: We can't send pong directly since sender is in another task
                // The WebSocket protocol should handle this automatically
            }
            Ok(Message::Pong(_)) => {
                // Reset error counter on successful pong
                consecutive_errors = 0;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                consecutive_errors += 1;
                if consecutive_errors >= 3 {
                    error!("Too many consecutive WebSocket errors, closing connection");
                    break;
                }
                // Add small delay before continuing
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            _ => {
                // Ignore other message types
            }
        }
    }

    info!("WebSocket connection closed for rift: {}", rift_id);
    if remove_presence(&state, &rift_id, &connection_channel).await {
        if let Ok(rift_uuid) = Uuid::parse_str(&rift_id) {
            events::record(&state.db, rift_uuid, Some(user_id), events::LEAVE, serde_json::json!({}));
        }
    }
    sender_task.abort();
}

/// Drop a closed connection from the rift's presence map. Returns true if it had joined the rift.
async fn remove_presence(state: &SyncState, rift_id: &str, connection_channel: &str) -> bool {
    let Ok(rift_uuid) = Uuid::parse_str(rift_id) else { return false };
    let mut presence = state.presence.write().await;
    let mut was_present = false;
    if let Some(connections) = presence.get_mut(&rift_uuid) {
        was_present = connections.remove(connection_channel).is_some();
        if connections.is_empty() {
            presence.remove(&rift_uuid);
        }
    }
    was_present
}

/// True when a client's edit was made against content other than the current main.
/// Clients that send no base hash get the legacy behavior (no check).
fn is_stale_base(base_hash: Option<&str>, original_content: &str) -> bool {
    match base_hash {
        Some(base) => base != mothership_common::content_hash(original_content),
        None => false,
    }
}

#[cfg(test)]
mod stale_base_tests {
    use super::is_stale_base;
    use mothership_common::content_hash;

    #[test]
    fn no_base_hash_is_never_stale() {
        assert!(!is_stale_base(None, "anything"));
    }

    #[test]
    fn matching_base_is_not_stale() {
        let main = "fn main() {}\n";
        assert!(!is_stale_base(Some(&content_hash(main)), main));
    }

    #[test]
    fn mismatched_base_is_stale() {
        let main = "fn main() {}\n";
        let old = "fn main() { }\n";
        assert!(is_stale_base(Some(&content_hash(old)), main));
    }
}

/// Stale-base edit: main stays unchanged, the client's version goes into a conflict rift
/// owned by the editing user, and the client is told to resync main.
async fn create_conflict_rift_for_stale_edit(
    state: &SyncState,
    original_rift_id: Uuid,
    user_id: Uuid,
    connection_channel: &str,
    path: &PathBuf,
    client_content: String,
    main_content: String,
) -> Result<()> {
    let original_rift = state.db.get_rift(original_rift_id).await?
        .ok_or_else(|| anyhow::anyhow!("Rift not found: {}", original_rift_id))?;
    let username = state.db.get_user(user_id).await?
        .map(|u| u.username)
        .unwrap_or_else(|| user_id.to_string());

    let base_name = format!("{}{}-{}", mothership_common::CONFLICT_RIFT_PREFIX, username, path.display());
    // A repeat conflict on the same file gets a suffix so the name stays unique in the project
    let conflict_name = if state.db.rift_name_exists(original_rift.project_id, &base_name).await? {
        format!("{}-{}", base_name, &Uuid::new_v4().to_string()[..8])
    } else {
        base_name
    };
    let conflict_rift = state.db
        .create_rift(original_rift.project_id, user_id, Some(conflict_name.clone()))
        .await?;
    state.storage.update_live_state(conflict_rift.id, path.clone(), client_content).await?;

    let _ = state.broadcaster.send((connection_channel.to_string(), SyncMessage::ConflictRiftCreated {
        original_rift_id,
        new_rift_id: conflict_rift.id,
        conflict_rift_name: conflict_name,
    }));
    let _ = state.broadcaster.send((connection_channel.to_string(), SyncMessage::ForceSync {
        path: path.clone(),
        server_content: main_content,
        server_timestamp: chrono::Utc::now(),
    }));
    info!("Created conflict rift {} for {} (original rift {})", conflict_rift.id, path.display(), original_rift_id);
    Ok(())
}

async fn handle_sync_message(message: &str, state: &SyncState, client_rift_id: &str, user_id: Uuid, connection_channel: &str) -> Result<()> {
    let sync_message: SyncMessage = serde_json::from_str(message)?;
    
    match sync_message {
        SyncMessage::SendChatMessage { rift_id: msg_rift_id, content } => {
            // SECURITY CHECK: Chat is scoped to the rift this socket was authorized for
            if msg_rift_id.to_string() != client_rift_id {
                error!("SECURITY: Client attempted to chat in unauthorized rift {} (authorized: {})", msg_rift_id, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized rift access attempt"));
            }
            chat::post_chat_message(&state.db, &state.broadcaster, &state.chat, msg_rift_id, user_id, &content)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        SyncMessage::JoinRift { rift_id: msg_rift_id, last_checkpoint } => {
            info!("Client joining rift: {} (last checkpoint: {:?})", msg_rift_id, last_checkpoint);
            
            // SECURITY CHECK: Verify client is authorized for this rift
            let msg_rift_id_str = msg_rift_id.to_string();
            if msg_rift_id_str != client_rift_id {
                error!("SECURITY: Client attempted to join unauthorized rift {} (authorized: {})", msg_rift_id_str, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized rift access attempt"));
            }
            
            // Get current live state for the rift
            let live_files = match state.storage.get_live_state(msg_rift_id).await {
                Ok(files) => {
                    info!("Retrieved {} files from rift {}", files.len(), msg_rift_id);
                    files
                }
                Err(e) => {
                    error!("Failed to get live state for rift {}: {}", msg_rift_id, e);
                    return Err(e);
                }
            };

            let response = SyncMessage::RiftJoined {
                rift_id: msg_rift_id,
                current_files: live_files,
                participants: rift_participants(state, msg_rift_id).await,
                last_checkpoint,
            };
            
            // Test serialization before sending
            match serde_json::to_string(&response) {
                Ok(json) => {
                    info!("RiftJoined message serialized successfully ({} bytes)", json.len());
                    
                    // Send only to the joining client (not broadcast to all)
                    let channel = format!("rift_{}", msg_rift_id);
                    match state.broadcaster.send((channel.clone(), response)) {
                        Ok(_) => {
                            info!("RiftJoined message sent to channel: {}", channel);
                        }
                        Err(e) => {
                            error!("Failed to send RiftJoined message to channel {}: {}", channel, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to serialize RiftJoined message: {}", e);
                    return Err(anyhow::anyhow!("Serialization failed: {}", e));
                }
            }
        }

        SyncMessage::FileChanged { rift_id: msg_rift_id, path, content, timestamp: _, base_hash } => {
            // Rate-limit noisy clients per rift
            if state.should_throttle(&msg_rift_id.to_string()).await {
                warn!("High change rate for rift {}; processing FileChanged anyway", msg_rift_id);
            }
            // SECURITY CHECK: Verify client is authorized for this rift
            let msg_rift_id_str = msg_rift_id.to_string();
            if msg_rift_id_str != client_rift_id {
                error!("SECURITY: Client attempted to modify unauthorized rift {} (authorized: {})", msg_rift_id_str, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized rift modification attempt"));
            }
            
            info!("File changed in rift {}: {} ({} bytes)", msg_rift_id, path.display(), content.len());
            
            // PERFORMANCE FIX: Get original content to generate diff
            let original_content = match state.storage.get_file_content(msg_rift_id, &path).await {
                Ok(content) => content,
                Err(_) => String::new(), // New file
            };

            // Stale base: the client edited against content that is no longer main.
            // Keep main as-is and put the client's version in a conflict rift.
            if is_stale_base(base_hash.as_deref(), &original_content) {
                warn!("Stale base for {} in rift {}; diverting edit to conflict rift", path.display(), msg_rift_id);
                return create_conflict_rift_for_stale_edit(
                    state, msg_rift_id, user_id, connection_channel, &path, content, original_content,
                ).await;
            }
            
            // Update live working state
            state.storage.update_live_state(msg_rift_id, path.clone(), content.clone()).await?;
            notify_wormholes(&state, msg_rift_id);

            let diff_engine = DiffEngine::new();
            let diff = diff_engine.generate_line_diff(&original_content, &content);
            let diff_change = FileDiffChange {
                path: path.clone(),
                diff,
                file_size: content.len() as u64,
            };
            
            info!("Generated diff for {}: original {} bytes -> new {} bytes", 
                path.display(), original_content.len(), content.len());
            
            // PERFORMANCE FIX: Add to batch instead of immediate broadcast
            handle_diff_change_batched(state, msg_rift_id, user_id, connection_channel, diff_change).await?;
            
            // TODO: Implement smart checkpointing
            // Check if we should create automatic checkpoint (every N changes or time-based)
            // if should_create_auto_checkpoint(msg_rift_id, &state).await? {
            //     let checkpoint = state.storage.create_checkpoint(...).await?;
            //     // Broadcast checkpoint creation
            // }
        }

        SyncMessage::FileDiffChanged { rift_id: msg_rift_id, path, diff, file_size, timestamp: _, base_hash } => {
            // Rate-limit noisy clients per rift
            if state.should_throttle(&msg_rift_id.to_string()).await {
                warn!("High change rate for rift {}; processing FileDiffChanged anyway", msg_rift_id);
            }
            // SECURITY CHECK: Verify client is authorized for this rift
            let msg_rift_id_str = msg_rift_id.to_string();
            if msg_rift_id_str != client_rift_id {
                error!("SECURITY: Client attempted to modify unauthorized rift {} (authorized: {})", msg_rift_id_str, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized rift modification attempt"));
            }
            
            info!("Diff change in rift {}: {} ({} bytes)", msg_rift_id, path.display(), file_size);
            
            // PERFORMANCE FIX: Apply diff to get new content
            let original_content = match state.storage.get_file_content(msg_rift_id, &path).await {
                Ok(content) => content,
                Err(_) => String::new(), // New file
            };

            // A diff only applies cleanly to the base it was made against. On a stale base,
            // the server cannot rebuild the client's content from the diff, so it asks the
            // client for the full content (see FullContentRequired below).
            if is_stale_base(base_hash.as_deref(), &original_content) {
                // Do not ForceSync here: that overwrites the client's edit. Ask for the full
                // content instead, so the FileChanged path can divert the edit to a conflict rift.
                warn!("Stale base for diff on {} in rift {}; requesting full content", path.display(), msg_rift_id);
                let _ = state.broadcaster.send((connection_channel.to_string(), SyncMessage::FullContentRequired {
                    rift_id: msg_rift_id,
                    path: path.clone(),
                    base_hash,
                }));
                return Ok(());
            }
            
            let diff_engine = DiffEngine::new();
            let new_content = diff_engine.apply_diff(&original_content, &diff)?;
            
            // Update live working state
            state.storage.update_live_state(msg_rift_id, path.clone(), new_content).await?;
            notify_wormholes(&state, msg_rift_id);
            
            // PERFORMANCE FIX: Batch the diff change
            let diff_change = FileDiffChange { path, diff, file_size };
            handle_diff_change_batched(state, msg_rift_id, user_id, connection_channel, diff_change).await?;
            
            info!("Applied diff successfully: {} bytes", file_size);
        }

        SyncMessage::BatchDiffChanges { rift_id: msg_rift_id, changes, timestamp: _, compressed } => {
            // Rate-limit noisy clients per rift
            if state.should_throttle(&msg_rift_id.to_string()).await {
                warn!("High change rate for rift {}; processing BatchDiffChanges anyway", msg_rift_id);
            }
            // SECURITY CHECK: Verify client is authorized for this rift
            let msg_rift_id_str = msg_rift_id.to_string();
            if msg_rift_id_str != client_rift_id {
                error!("SECURITY: Client attempted to modify unauthorized rift {} (authorized: {})", msg_rift_id_str, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized rift modification attempt"));
            }
            
            info!("Batch diff changes in rift {}: {} changes (compressed: {})", 
                msg_rift_id, changes.len(), compressed);
            
            // Clone changes before processing to avoid move issues
            let changes_for_response = changes.clone();
            
            // Process each change in the batch
            for change in changes {
                // Apply diff to get new content
                let original_content = match state.storage.get_file_content(msg_rift_id, &change.path).await {
                    Ok(content) => content,
                    Err(_) => String::new(), // New file
                };
                
                let diff_engine = DiffEngine::new();
                let new_content = diff_engine.apply_diff(&original_content, &change.diff)?;
                
                // Update live working state
                state.storage.update_live_state(msg_rift_id, change.path.clone(), new_content).await?;
                notify_wormholes(&state, msg_rift_id);
            }
            
            // PERFORMANCE FIX: Forward the batch to other collaborators
            let response = SyncMessage::RiftDiffUpdate {
                rift_id: msg_rift_id,
                diff_changes: changes_for_response,
                author: user_id,
                timestamp: chrono::Utc::now(),
                compressed,
                origin: Some(connection_channel.to_string()),
            };
            
            let channel = format!("rift_{}", msg_rift_id);
            let _ = state.broadcaster.send((channel.clone(), response));
            
            info!("Forwarded diff batch to rift channel: {}", channel);
        }

        SyncMessage::CreateCheckpoint { rift_id: msg_rift_id, message } => {
            // SECURITY CHECK: Verify client is authorized for this rift
            let msg_rift_id_str = msg_rift_id.to_string();
            if msg_rift_id_str != client_rift_id {
                error!("SECURITY: Client attempted to create checkpoint in unauthorized rift {} (authorized: {})", msg_rift_id_str, client_rift_id);
                return Err(anyhow::anyhow!("Unauthorized checkpoint creation attempt"));
            }
            
            info!("Checkpoint requested for rift: {} (message: {:?})", msg_rift_id, message);
            
            // Create actual checkpoint using storage engine
            let checkpoint = state.storage.create_checkpoint(
                msg_rift_id,
                user_id,
                message.clone(),
                false, // Manual checkpoint
            ).await?;
            
            let response = SyncMessage::CheckpointCreated {
                rift_id: msg_rift_id,
                checkpoint_id: checkpoint.id,
                author: checkpoint.author,
                timestamp: checkpoint.timestamp,
                message,
            };
            
            let channel = format!("rift_{}", msg_rift_id);
            let _ = state.broadcaster.send((channel, response));
        }

        SyncMessage::Heartbeat => {
            // Heartbeat messages are just for connection keepalive - no action needed
            debug!("Received heartbeat from client");
        }

        // Streaming support
        SyncMessage::StartFileStream { stream_id, rift_id, path, total_size, chunk_size, content_hash, is_binary, timestamp: _, base_hash } => {
            info!("Starting file stream {} for {}: {} bytes", stream_id, path.display(), total_size);
            
            let mut stream_manager = state.stream_manager.lock().await;
            let metadata = StreamMetadata::new(stream_id, rift_id, path.clone(), total_size, chunk_size, is_binary, content_hash, base_hash);
            stream_manager.active_streams.insert(stream_id, metadata);
            
            info!("Stream {} registered for file {}", stream_id, path.display());
        }

        SyncMessage::FileChunk { stream_id, chunk_index, data, is_compressed, chunk_hash } => {
            debug!("Received chunk {} for stream {}", chunk_index, stream_id);
            
            let mut stream_manager = state.stream_manager.lock().await;
            if let Some(metadata) = stream_manager.active_streams.get_mut(&stream_id) {
                // Verify chunk hash if provided
                if !chunk_hash.is_empty() {
                    let computed_hash = format!("{:x}", Sha256::digest(&data));
                    if computed_hash != chunk_hash {
                        error!("Chunk hash mismatch for stream {} chunk {}", stream_id, chunk_index);
                        // Send error back to client
                        let error_msg = SyncMessage::StreamError {
                            stream_id,
                            error_code: mothership_common::protocol::StreamErrorCode::ChunkCorrupted,
                            message: format!("Chunk {} hash mismatch", chunk_index),
                            retry_chunk: Some(chunk_index),
                        };
                        let channel = format!("rift_{}", metadata.rift_id);
                        let _ = state.broadcaster.send((channel, error_msg));
                        return Ok(());
                    }
                }

                // Decompress chunk if needed
                let chunk_data = if is_compressed {
                    use std::io::Read;
                    let mut decoder = flate2::read::DeflateDecoder::new(&data[..]);
                    let mut decompressed = Vec::new();
                    match decoder.read_to_end(&mut decompressed) {
                        Ok(_) => decompressed,
                        Err(e) => {
                            error!("Failed to decompress chunk {} for stream {}: {}", chunk_index, stream_id, e);
                            let error_msg = SyncMessage::StreamError {
                                stream_id,
                                error_code: mothership_common::protocol::StreamErrorCode::ChunkCorrupted,
                                message: format!("Decompression failed: {}", e),
                                retry_chunk: Some(chunk_index),
                            };
                            let channel = format!("rift_{}", metadata.rift_id);
                            let _ = state.broadcaster.send((channel, error_msg));
                            return Ok(());
                        }
                    }
                } else {
                    data
                };

                // Store chunk
                metadata.received_chunks.insert(chunk_index, chunk_data);
                
                debug!("Chunk {} received for stream {} ({}/{} chunks)", 
                    chunk_index, stream_id, metadata.received_chunks.len(), metadata.expected_chunks);

                // Check if stream is complete
                if metadata.is_complete() {
                    info!("Stream {} complete, assembling file {}", stream_id, metadata.file_path.display());
                    
                    // Assemble file from chunks
                    match SyncState::assemble_stream_file(metadata).await {
                        Ok(final_path) => {
                            info!("File assembled successfully: {}", final_path.display());
                            
                            // Read assembled file content and save to storage
                            match tokio::fs::read_to_string(&final_path).await {
                                Ok(content) => {
                                    let rift_id = metadata.rift_id;
                                    let file_path = metadata.file_path.clone();
                                    let original = state.storage.get_file_content(rift_id, &file_path).await.unwrap_or_default();
                                    if is_stale_base(metadata.base_hash.as_deref(), &original) {
                                        warn!("Stale base for streamed {} in rift {}; diverting to conflict rift", file_path.display(), rift_id);
                                        if let Err(e) = create_conflict_rift_for_stale_edit(
                                            state, rift_id, user_id, connection_channel, &file_path, content, original,
                                        ).await {
                                            error!("Failed to create conflict rift for streamed file: {}", e);
                                        }
                                    } else if let Err(e) = state.storage.update_live_state(metadata.rift_id, metadata.file_path.clone(), content.clone()).await {
                                        error!("Failed to save streamed file to storage: {}", e);
                                    } else {
                                        notify_wormholes(&state, metadata.rift_id);
                                        info!("Streamed file saved to storage: {}", metadata.file_path.display());
                                        
                                        // Broadcast file change to other collaborators
                                        let response = SyncMessage::FileUpdate {
                                            rift_id: metadata.rift_id,
                                            path: metadata.file_path.clone(),
                                            content,
                                            author: user_id,
                                            timestamp: chrono::Utc::now(),
                                        };
                                        
                                        let channel = format!("rift_{}", metadata.rift_id);
                                        let _ = state.broadcaster.send((channel, response));
                                    }
                                    
                                    // Clean up temp file
                                    let _ = tokio::fs::remove_file(&final_path).await;
                                }
                                Err(e) => {
                                    error!("Failed to read assembled file: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to assemble stream {}: {}", stream_id, e);
                            let error_msg = SyncMessage::StreamError {
                                stream_id,
                                error_code: mothership_common::protocol::StreamErrorCode::StorageFull,
                                message: format!("Assembly failed: {}", e),
                                retry_chunk: None,
                            };
                            let channel = format!("rift_{}", metadata.rift_id);
                            let _ = state.broadcaster.send((channel, error_msg));
                        }
                    }
                    
                    // Remove completed stream
                    stream_manager.active_streams.remove(&stream_id);
                }
            } else {
                warn!(" Received chunk for unknown stream {}", stream_id);
                // We don't know the rift_id, so this error might not be delivered properly
                // This is a design limitation - we might need to track stream_id -> rift_id mapping
            }
        }

        SyncMessage::EndFileStream { stream_id, final_hash, success } => {
            info!("Stream {} ended (success: {}, hash: {})", stream_id, success, final_hash);
            
            let mut stream_manager = state.stream_manager.lock().await;
            if let Some(metadata) = stream_manager.active_streams.remove(&stream_id) {
                if !success {
                    info!(" Stream {} failed, cleaning up", stream_id);
                    let _ = tokio::fs::remove_file(&metadata.temp_file_path).await;
                } else {
                    // Verify final hash if provided
                    if !final_hash.is_empty() {
                        // TODO: Verify against assembled file hash
                    }
                    info!("Stream {} completed successfully", stream_id);
                }
            }
        }

        SyncMessage::StreamControl { stream_id, operation } => {
            info!("Stream control for {}: {:?}", stream_id, operation);
            // TODO: Implement stream control operations (pause, resume, cancel)
        }

        SyncMessage::StreamError { stream_id, error_code, message, retry_chunk } => {
            error!("Stream error for {}: {:?} - {} (retry chunk: {:?})", 
                stream_id, error_code, message, retry_chunk);
            // TODO: Implement error handling and retry logic
        }

        _ => {
            warn!(" Unhandled sync message type");
        }
    }

    Ok(())
}

/// PERFORMANCE FIX: Add diff change to batch (with immediate flush if batch is full)
/// A batch carries a single author, so a change from a different user flushes the pending batch first.
async fn handle_diff_change_batched(
    state: &SyncState, 
    rift_id: uuid::Uuid, 
    author: Uuid,
    origin: &str,
    diff_change: FileDiffChange
) -> Result<()> {
    events::record(
        &state.db,
        rift_id,
        Some(author),
        events::FILE_SAVED,
        serde_json::json!({
            "path": diff_change.path.to_string_lossy(),
            "file_size": diff_change.file_size,
        }),
    );

    let rift_id_str = rift_id.to_string();
    let now = Instant::now();
    let mut to_flush: Option<(Uuid, String, Vec<FileDiffChange>)> = None;
    
    {
        let mut batching = state.batching_state.write().await;

        // Author or connection changed: take the previous batch out so it is sent under its own origin
        if batching.pending_author.get(&rift_id_str).is_some_and(|(prev, prev_origin)| *prev != author || prev_origin != origin) {
            let prev = batching.pending_author.remove(&rift_id_str);
            let prev_changes = batching.pending_changes.remove(&rift_id_str).unwrap_or_default();
            if let (Some((prev_author, prev_origin)), false) = (prev, prev_changes.is_empty()) {
                to_flush = Some((prev_author, prev_origin, prev_changes));
            }
        }

        batching.pending_author.insert(rift_id_str.clone(), (author, origin.to_string()));
        
        // Add to pending changes
        let changes = batching.pending_changes.entry(rift_id_str.clone()).or_insert_with(Vec::new);
        changes.push(diff_change);
        let changes_len = changes.len();
        
        // Update last batch time
        batching.last_batch_time.insert(rift_id_str.clone(), now);
        
        // Flush immediately if batch is full
        if changes_len >= MAX_BATCH_SIZE && to_flush.is_none() {
            let full = batching.pending_changes.remove(&rift_id_str).unwrap_or_default();
            batching.pending_author.remove(&rift_id_str);
            batching.last_batch_time.remove(&rift_id_str);
            to_flush = Some((author, origin.to_string(), full));
        }
    }
    
    if let Some((batch_author, batch_origin, changes)) = to_flush {
        if !changes.is_empty() {
            debug!("Flushing batch for rift {} ({} changes)", rift_id_str, changes.len());
            SyncState::send_diff_batch(state, &rift_id_str, batch_author, batch_origin, changes).await?;
        }
    }
    
    Ok(())
} 