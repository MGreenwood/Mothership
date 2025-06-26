use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{sink::SinkExt, stream::StreamExt};
use mothership_common::protocol::SyncMessage;
use std::collections::HashMap;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;

/// Manages real-time synchronization between clients
pub struct SyncManager {
    /// Active WebSocket connections per rift
    connections: RwLock<HashMap<String, broadcast::Sender<SyncMessage>>>,
}

impl SyncManager {
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new connection for a rift
    pub async fn register_connection(&self, rift_id: &str) -> broadcast::Receiver<SyncMessage> {
        let mut connections = self.connections.write().await;
        
        // Get or create broadcast channel for this rift
        let sender = connections
            .entry(rift_id.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(1000); // Buffer 1000 messages
                info!("Created new sync channel for rift: {}", rift_id);
                tx
            })
            .clone();

        sender.subscribe()
    }

    /// Broadcast a message to all clients in a rift
    pub async fn broadcast_to_rift(&self, rift_id: &str, message: SyncMessage) -> Result<()> {
        let connections = self.connections.read().await;
        
        if let Some(sender) = connections.get(rift_id) {
            if let Err(e) = sender.send(message) {
                warn!("Failed to broadcast to rift {}: {}", rift_id, e);
            }
        }

        Ok(())
    }

    /// Get connection count for a rift
    pub async fn get_connection_count(&self, rift_id: &str) -> usize {
        let connections = self.connections.read().await;
        connections
            .get(rift_id)
            .map(|sender| sender.receiver_count())
            .unwrap_or(0)
    }

    /// Clean up empty channels
    pub async fn cleanup(&self) {
        let mut connections = self.connections.write().await;
        connections.retain(|rift_id, sender| {
            let has_receivers = sender.receiver_count() > 0;
            if !has_receivers {
                info!("Cleaning up empty sync channel for rift: {}", rift_id);
            }
            has_receivers
        });
    }
}

/// Handle a WebSocket connection for real-time sync
pub async fn handle_websocket(
    state: AppState,
    socket: WebSocket,
    rift_id: String,
) -> Result<()> {
    info!("New WebSocket connection for rift: {}", rift_id);

    let (mut sender, mut receiver) = socket.split();
    
    // Register this connection with the sync manager
    let mut broadcast_rx = state.sync.register_connection(&rift_id).await;

    // Task to handle outgoing messages (server -> client)
    let rift_id_clone = rift_id.clone();
    let outgoing_task = tokio::spawn(async move {
        while let Ok(message) = broadcast_rx.recv().await {
            let json = match serde_json::to_string(&message) {
                Ok(json) => json,
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                    continue;
                }
            };

            if let Err(e) = sender.send(Message::Text(json)).await {
                error!("Failed to send WebSocket message: {}", e);
                break;
            }
        }
        info!("Outgoing task ended for rift: {}", rift_id_clone);
    });

    // Task to handle incoming messages (client -> server)
    let rift_id_clone = rift_id.clone();
    let state_clone = state.clone();
    let incoming_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = handle_sync_message(&state_clone, &rift_id_clone, text).await {
                        error!("Error handling sync message: {}", e);
                    }
                }
                Ok(Message::Binary(_)) => {
                    warn!("Received unexpected binary message");
                }
                Ok(Message::Ping(_data)) => {
                    // TODO: Handle ping properly - for now just ignore
                    // In a real implementation, we'd need to handle this differently
                    // since sender is moved to the outgoing task
                }
                Ok(Message::Pong(_)) => {
                    // Ignore pong messages
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket closed for rift: {}", rift_id_clone);
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
            }
        }
        info!("Incoming task ended for rift: {}", rift_id_clone);
    });

    // Wait for either task to complete
    tokio::select! {
        _ = outgoing_task => {
            info!("Outgoing task completed for rift: {}", rift_id);
        }
        _ = incoming_task => {
            info!("Incoming task completed for rift: {}", rift_id);
        }
    }

    Ok(())
}

/// Handle individual sync messages from clients
async fn handle_sync_message(
    state: &AppState,
    rift_id: &str,
    message_text: String,
) -> Result<()> {
    let message: SyncMessage = match serde_json::from_str(&message_text) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Failed to parse sync message: {}", e);
            return Ok(()); // Don't propagate parse errors
        }
    };

    match message {
        SyncMessage::JoinRift { rift_id: msg_rift_id, last_checkpoint } => {
            info!("Client joining rift: {} (last checkpoint: {:?})", msg_rift_id, last_checkpoint);
            
            // TODO: Send initial sync data if needed
            // For now, just acknowledge the join
            let response = SyncMessage::CollaboratorJoined {
                rift_id: msg_rift_id,
                user_id: Uuid::new_v4(), // TODO: Get from auth context
                username: "anonymous".to_string(),
            };
            
            state.sync.broadcast_to_rift(rift_id, response).await?;
        }
        
        SyncMessage::LeaveRift { rift_id: msg_rift_id } => {
            info!("Client leaving rift: {}", msg_rift_id);
            
            let response = SyncMessage::CollaboratorLeft {
                rift_id: msg_rift_id,
                user_id: Uuid::new_v4(), // TODO: Get from auth context
            };
            
            state.sync.broadcast_to_rift(rift_id, response).await?;
        }
        
        SyncMessage::FileChanged { rift_id: msg_rift_id, path, content: _, timestamp } => {
            info!("File changed in rift {}: {:?}", msg_rift_id, path);
            
            // TODO: Store the file change and create checkpoint
            // For now, just broadcast to other clients
            let response = SyncMessage::RiftUpdate {
                rift_id: msg_rift_id,
                changes: vec![], // TODO: Convert to FileChange
                author: Uuid::new_v4(), // TODO: Get from auth context
                timestamp,
            };
            
            state.sync.broadcast_to_rift(rift_id, response).await?;
        }
        
        SyncMessage::CreateCheckpoint { rift_id: msg_rift_id, message } => {
            info!("Checkpoint requested for rift: {} (message: {:?})", msg_rift_id, message);
            
            // TODO: Create actual checkpoint
            let response = SyncMessage::CheckpointCreated {
                rift_id: msg_rift_id,
                checkpoint_id: Uuid::new_v4(),
                author: Uuid::new_v4(), // TODO: Get from auth context
                timestamp: chrono::Utc::now(),
                message,
            };
            
            state.sync.broadcast_to_rift(rift_id, response).await?;
        }
        
        SyncMessage::Heartbeat => {
            // Echo back heartbeat
            state.sync.broadcast_to_rift(rift_id, SyncMessage::Heartbeat).await?;
        }
        
        _ => {
            warn!("Unhandled sync message type");
        }
    }

    Ok(())
} 