use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use mothership_common::protocol::SyncMessage;
use serde_json;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::database::Database;
use crate::storage::StorageEngine;

#[derive(Clone)]
pub struct SyncState {
    pub db: Database,
    pub storage: Arc<StorageEngine>,
    pub broadcaster: broadcast::Sender<(String, SyncMessage)>,
}

impl SyncState {
    pub fn new(db: Database, storage: Arc<StorageEngine>) -> Self {
        let (broadcaster, _) = broadcast::channel(1000);
        Self {
            db,
            storage,
            broadcaster,
        }
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

        SyncMessage::FileChanged { rift_id: msg_rift_id, path, content, timestamp } => {
            info!("📝 File changed in rift {}: {} ({} bytes)", msg_rift_id, path.display(), content.len());
            
            // Update live working state
            state.storage.update_live_state(msg_rift_id, path.clone(), content.clone()).await?;
            
            // Broadcast file change with actual content to other collaborators
            let response = SyncMessage::FileUpdate {
                rift_id: msg_rift_id,
                path: path.clone(),
                content: content.clone(),
                author: Uuid::new_v4(), // TODO: Get actual user ID from WebSocket session
                timestamp,
            };
            
            let channel = format!("rift_{}", msg_rift_id);
            let _ = state.broadcaster.send((channel.clone(), response));
            
            info!("📤 Broadcasted file change to rift channel: {}", channel);
            
            // TODO: Implement smart checkpointing
            // Check if we should create automatic checkpoint (every N changes or time-based)
            // if should_create_auto_checkpoint(msg_rift_id, &state).await? {
            //     let checkpoint = state.storage.create_checkpoint(...).await?;
            //     // Broadcast checkpoint creation
            // }
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