use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

// Tokio imports
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::time::{Duration, Instant, sleep_until};

// WebSocket imports
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite;

// External crates
use anyhow::{Result, anyhow};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Internal imports
use crate::file_watcher::{FileChangeEvent, FileWatcher};
use crate::ipc_server::IpcServer;
use crate::system_tray::SystemTray;
use mothership_common::{
    DiffEngine,
    LogicalPosition,
    CRDTOperationType,
    FileDiff,
    ConflictRiftInfo,
    SyncMessage,
    transaction::TransactionManager,
};

/// Information about a tracked project
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackedProject {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_path: PathBuf,
    pub added_at: chrono::DateTime<chrono::Utc>,
}

/// The main Mothership daemon that coordinates all background services
pub struct MothershipDaemon {
    /// Maps project ID to file watcher
    project_watchers: Arc<RwLock<HashMap<Uuid, FileWatcher>>>,
    
    /// Registry of tracked projects
    tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
    
    /// IPC server for CLI communication
    ipc_server: IpcServer,
    
    /// System tray integration
    system_tray: Option<SystemTray>,
    
    /// Channel for receiving file change events
    file_change_receiver: mpsc::UnboundedReceiver<FileChangeEvent>,
    
    /// Channel for sending file change events to watchers
    file_change_sender: mpsc::UnboundedSender<FileChangeEvent>,
    
    /// Current daemon status
    status: Arc<RwLock<DaemonStatus>>,
    
    /// Maps project ID to WebSocket listener task handles
    websocket_listeners: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
    
    /// Maps project ID to outgoing message channels (for sending to WebSocket)
    outgoing_channels: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    
    transaction_manager: Arc<Mutex<TransactionManager>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DaemonStatus {
    pub is_running: bool,
    pub projects_tracked: usize,
    pub files_syncing: usize,
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    pub server_connected: bool,
}

impl Default for DaemonStatus {
    fn default() -> Self {
        Self {
            is_running: false,
            projects_tracked: 0,
            files_syncing: 0,
            last_sync: None,
            server_connected: false,
        }
    }
}

impl MothershipDaemon {
    /// Create a new Mothership daemon instance
    pub async fn new() -> Result<Self> {
        info!("Initializing Mothership Daemon...");
        
        // Create communication channels
        let (file_change_sender, file_change_receiver) = mpsc::unbounded_channel();
        
        // Initialize components
        let status = Arc::new(RwLock::new(DaemonStatus::default()));
        let tracked_projects = Arc::new(RwLock::new(HashMap::new()));
        
        // Create IPC server with access to daemon methods
        let ipc_server = IpcServer::new(
            status.clone(),
            tracked_projects.clone(),
            file_change_sender.clone(),
        ).await?;
        
        // Initialize system tray (Windows only)
        #[cfg(windows)]
        let system_tray = Some(SystemTray::new(status.clone(), tracked_projects.clone())?);
        #[cfg(not(windows))]
        let system_tray = None;
        
        Ok(Self {
            project_watchers: Arc::new(RwLock::new(HashMap::new())),
            tracked_projects,
            ipc_server,
            system_tray,
            file_change_receiver,
            file_change_sender,
            status,
            websocket_listeners: Arc::new(RwLock::new(HashMap::new())),
            outgoing_channels: Arc::new(RwLock::new(HashMap::new())),
            transaction_manager: Arc::new(Mutex::new(TransactionManager::new(Uuid::new_v4()))),
        })
    }
    
    /// Run the daemon (main event loop)
    pub async fn run(self) -> Result<()> {
        info!("🚀 Starting Mothership Daemon...");
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.is_running = true;
        }
        
        // Start the IPC server
        let ipc_handle = {
            // Move the IPC server out of self
            let ipc_server = self.ipc_server;
            tokio::spawn(async move {
                if let Err(e) = ipc_server.start().await {
                    error!("IPC server error: {}", e);
                }
            })
        };
        
        // Get file change receiver (moved out of self since IPC server was moved)
        let mut file_change_receiver = self.file_change_receiver;
        
        // Start system tray (Windows only)
        #[cfg(windows)]
        let tray_handle = if let Some(system_tray) = self.system_tray {
            Some(tokio::spawn(async move {
                if let Err(e) = system_tray.run().await {
                    error!("System tray error: {}", e);
                }
            }))
        } else {
            None
        };
        #[cfg(not(windows))]
        let tray_handle: Option<tokio::task::JoinHandle<()>> = None;
        
        // Main event loop - process file change events
        info!("✅ Mothership Daemon is running!");
        info!("🔍 IPC server listening on http://localhost:7525");
        info!("⏳ Waiting for projects to be registered via CLI/GUI...");
        
