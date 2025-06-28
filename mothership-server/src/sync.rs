use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use mothership_common::protocol::{SyncMessage, FileDiffChange};
use mothership_common::diff::DiffEngine;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn, debug};
use uuid::Uuid;

use crate::database::Database;
use crate::storage::StorageEngine;

/// PERFORMANCE FIX: Batching state for reducing message overhead
#[derive(Default)]
struct BatchingState {
    pending_changes: HashMap<String, Vec<FileDiffChange>>, // rift_id -> changes
    last_batch_time: HashMap<String, Instant>, // rift_id -> time
}

const BATCH_TIMEOUT: Duration = Duration::from_millis(100); // 100ms batching window
const MAX_BATCH_SIZE: usize = 50; // Maximum changes per batch

#[derive(Clone)]
pub struct SyncState {
    pub db: Database,
    pub storage: Arc<StorageEngine>,
    pub broadcaster: broadcast::Sender<(String, SyncMessage)>,
    pub batching_state: Arc<RwLock<BatchingState>>, // PERFORMANCE FIX: Batching support
}

impl SyncState {
    pub fn new(db: Database, storage: Arc<StorageEngine>) -> Self {
        let (broadcaster, _) = broadcast::channel(1000);
        let sync_state = Self {
            db,
            storage,
            broadcaster,
            batching_state: Arc::new(RwLock::new(BatchingState::default())),
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
                if let Some(changes) = batching.pending_changes.remove(&rift_id) {
                    if !changes.is_empty() {
                        to_flush.push((rift_id.clone(), changes));
                    }
                }
                batching.last_batch_time.remove(&rift_id);
            }
        }
        
        // Send batched updates
        for (rift_id, changes) in to_flush {
            debug!("🚀 Flushing batch for rift {} ({} changes)", rift_id, changes.len());
            Self::send_diff_batch(state, &rift_id, changes).await?;
        }
        
        Ok(())
    }
    
    /// PERFORMANCE FIX: Send batched diff changes with compression
    async fn send_diff_batch(state: &SyncState, rift_id: &str, changes: Vec<FileDiffChange>) -> Result<()> {
        let should_compress = changes.len() > 5; // Compress if more than 5 changes
        
        let response = SyncMessage::RiftDiffUpdate {
            rift_id: rift_id.parse()?,
            diff_changes: changes,
            author: Uuid::new_v4(), // TODO: Get actual user ID
            timestamp: chrono::Utc::now(),
            compressed: should_compress,
        };
        
        let channel = format!("rift_{}", rift_id);
        let _ = state.broadcaster.send((channel.clone(), response));
        
        info!("📤 Sent diff batch to rift channel: {} (compressed: {})", channel, should_compress);
        Ok(())
    }
}

pub async fn handle_websocket(socket: WebSocket, state: SyncState) {
    let (sender, mut receiver) = socket.split();
    let mut broadcast_receiver = state.broadcaster.subscribe();

    // Spawn task to handle broadcasting to this client
    let sender_task = {
        let mut sender = sender;
        tokio::spawn(async move {
            while let Ok((_channel, message)) = broadcast_receiver.recv().await {
                let json = match serde_json::to_string(&message) {
                    Ok(json) => json,
                    Err(e) => {
                        error!("Failed to serialize message: {}", e);
                        continue;
                    }
                };
                
                if let Err(e) = sender.send(Message::Text(json)).await {
                    error!("Failed to send message to client: {}", e);
                    break;
                }
            }
        })
    };

    // Handle incoming messages
    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                if let Err(e) = handle_sync_message(&text, &state).await {
                    error!("Error handling sync message: {}", e);
                }
            }
            Message::Close(_) => {
                info!("WebSocket connection closed");
                break;
            }
            _ => {
                // Ignore other message types
            }
        }
    }

    sender_task.abort();
}

