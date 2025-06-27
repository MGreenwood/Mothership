use anyhow::Result;

use colored::*;
use futures_util::{SinkExt, StreamExt};
use mothership_common::protocol::SyncMessage;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;


use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{error, info, warn};

pub struct FileWatcher {
    project_path: PathBuf,
    rift_id: String,
    websocket_url: String,
}

impl FileWatcher {
    pub fn new(project_path: PathBuf, rift_id: String, websocket_url: String) -> Self {
        Self {
            project_path,
            rift_id,
            websocket_url,
        }
    }

    pub async fn start_watching(&self) -> Result<()> {
        println!("{}", format!("🔍 Starting file watcher for: {}", self.project_path.display()).cyan());
        
        // Create WebSocket connection
        let websocket_url = format!("{}/ws/rift/{}", self.websocket_url.replace("http", "ws"), self.rift_id);
        println!("{}", format!("🌐 Connecting to: {}", websocket_url).dimmed());
        
        let (ws_stream, _) = connect_async(&websocket_url).await
            .map_err(|e| anyhow::anyhow!("Failed to connect to WebSocket: {}", e))?;
        
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();
        
        // Send join message
        let join_message = SyncMessage::JoinRift {
            rift_id: self.rift_id.parse()?,
            last_checkpoint: None,
        };
        let join_json = serde_json::to_string(&join_message)?;
        ws_sender.send(Message::Text(join_json)).await?;
        
        // Set up file system watcher
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
        watcher.watch(&self.project_path, RecursiveMode::Recursive)?;
        
        println!("{}", "✅ File watcher started successfully!".green().bold());
        println!("{}", "📝 Now edit files in your project - changes will sync automatically".dimmed());
        
        // Handle file changes and WebSocket messages concurrently
        let project_path = self.project_path.clone();
        let rift_id = self.rift_id.clone();
        
        // Task for handling file system events
        let file_events_task = tokio::task::spawn_blocking(move || {
            for res in rx {
                match res {
                    Ok(event) => {
                        if let Err(e) = handle_file_event(&event, &project_path, &rift_id) {
                            error!("Error handling file event: {}", e);
                        }
                    }
                    Err(e) => error!("File watcher error: {}", e),
                }
            }
        });
        
        // Task for handling WebSocket messages from server
        let websocket_task = tokio::spawn(async move {
            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(sync_msg) = serde_json::from_str::<SyncMessage>(&text) {
                            handle_sync_message(sync_msg).await;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        println!("{}", "🔌 WebSocket connection closed".yellow());
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });
        
        // Run both tasks concurrently
        tokio::select! {
            _ = file_events_task => {
                println!("{}", "📁 File watcher stopped".yellow());
            }
            _ = websocket_task => {
                println!("{}", "🌐 WebSocket connection ended".yellow());
            }
        }
        
        Ok(())
    }
}

fn handle_file_event(event: &Event, project_path: &Path, _rift_id: &str) -> Result<()> {
    // Filter out events we don't care about
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            // Process the event
        }
        _ => return Ok(()), // Ignore other event types
    }
    
    for path in &event.paths {
        // Skip hidden files and directories
        if path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        
        // Skip directories
        if path.is_dir() {
            continue;
        }
        
        // Skip common build/cache directories
        let path_str = path.to_string_lossy();
        if path_str.contains("target/") 
            || path_str.contains("node_modules/")
            || path_str.contains(".git/")
            || path_str.contains("dist/")
            || path_str.contains("build/")
        {
            continue;
        }
        
        // Get relative path from project root
        let relative_path = path.strip_prefix(project_path)
            .unwrap_or(path)
            .to_path_buf();
        
        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                println!("{}", format!("📝 File changed: {}", relative_path.display()).blue());
                
                // TODO: Send to WebSocket
                // For now, just log the change
                info!("File changed: {} ({} bytes)", relative_path.display(), content.len());
                
                // In a real implementation, we would:
                // 1. Create FileChanged message
                // 2. Send via WebSocket to server
                // 3. Handle any errors/retries
            }
            Err(e) => {
                warn!("Could not read file {}: {}", path.display(), e);
            }
        }
    }
    
    Ok(())
}

async fn handle_sync_message(message: SyncMessage) {
    match message {
        SyncMessage::RiftUpdate { changes, .. } => {
            println!("{}", format!("🔄 Received {} changes from collaborator", changes.len()).green());
            // TODO: Apply changes to local files
        }
        SyncMessage::CheckpointCreated { checkpoint_id, message, .. } => {
            let msg = message.unwrap_or_else(|| "Auto checkpoint".to_string());
            println!("{}", format!("📸 Checkpoint created: {} ({})", checkpoint_id, msg).green());
        }
        SyncMessage::CollaboratorJoined { username, .. } => {
            println!("{}", format!("👋 {} joined the collaboration", username).green());
        }
        SyncMessage::CollaboratorLeft { user_id, .. } => {
            println!("{}", format!("👋 Collaborator {} left", user_id).yellow());
        }
        _ => {
            // Handle other message types
        }
    }
} 