        while let Some(event) = file_change_receiver.recv().await {
            if let Err(e) = Self::handle_file_change_static(event, &self.tracked_projects, &self.status, &self.outgoing_channels).await {
                error!("Error handling file change: {}", e);
            }
        }
        
        // Clean shutdown
        info!("🔄 Shutting down Mothership Daemon...");
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.is_running = false;
        }
        
        // Stop all persistent WebSocket connections
        {
            let mut listeners = self.websocket_listeners.write().await;
            for (project_id, handle) in listeners.drain() {
                handle.abort();
                info!("✅ Stopped persistent WebSocket for project {}", project_id);
            }
        }
        
        // Clear outgoing channels
        {
            let mut channels = self.outgoing_channels.write().await;
            channels.clear();
        }
        
        // Cancel background tasks
        ipc_handle.abort();
        
        #[cfg(windows)]
        if let Some(handle) = tray_handle {
            handle.abort();
        }
        
        info!("✅ Mothership Daemon shutdown complete");
        Ok(())
    }
    
    /// Handle a file change event (static version for use after moving fields)
    async fn handle_file_change_static(
        event: FileChangeEvent,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        status: &Arc<RwLock<DaemonStatus>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    ) -> Result<()> {
        // Get project info for better logging
        let project_name = {
            let projects = tracked_projects.read().await;
            projects.get(&event.project_id)
                .map(|p| p.project_name.clone())
                .unwrap_or_else(|| event.project_id.to_string())
        };
        
        info!("📝 File {:?}: {} ({} bytes) in project '{}'", 
            event.change_type, event.file_path.display(), event.file_size, project_name);
        
        // Update sync status
        {
            let mut status_guard = status.write().await;
            status_guard.files_syncing += 1;
            status_guard.last_sync = Some(chrono::Utc::now());
        }
        
        // PERSISTENT WEBSOCKET: Send file change via persistent connection
        let sync_result = Self::send_file_change_via_persistent_websocket(&event, tracked_projects, outgoing_channels).await;
        match sync_result {
            Ok(()) => {
                info!("✅ Successfully queued file change for persistent WebSocket");
            }
            Err(e) => {
                error!("❌ Failed to queue file change for persistent WebSocket: {}", e);
            }
        }
        
        // Update sync status
        {
            let mut status_guard = status.write().await;
            status_guard.files_syncing = status_guard.files_syncing.saturating_sub(1);
        }
        
        Ok(())
    }

    /// Handle a file change event
    async fn handle_file_change(&self, event: FileChangeEvent) -> Result<()> {
        Self::handle_file_change_static(event, &self.tracked_projects, &self.status, &self.outgoing_channels).await
    }
    
    /// Send file change via persistent WebSocket connection
    async fn send_file_change_via_persistent_websocket(
        event: &FileChangeEvent,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    ) -> Result<()> {
        // Get project metadata to determine rift_id
        let rift_id = {
            let projects = tracked_projects.read().await;
            let project = projects.get(&event.project_id)
                .ok_or_else(|| anyhow::anyhow!("Project not found in tracked projects: {}", event.project_id))?;
            
            // Load project metadata to get rift_id
            let metadata_file = project.project_path.join(".mothership").join("project.json");
            let metadata_content = tokio::fs::read_to_string(&metadata_file).await
                .map_err(|e| anyhow::anyhow!("Failed to read project metadata: {}", e))?;
            
            let metadata: ProjectMetadata = serde_json::from_str(&metadata_content)
                .map_err(|e| anyhow::anyhow!("Failed to parse project metadata: {}", e))?;
            
            if let Some(rift_id_str) = &metadata.rift_id {
                uuid::Uuid::parse_str(rift_id_str)
                    .map_err(|e| anyhow::anyhow!("Invalid rift_id in metadata: {}", e))?
            } else {
                event.project_id // Fallback to project_id
            }
        };
        
        // Create sync message
        let sync_message = SyncMessage::FileChanged {
            rift_id,
            path: event.file_path.clone(),
            content: event.content.clone(),
            timestamp: event.timestamp,
        };
        
        // Send via persistent WebSocket channel
        {
            let channels = outgoing_channels.read().await;
            if let Some(sender) = channels.get(&event.project_id) {
                sender.send(sync_message)
                    .map_err(|e| anyhow::anyhow!("Failed to queue message for persistent WebSocket: {}", e))?;
            } else {
                return Err(anyhow::anyhow!("No persistent WebSocket connection found for project {}", event.project_id));
            }
        }
        
        Ok(())
    }
    
    /// Get current daemon status
    pub async fn get_status(&self) -> DaemonStatus {
        self.status.read().await.clone()
    }
    
    /// Get list of tracked projects
    pub async fn get_tracked_projects(&self) -> Vec<TrackedProject> {
        self.tracked_projects.read().await.values().cloned().collect()
    }
    
    /// Add a new project to track (called via IPC from CLI/GUI)
    pub async fn add_project(
        &self, 
        project_id: Uuid, 
        project_name: String,
        project_path: PathBuf
    ) -> Result<()> {
        info!("📂 Registering project for tracking: '{}' at {} ({})", 
            project_name, project_path.display(), project_id);
        
        // Check if project is already tracked
        {
            let projects = self.tracked_projects.read().await;
            if projects.contains_key(&project_id) {
                warn!("Project '{}' ({}) is already being tracked", project_name, project_id);
                return Ok(());
            }
        }
        
        // Validate project path exists and has .mothership directory
        if !project_path.exists() {
            return Err(anyhow::anyhow!("Project path does not exist: {}", project_path.display()));
        }
        
        let mothership_dir = project_path.join(".mothership");
        if !mothership_dir.exists() {
            return Err(anyhow::anyhow!("No .mothership directory found at: {}", project_path.display()));
        }
        
        // Create file watcher
        let watcher = FileWatcher::new(
            project_path.clone(),
            project_id,
            self.file_change_sender.clone(),
        ).await?;
        
        // Add to tracked projects registry
        let tracked_project = TrackedProject {
            project_id,
            project_name: project_name.clone(),
            project_path: project_path.clone(),
            added_at: chrono::Utc::now(),
        };
        
        {
            let mut projects = self.tracked_projects.write().await;
            projects.insert(project_id, tracked_project);
        }
        
        // Add to file watchers
        {
            let mut watchers = self.project_watchers.write().await;
            watchers.insert(project_id, watcher);
        }
        
        // Check server connectivity and update status
        let server_reachable = Self::check_server_connectivity().await;
        {
            let mut status = self.status.write().await;
            status.projects_tracked = self.tracked_projects.read().await.len();
            status.server_connected = server_reachable;
        }
        
        // Start persistent WebSocket connection for bidirectional sync
        if let Err(e) = Self::start_websocket_listener(
            project_id,
            self.tracked_projects.clone(),
            self.status.clone(),
            self.websocket_listeners.clone(),
            self.outgoing_channels.clone(),
        ).await {
            error!("Failed to start persistent WebSocket for project '{}': {}", project_name, e);
            // Don't fail the entire operation if WebSocket fails
        }
        
        info!("✅ Successfully registered project '{}' for tracking", project_name);
        Ok(())
    }
    
    /// Remove a project from tracking (called via IPC from CLI/GUI)
    pub async fn remove_project(&self, project_id: Uuid) -> Result<()> {
        info!("🗑️ Unregistering project from tracking: {}", project_id);
        
        // Get project name for logging
        let project_name = {
            let projects = self.tracked_projects.read().await;
            projects.get(&project_id)
                .map(|p| p.project_name.clone())
                .unwrap_or_else(|| project_id.to_string())
        };
        
        // Remove from tracked projects
        {
            let mut projects = self.tracked_projects.write().await;
            projects.remove(&project_id);
        }
        
        // Remove file watcher
        {
            let mut watchers = self.project_watchers.write().await;
            if watchers.remove(&project_id).is_some() {
                info!("✅ Stopped file watching for project '{}'", project_name);
            } else {
                warn!("Project '{}' was not being watched", project_name);
            }
        }
        
        // Stop persistent WebSocket connection
        {
            let mut listeners = self.websocket_listeners.write().await;
            if let Some(handle) = listeners.remove(&project_id) {
                handle.abort();
                info!("✅ Stopped persistent WebSocket for project '{}'", project_name);
            } else {
                warn!("Project '{}' had no persistent WebSocket", project_name);
            }
        }
        
        // Remove outgoing channel
        {
            let mut channels = self.outgoing_channels.write().await;
            channels.remove(&project_id);
        }
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.projects_tracked = self.tracked_projects.read().await.len();
        }
        
        info!("✅ Successfully unregistered project '{}' from tracking", project_name);
        Ok(())
    }
    
    /// Check if a project is being tracked
    pub async fn is_project_tracked(&self, project_id: Uuid) -> bool {
        let projects = self.tracked_projects.read().await;
        projects.contains_key(&project_id)
    }
    
    /// Check server connectivity by making a simple HTTP health check
    async fn check_server_connectivity() -> bool {
        if let Some(server_url) = get_active_server_url() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            
            let health_url = format!("{}/health", server_url);
            
            match client.get(&health_url).send().await {
                Ok(response) => {
                    let is_ok = response.status().is_success();
                    if is_ok {
                        info!("✅ Server connectivity check passed");
                    } else {
                        warn!("❌ Server responded with status: {}", response.status());
                    }
                    is_ok
                }
                Err(e) => {
                    warn!("❌ Server connectivity check failed: {}", e);
                    false
                }
            }
        } else {
            warn!("❌ No active server URL found");
            false
        }
    }
    
    /// Start persistent bidirectional WebSocket connection for a project
    async fn start_websocket_listener(
        project_id: Uuid,
        tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        status: Arc<RwLock<DaemonStatus>>,
        websocket_listeners: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
        outgoing_channels: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    ) -> Result<()> {
        // Get project information
        let (project_path, rift_id) = {
            let projects = tracked_projects.read().await;
            let project = projects.get(&project_id)
                .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_id))?;
            
            // Load project metadata to get rift_id
            let metadata_file = project.project_path.join(".mothership").join("project.json");
            let metadata_content = tokio::fs::read_to_string(&metadata_file).await
                .map_err(|e| anyhow::anyhow!("Failed to read project metadata: {}", e))?;
            
            let metadata: ProjectMetadata = serde_json::from_str(&metadata_content)
                .map_err(|e| anyhow::anyhow!("Failed to parse project metadata: {}", e))?;
            
            let rift_id = if let Some(rift_id_str) = &metadata.rift_id {
                uuid::Uuid::parse_str(rift_id_str)
                    .map_err(|e| anyhow::anyhow!("Invalid rift_id in metadata: {}", e))?
            } else {
                project_id // Fallback to project_id
            };
            
            (project.project_path.clone(), rift_id)
        };
        
        // Get authentication token
        let auth_token = load_auth_token()
            .ok_or_else(|| anyhow::anyhow!("No authentication token found"))?;
        
        debug!("🔑 Loaded auth token: {}...", &auth_token.chars().take(10).collect::<String>());
        
        // Get server URL
        let server_url = get_active_server_url()
            .ok_or_else(|| anyhow::anyhow!("No active server connection found"))?;
        
        debug!("🌐 Active server URL: {}", server_url);
        
        // Construct WebSocket URL
        let ws_url = if server_url.starts_with("https://") {
            let ws_base = server_url.replace("https://", "wss://");
            format!("{}/sync/{}?token={}", ws_base, rift_id, urlencoding::encode(&auth_token))
        } else if server_url.starts_with("http://") {
            let ws_base = server_url.replace("http://", "ws://");
            format!("{}/sync/{}?token={}", ws_base, rift_id, urlencoding::encode(&auth_token))
        } else {
            format!("wss://{}/sync/{}?token={}", server_url, rift_id, urlencoding::encode(&auth_token))
        };
        
        info!("🔄 Starting persistent WebSocket connection for project {} (rift: {})", project_id, rift_id);
        info!("📡 WebSocket URL: {}", ws_url.replace(&auth_token, "***TOKEN***"));
        
        // Create channel for outgoing messages
        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<SyncMessage>();
        let outgoing_tx_clone = outgoing_tx.clone();
        {
            let mut channels = outgoing_channels.write().await;
            channels.insert(project_id, outgoing_tx);
        }

        let status_clone = status.clone();
        let listener_handle = tokio::spawn(async move {
            let ping_interval = Duration::from_secs(30);
            let health_log_interval = Duration::from_secs(300);
            let reconnect_delay = Duration::from_secs(5);
            let mut health = ConnectionHealth::new();
            
            // CRITICAL FIX: Add reconnection loop
            loop {
                let mut next_ping = Instant::now() + ping_interval;
                let mut next_health_log = Instant::now() + health_log_interval;
                
                info!("🔌 Connecting to WebSocket: {}", ws_url);
                
                // CRITICAL FIX: Actually connect to the WebSocket server!
                match tokio_tungstenite::connect_async(&ws_url).await {
                    Ok((ws_stream, response)) => {
                        info!("✅ WebSocket connected successfully!");
                        debug!("📋 WebSocket response status: {}", response.status());
                        debug!("📋 WebSocket response headers: {:?}", response.headers());
                        
                        // Update connection status to connected
                        {
                            let mut status_guard = status_clone.write().await;
                            status_guard.server_connected = true;
                        }
                        
                        let (mut ws_sender, mut ws_receiver) = ws_stream.split();
                        
                        // Send initial join message
                        let join_msg = SyncMessage::JoinRift { 
                            rift_id, 
                            last_checkpoint: None 
                        };
                        if let Ok(join_json) = serde_json::to_string(&join_msg) {
                            debug!("📤 Sending join message: {}", join_json);
                            if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Text(join_json)).await {
                                error!("Failed to send join message: {}", e);
                            } else {
                                info!("📡 Sent rift join message");
                            }
                        }
                        
                        loop {
                            tokio::select! {
                                // Handle outgoing messages (from file watcher)
                                msg = outgoing_rx.recv() => {
                                    match msg {
                                        Some(sync_msg) => {
                                            if let Ok(json) = serde_json::to_string(&sync_msg) {
                                                if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Text(json)).await {
                                                    error!("Failed to send WebSocket message: {}", e);
                                                    health.record_error();
                                                    break;
                                                } else {
                                                    health.record_message_sent();
                                                    debug!("📤 Sent sync message to server");
                                                }
                                            }
                                        }
                                        None => {
                                            info!("Outgoing channel closed, stopping WebSocket");
                                            break;
                                        }
                                    }
                                }
                                
                                // Handle incoming messages (from server)
                                msg = ws_receiver.next() => {
                                    match msg {
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                            health.record_message_received();
                                            debug!("📥 Received WebSocket message: {} chars", text.len());
                                            
                                            // Handle incoming sync message
                                            if let Err(e) = Self::handle_websocket_sync_message(&text, &project_path).await {
                                                error!("Failed to handle incoming sync message: {}", e);
                                            }
                                        }
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(close_frame))) => {
                                            info!("WebSocket closed by server: {:?}", close_frame);
                                            // Send close frame back to complete handshake
                                            let _ = ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(close_frame)).await;
                                            break;
                                        }
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                                            // Respond to ping with pong
                                            if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await {
                                                error!("Failed to send pong: {}", e);
                                                health.record_error();
                                            } else {
                                                debug!("🏓 Sent pong response");
                                            }
                                        }
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(_))) => {
                                            debug!("🏓 Received pong");
                                            health.record_message_received();
                                        }
                                        Some(Err(e)) => {
                                            error!("WebSocket error: {}", e);
                                            health.record_error();
                                            // Don't break immediately on error - let health check decide
                                            if health.should_reset(3) {
                                                error!("Too many consecutive errors, closing connection");
                                                break;
                                            }
                                        }
                                        None => {
                                            info!("WebSocket stream ended");
                                            break;
                                        }
                                        _ => {} // Ignore other message types
                                    }
                                }

                                // Send periodic ping to keep connection alive
                                _ = sleep_until(next_ping) => {
                                    let ping_msg = SyncMessage::Heartbeat;
                                    if let Ok(ping_json) = serde_json::to_string(&ping_msg) {
                                        if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Text(ping_json)).await {
                                            error!("Failed to send ping: {}", e);
                                            health.record_error();
                                            if health.should_reset(3) {
                                                break;
                                            }
                                        } else {
                                            debug!("🏓 Sent ping");
                                            health.record_message_sent();
                                        }
                                    }
                                    next_ping = Instant::now() + ping_interval;
                                }

                                // Log connection health periodically
                                _ = sleep_until(next_health_log) => {
                                    info!("📊 Connection health: {}", health.get_health_report());
                                    next_health_log = Instant::now() + health_log_interval;
                                }
                            }
                        }
                        
                        info!("🔌 WebSocket connection closed");
                        
                        // Update connection status to disconnected
                        {
                            let mut status_guard = status_clone.write().await;
                            status_guard.server_connected = false;
                        }
                    }
                    Err(e) => {
                        error!("❌ Failed to connect to WebSocket: {}", e);
                        
                        // Log more specific error details based on error string
                        let error_str = e.to_string();
                        
                        if error_str.contains("401") {
                            error!("  Authentication failed - token may be invalid or expired");
                            error!("  Try running 'mothership auth' to refresh your credentials");
                        } else if error_str.contains("404") {
                            error!("  WebSocket endpoint not found - rift may not exist");
                            error!("  Rift ID: {}", rift_id);
                        } else if error_str.contains("Connection refused") {
                            error!("  Connection refused - server may be down");
                            error!("  Server URL: {}", server_url);
                        } else if error_str.contains("Invalid status code") {
                            error!("  Server returned unexpected status code");
                            error!("  This might indicate an authentication or routing issue");
                        } else if error_str.contains("DNS") || error_str.contains("resolve") {
                            error!("  DNS resolution failed - check server URL");
                            error!("  Server URL: {}", server_url);
                        }
                        
                        health.record_error();
                        
                        // Update connection status to disconnected
                        {
                            let mut status_guard = status_clone.write().await;
                            status_guard.server_connected = false;
                        }
                    }
                }
                
                // Wait before reconnecting
                info!("⏱️  Waiting {} seconds before reconnecting...", reconnect_delay.as_secs());
                tokio::time::sleep(reconnect_delay).await;
                
                // Reset health on reconnection attempt
                health.record_reset();
            } // End of reconnection loop
        }); // End of spawned task

        // Store the handle for later cleanup, but don't wait for it
        {
            let mut listeners = websocket_listeners.write().await;
            listeners.insert(project_id, listener_handle);
        }
        
        info!("✅ WebSocket listener started for project {}", project_id);
        Ok(())
    }
    
    /// Handle incoming sync messages from the server
    async fn handle_incoming_sync_message(text: &str, project_path: &PathBuf, state: &Arc<MothershipDaemon>) -> Result<()> {
        let sync_message: SyncMessage = serde_json::from_str(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse sync message: {}", e))?;
        
        match sync_message {
            SyncMessage::FileChanged { path, content, .. } => {
                info!("📥 Received file change: {} ({} bytes)", path.display(), content.len());
                
                // Write the file to disk
                let file_path = project_path.join(&path);
                
                // Create parent directories if needed
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                
                // Write file content
                tokio::fs::write(&file_path, &content).await?;
                info!("💾 Wrote incoming file change: {}", path.display());
                Ok(())
            }
            SyncMessage::FilesChanged { changes, .. } => {
                info!("📥 Received {} file changes", changes.len());
                
                for change in changes {
                    // For now, we'll need to get the content from the server
                    // This is a simplified implementation
                    info!("📄 File change: {} ({:?})", change.path.display(), change.change_type);
                }
                Ok(())
            }
            SyncMessage::RiftJoined { current_files, .. } => {
                info!("📥 Received current rift state with {} files", current_files.len());
                
                // Write all current files (initial sync)
                for (path, content) in current_files {
                    let file_path = project_path.join(&path);
                    
                    // Create parent directories if needed
                    if let Some(parent) = file_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    
                    // Write file content
                    tokio::fs::write(&file_path, &content).await?;
                    info!("💾 Wrote initial file: {}", path.display());
                }
                Ok(())
            }
            SyncMessage::ConflictDetected { 
                path, 
                server_content, 
                client_diff: _,
                server_timestamp: _,
                client_timestamp: _,
                auto_created_rift: _,
                rift_id: _,
                conflict: _,
                suggestions: _,
            } => {
                info!("🔄 Conflict detected for {}, accepting server version", path.display());
                
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, &server_content).await?;
                Ok(())
            }
            SyncMessage::ForceSync { 
                path, 
                server_content,
                server_timestamp: _,
            } => {
                Self::force_sync(path, server_content).await?;
                Ok(())
            }
            SyncMessage::RequestLatestContent { path } => {
                // Server is requesting our latest content - send it
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let response = SyncMessage::ContentResponse {
                        path,
                        content,
                        timestamp: chrono::Utc::now(),
                    };
                    
                    if let Some(sender) = Self::get_message_sender().await {
                        sender.send(response)?;
                    }
                }
                Ok(())
            }
            SyncMessage::BeginTransaction { 
                transaction_id: _,
                description,
                author,
                rift_id: _,
                ..
            } => {
                info!("📝 Begin transaction from {}: {}", author, description);
                let mut tx_manager = state.transaction_manager.lock().await;
                let _transaction = tx_manager.create_transaction(author, description);
                Ok(())
            }
            SyncMessage::AddFileModification { 
                transaction_id, 
                path, 
                diff, 
                previous_hash: _,
            } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                let current_content = tokio::fs::read_to_string(&path).await?;
                
                let engine = DiffEngine::new();
                let new_content = engine.apply_diff(&current_content, &diff)?;
                tx_manager.add_file_modification(
                    transaction_id,
                    path,
                    &new_content,
                    &current_content,
                )?;
                Ok(())
            }
            SyncMessage::AddFileCreation { transaction_id, path, content } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                tx_manager.add_file_creation(transaction_id, path, content)?;
                Ok(())
            }
            SyncMessage::AddFileDeletion { transaction_id, path, previous_hash } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                let current_content = tokio::fs::read_to_string(&path).await?;
                
                if crypto_hash(&current_content) != previous_hash {
                    return Err(anyhow!("File content changed since transaction started"));
                }
                
                tx_manager.add_file_deletion(transaction_id, path, current_content)?;
                Ok(())
            }
            SyncMessage::CommitTransaction { transaction_id } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                tx_manager.commit_transaction(transaction_id).await?;
                Ok(())
            }
            SyncMessage::RollbackTransaction { transaction_id } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                tx_manager.rollback_transaction(transaction_id).await?;
                Ok(())
            }
            SyncMessage::DirectoryUpdate { 
                path, 
                crdt_operations, 
                timestamp: _,
            } => {
                let mut tx_manager = state.transaction_manager.lock().await;
                let crdt = tx_manager.get_directory_crdt(&path);
                
                for op in crdt_operations {
                    match op.operation_type {
                        CRDTOperationType::CreateFile { name } => {
                            crdt.insert(
                                LogicalPosition::new(vec![0], op.author),
                                format!("create_file:{}", name),
                            );
                        }
                        CRDTOperationType::DeleteFile { name } => {
                            crdt.insert(
                                LogicalPosition::new(vec![0], op.author),
                                format!("delete_file:{}", name),
                            );
                        }
                        CRDTOperationType::CreateDirectory { name } => {
                            crdt.insert(
                                LogicalPosition::new(vec![0], op.author),
                                format!("create_dir:{}", name),
                            );
                        }
                        CRDTOperationType::DeleteDirectory { name } => {
                            crdt.insert(
                                LogicalPosition::new(vec![0], op.author),
                                format!("delete_dir:{}", name),
                            );
                        }
                        CRDTOperationType::RenameEntry { old_name, new_name } => {
                            crdt.insert(
                                LogicalPosition::new(vec![0], op.author),
                                format!("rename:{}:{}", old_name, new_name),
                            );
                        }
                    }
                }
                
                Ok(())
            }
            SyncMessage::ConflictRiftCreated {
                original_rift_id: _,
                new_rift_id: _,
                conflict_rift_name,
            } => {
                info!("✨ Conflict rift '{}' created successfully", conflict_rift_name);
                info!("🔀 Use 'mothership beam \"{}\"' to work on your changes", conflict_rift_name);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Handle WebSocket sync message (simplified version for static context)
    async fn handle_websocket_sync_message(text: &str, project_path: &PathBuf) -> Result<()> {
        let sync_message: SyncMessage = serde_json::from_str(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse sync message: {}", e))?;
        
        match sync_message {
            SyncMessage::FileChanged { path, content, .. } => {
                info!("📥 Received file change from collaborator: {} ({} bytes)", path.display(), content.len());
                
                // Write the file to disk
                let file_path = project_path.join(&path);
                
                // Create parent directories if needed
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                
                // Write file content
                tokio::fs::write(&file_path, &content).await?;
                info!("💾 Applied file change from collaborator: {}", path.display());
                Ok(())
            }
            SyncMessage::RiftDiffUpdate { diff_changes, .. } => {
                info!("📥 Received {} diff updates from collaborator", diff_changes.len());
                
                for change in diff_changes {
                    let file_path = project_path.join(&change.path);
                    
                    // Read current content
                    let current_content = if file_path.exists() {
                        tokio::fs::read_to_string(&file_path).await.unwrap_or_default()
                    } else {
                        String::new()
                    };
                    
                    // Apply diff
                    let diff_engine = DiffEngine::new();
                    match diff_engine.apply_diff(&current_content, &change.diff) {
                        Ok(new_content) => {
                            // Create parent directories if needed
                            if let Some(parent) = file_path.parent() {
                                tokio::fs::create_dir_all(parent).await?;
                            }
                            
                            // Write updated content
                            tokio::fs::write(&file_path, &new_content).await?;
                            info!("💾 Applied diff to {}: {} -> {} bytes", 
                                change.path.display(), current_content.len(), new_content.len());
                        }
                        Err(e) => {
                            error!("Failed to apply diff to {}: {}", change.path.display(), e);
                        }
                    }
                }
                Ok(())
            }
            SyncMessage::RiftJoined { current_files, .. } => {
                info!("📥 Received initial rift state with {} files", current_files.len());
                
                // Write all current files (initial sync)
                for (path, content) in current_files {
                    let file_path = project_path.join(&path);
                    
                    // Create parent directories if needed
                    if let Some(parent) = file_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    
                    // Write file content
                    tokio::fs::write(&file_path, &content).await?;
                    info!("💾 Wrote initial file: {}", path.display());
                }
                Ok(())
            }
            SyncMessage::Heartbeat => {
                debug!("🏓 Received heartbeat from server");
                Ok(())
            }
            _ => {
                debug!("📨 Received sync message: {:?} (not handled in WebSocket context)", std::mem::discriminant(&sync_message));
                Ok(())
            }
        }
    }

    /// Handle a conflict by creating a new rift for conflicting changes
    async fn handle_conflict_with_rift(
        &self,
        path: PathBuf,
        server_content: String,
        _client_diff: FileDiff,
        _auto_created_rift: Option<ConflictRiftInfo>,
    ) -> Result<()> {
        // Always accept server's version in the original rift
        info!("🔄 Conflict detected for {}, accepting server version", path.display());
        
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write server's content to original rift
        tokio::fs::write(&path, &server_content).await?;

        Ok(())
    }

    /// Force sync a file from the server
    async fn force_sync(path: PathBuf, server_content: String) -> Result<()> {
        info!("🔄 Force syncing {} from server", path.display());
        
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write server's content
        tokio::fs::write(&path, server_content).await?;
        
        Ok(())
    }

    /// Get message sender for WebSocket communication
    async fn get_message_sender() -> Option<mpsc::UnboundedSender<SyncMessage>> {
        // This is a placeholder - in a real implementation, this would return
        // the sender for the active WebSocket connection
        None
    }
}

/// Project metadata structure (must match what's stored by CLI)
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
    rift_id: Option<String>, // CRITICAL FIX: Read rift_id for WebSocket connection
}