async fn handle_sync_message(message: &str, state: &SyncState) -> Result<()> {
    let sync_message: SyncMessage = serde_json::from_str(message)?;
    
    match sync_message {
        SyncMessage::JoinRift { rift_id: msg_rift_id, last_checkpoint } => {
            info!("Client joining rift: {} (last checkpoint: {:?})", msg_rift_id, last_checkpoint);
            
            // Get current live state for the rift
            let live_files = state.storage.get_live_state(msg_rift_id).await?;
            
            let response = SyncMessage::RiftJoined {
                rift_id: msg_rift_id,
                current_files: live_files,
                participants: vec![], // TODO: Get actual participants
                last_checkpoint,
            };
            
            // Send only to the joining client (not broadcast to all)
            let channel = format!("rift_{}", msg_rift_id);
            let _ = state.broadcaster.send((channel, response));
        }

        SyncMessage::FileChanged { rift_id: msg_rift_id, path, content, timestamp: _ } => {
            info!("📝 File changed in rift {}: {} ({} bytes)", msg_rift_id, path.display(), content.len());
            
            // PERFORMANCE FIX: Get original content to generate diff
            let original_content = match state.storage.get_file_content(msg_rift_id, &path).await {
                Ok(content) => content,
                Err(_) => String::new(), // New file
            };
            
            // Update live working state
            state.storage.update_live_state(msg_rift_id, path.clone(), content.clone()).await?;
            
            // PERFORMANCE FIX: Generate diff instead of sending full content
            let diff_change = DiffEngine::create_diff_change(path.clone(), &original_content, &content);
            
            info!("📊 Generated diff for {}: original {} bytes -> new {} bytes", 
                path.display(), original_content.len(), content.len());
            
            // PERFORMANCE FIX: Add to batch instead of immediate broadcast
            handle_diff_change_batched(state, msg_rift_id, diff_change).await?;
            
            // TODO: Implement smart checkpointing
            // Check if we should create automatic checkpoint (every N changes or time-based)
            // if should_create_auto_checkpoint(msg_rift_id, &state).await? {
            //     let checkpoint = state.storage.create_checkpoint(...).await?;
            //     // Broadcast checkpoint creation
            // }
        }

        SyncMessage::FileDiffChanged { rift_id: msg_rift_id, path, diff, file_size, timestamp: _ } => {
            info!("📝 Diff change in rift {}: {} ({} bytes)", msg_rift_id, path.display(), file_size);
            
            // PERFORMANCE FIX: Apply diff to get new content
            let original_content = match state.storage.get_file_content(msg_rift_id, &path).await {
                Ok(content) => content,
                Err(_) => String::new(), // New file
            };
            
            let new_content = DiffEngine::apply_diff(&original_content, &diff)?;
            
            // Update live working state
            state.storage.update_live_state(msg_rift_id, path.clone(), new_content).await?;
            
            // PERFORMANCE FIX: Batch the diff change
            let diff_change = FileDiffChange { path, diff, file_size };
            handle_diff_change_batched(state, msg_rift_id, diff_change).await?;
            
            info!("✅ Applied diff successfully: {} bytes", file_size);
        }

        SyncMessage::BatchDiffChanges { rift_id: msg_rift_id, changes, timestamp: _, compressed } => {
            info!("📦 Batch diff changes in rift {}: {} changes (compressed: {})", 
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
                
                let new_content = DiffEngine::apply_diff(&original_content, &change.diff)?;
                
                // Update live working state
                state.storage.update_live_state(msg_rift_id, change.path.clone(), new_content).await?;
            }
            
            // PERFORMANCE FIX: Forward the batch to other collaborators
            let response = SyncMessage::RiftDiffUpdate {
                rift_id: msg_rift_id,
                diff_changes: changes_for_response,
                author: Uuid::new_v4(), // TODO: Get actual user ID
                timestamp: chrono::Utc::now(),
                compressed,
            };
            
            let channel = format!("rift_{}", msg_rift_id);
            let _ = state.broadcaster.send((channel.clone(), response));
            
            info!("📤 Forwarded diff batch to rift channel: {}", channel);
        }

        SyncMessage::CreateCheckpoint { rift_id: msg_rift_id, message } => {
            info!("📸 Checkpoint requested for rift: {} (message: {:?})", msg_rift_id, message);
            
            // Create actual checkpoint using storage engine
            let checkpoint = state.storage.create_checkpoint(
                msg_rift_id,
                Uuid::new_v4(), // TODO: Get actual user ID from session
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

        _ => {
            warn!("⚠️  Unhandled sync message type");
        }
    }

    Ok(())
}

/// PERFORMANCE FIX: Add diff change to batch (with immediate flush if batch is full)
async fn handle_diff_change_batched(
    state: &SyncState, 
    rift_id: uuid::Uuid, 
    diff_change: FileDiffChange
) -> Result<()> {
    let rift_id_str = rift_id.to_string();
    let now = Instant::now();
    let mut should_flush = false;
    
    {
        let mut batching = state.batching_state.write().await;
        
        // Add to pending changes
        let changes = batching.pending_changes.entry(rift_id_str.clone()).or_insert_with(Vec::new);
        changes.push(diff_change);
        let changes_len = changes.len();
        
        // Update last batch time
        batching.last_batch_time.insert(rift_id_str.clone(), now);
        
        // Check if we should flush immediately
        if changes_len >= MAX_BATCH_SIZE {
            should_flush = true;
        }
    }
    
    // Flush immediately if batch is full
    if should_flush {
        let changes = {
            let mut batching = state.batching_state.write().await;
            batching.pending_changes.remove(&rift_id_str).unwrap_or_default()
        };
        
        if !changes.is_empty() {
            debug!("🚀 Flushing full batch for rift {} ({} changes)", rift_id_str, changes.len());
            SyncState::send_diff_batch(state, &rift_id_str, changes).await?;
        }
    }
    
    Ok(())
} 