/// Get the active server URL (prioritize active connection over project metadata)
fn get_active_server_url() -> Option<String> {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct ServerConnection {
        pub name: String,
        pub url: String,
        pub auth_token: Option<String>,
        pub auth_method: String,
        pub connected_at: chrono::DateTime<chrono::Utc>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct ConnectionsConfig {
        pub active_server: Option<String>,
        pub servers: std::collections::HashMap<String, ServerConnection>,
    }
    
    // Try to load active server connection
    if let Some(config_dir) = dirs::config_dir() {
        let connections_path = config_dir.join("mothership").join("connections.json");
        if connections_path.exists() {
            if let Ok(connections_content) = std::fs::read_to_string(&connections_path) {
                if let Ok(connections) = serde_json::from_str::<ConnectionsConfig>(&connections_content) {
                    if let Some(active_url) = &connections.active_server {
                        if let Some(server) = connections.servers.get(active_url) {
                            info!("🌐 Using active server connection: {}", server.url);
                            return Some(server.url.clone());
                        }
                    }
                }
            }
        }
    }
    
    None
}

/// Load stored authentication token for WebSocket connection
fn load_auth_token() -> Option<String> {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredCredentials {
        access_token: String,
        user_email: Option<String>,
        user_name: Option<String>,
        stored_at: String,
    }
    
    // Try to load OAuth credentials first
    if let Some(config_dir) = dirs::config_dir() {
        let credentials_path = config_dir.join("mothership").join("credentials.json");
        if credentials_path.exists() {
            if let Ok(credentials_content) = std::fs::read_to_string(&credentials_path) {
                if let Ok(credentials) = serde_json::from_str::<StoredCredentials>(&credentials_content) {
                    return Some(credentials.access_token);
                }
            }
        }
    }
    
    // Fallback to old config format
    if let Some(config_dir) = dirs::config_dir() {
        let config_path = config_dir.join("mothership").join("config.json");
        if config_path.exists() {
            if let Ok(config_content) = std::fs::read_to_string(&config_path) {
                if let Ok(config_json) = serde_json::from_str::<serde_json::Value>(&config_content) {
                    if let Some(token) = config_json.get("auth_token").and_then(|t| t.as_str()) {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }
    
    None
}

#[derive(Debug, Clone)]
struct ConnectionHealth {
    last_ping_time: Instant,
    consecutive_errors: u32,
    total_messages_sent: u64,
    total_messages_received: u64,
    connection_resets: u32,
    last_reset_time: Option<Instant>,
}

impl ConnectionHealth {
    fn new() -> Self {
        Self {
            last_ping_time: Instant::now(),
            consecutive_errors: 0,
            total_messages_sent: 0,
            total_messages_received: 0,
            connection_resets: 0,
            last_reset_time: None,
        }
    }

    fn record_message_sent(&mut self) {
        self.total_messages_sent += 1;
    }

    fn record_message_received(&mut self) {
        self.total_messages_received += 1;
        self.consecutive_errors = 0; // Reset errors on successful receive
    }

    fn record_error(&mut self) {
        self.consecutive_errors += 1;
    }

    fn record_reset(&mut self) {
        self.connection_resets += 1;
        self.last_reset_time = Some(Instant::now());
    }

    fn should_reset(&self, max_errors: u32) -> bool {
        self.consecutive_errors >= max_errors
    }

    fn get_health_report(&self) -> String {
        format!(
            "Connection Health Report:\n\
             - Messages Sent: {}\n\
             - Messages Received: {}\n\
             - Current Error Streak: {}\n\
             - Total Connection Resets: {}\n\
             - Time Since Last Reset: {}\n\
             - Time Since Last Ping: {}s",
            self.total_messages_sent,
            self.total_messages_received,
            self.consecutive_errors,
            self.connection_resets,
            self.last_reset_time.map_or("Never".to_string(), |t| format!("{:?} ago", t.elapsed())),
            self.last_ping_time.elapsed().as_secs()
        )
    }
}

fn crypto_hash(content: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
} 