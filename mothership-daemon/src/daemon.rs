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
use flate2;

// Internal imports
use crate::file_watcher::{FileChangeEvent, FileWatcher, MAX_DIRECT_SYNC_SIZE};
use crate::ipc_server::IpcServer;
use crate::snapshots::{SnapshotRegistry, SNAPSHOT_SWEEP_INTERVAL_SECS};
use crate::system_tray::SystemTray;
use mothership_common::{
    DiffEngine,
    LogicalPosition,
    CRDTOperationType,
    FileDiff,
    ConflictRiftInfo,
    SyncMessage,
    content_hash,
    streaming::StreamConfig,
    transaction::TransactionManager,
};

/// How often the daemon re-checks the server's health endpoint
const SERVER_PROBE_INTERVAL_SECS: u64 = 15;

/// Commands that can be sent to the daemon
#[derive(Debug, Clone)]
pub enum DaemonCommand {
    ForceSync,
    Restart,
    Stop,
}

/// Information about a tracked project
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackedProject {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_path: PathBuf,
    pub added_at: chrono::DateTime<chrono::Utc>,
}

fn tracked_projects_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mothership").join("tracked_projects.json"))
}

pub fn load_persisted_projects() -> Vec<TrackedProject> {
    let Some(path) = tracked_projects_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<TrackedProject>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.project_path.join(".mothership").join("project.json").is_file())
        .collect()
}

pub fn save_persisted_projects(projects: &HashMap<Uuid, TrackedProject>) {
    let Some(path) = tracked_projects_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let list: Vec<&TrackedProject> = projects.values().collect();
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(path, json);
    }
}

/// Files up to this size keep their synced content in `SyncBase`, so the next save can be sent as a diff.
/// Larger files keep only the hash, and their saves go as full content or streams.
const BASE_CONTENT_CACHE_LIMIT: usize = 1024 * 1024;

/// What this client last synced for one file
#[derive(Debug, Clone)]
pub struct SyncBase {
    pub hash: String,
    pub content: Option<String>,
}

impl SyncBase {
    fn from_content(content: &str) -> Self {
        Self {
            hash: content_hash(content),
            content: (content.len() <= BASE_CONTENT_CACHE_LIMIT).then(|| content.to_string()),
        }
    }

    fn hash_only(hash: String) -> Self {
        Self { hash, content: None }
    }
}

/// Last-synced base per (project ID, relative file path)
pub type BaseHashes = Arc<RwLock<HashMap<(Uuid, String), SyncBase>>>;

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
    
    /// Maps project ID to server write flags (prevents file watcher loops)
    server_write_flags: Arc<RwLock<HashMap<Uuid, bool>>>,

    /// Content hash of each file as last synced with the server, keyed by (project, relative path).
    /// Sent as `base_hash` so the server can detect edits made against a stale base.
    base_hashes: BaseHashes,
    
    /// Channel for receiving daemon commands from system tray
    command_receiver: mpsc::UnboundedReceiver<DaemonCommand>,
    
    /// Channel for sending daemon commands (cloned to system tray)
    command_sender: mpsc::UnboundedSender<DaemonCommand>,

    /// Wormhole snapshot folders waiting to be removed (see snapshots.rs)
    snapshots: Arc<SnapshotRegistry>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DaemonStatus {
    pub is_running: bool,
    pub projects_tracked: usize,
    /// File changes being queued right now. Usually 0, because queueing finishes almost immediately.
    pub files_syncing: usize,
    /// File changes queued for sync since the daemon started.
    pub files_synced: usize,
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    pub server_connected: bool,
}

impl Default for DaemonStatus {
    fn default() -> Self {
        Self {
            is_running: false,
            projects_tracked: 0,
            files_syncing: 0,
            files_synced: 0,
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
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        
        // Initialize components
        let status = Arc::new(RwLock::new(DaemonStatus::default()));
        let tracked_projects = Arc::new(RwLock::new(HashMap::new()));
        let websocket_listeners = Arc::new(RwLock::new(HashMap::new()));
        let outgoing_channels = Arc::new(RwLock::new(HashMap::new()));
        let server_write_flags = Arc::new(RwLock::new(HashMap::new()));
        
        let base_hashes: BaseHashes = Arc::new(RwLock::new(HashMap::new()));
        let snapshots = Arc::new(SnapshotRegistry::load());

        // Create IPC server with access to daemon methods
        let ipc_server = IpcServer::new(
            status.clone(),
            tracked_projects.clone(),
            file_change_sender.clone(),
            websocket_listeners.clone(),
            outgoing_channels.clone(),
            server_write_flags.clone(),
            base_hashes.clone(),
            snapshots.clone(),
        ).await?;
        
        // Initialize system tray (Windows only)
        #[cfg(windows)]
        let system_tray = Some(SystemTray::new(status.clone(), tracked_projects.clone(), command_sender.clone())?);
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
            websocket_listeners,
            outgoing_channels,
            transaction_manager: Arc::new(Mutex::new(TransactionManager::new(Uuid::new_v4()))),
            server_write_flags,
            base_hashes,
            command_receiver,
            command_sender,
            snapshots,
        })
    }
    
    /// Run the daemon (main event loop)
    pub async fn run(self) -> Result<()> {
        info!("Starting Mothership Daemon...");
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.is_running = true;
        }
        
        // Keep server_connected in sync with the server's health endpoint,
        // independent of whether any project WebSocket is open.
        let probe_status = self.status.clone();
        tokio::spawn(async move {
            loop {
                let reachable = Self::check_server_connectivity().await;
                probe_status.write().await.server_connected = reachable;
                tokio::time::sleep(std::time::Duration::from_secs(SERVER_PROBE_INTERVAL_SECS)).await;
            }
        });

        // Remove beamed wormhole snapshots once their wormhole is dissolved or converted
        let sweep_snapshots = self.snapshots.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(SNAPSHOT_SWEEP_INTERVAL_SECS)).await;
                sweep_snapshots.sweep().await;
            }
        });

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
        
        // Get channels (moved out of self since IPC server was moved)
        let mut file_change_receiver = self.file_change_receiver;
        let mut command_receiver = self.command_receiver;
        
        // Keep references to shared data
        let tracked_projects = self.tracked_projects.clone();
        let status = self.status.clone();
        let outgoing_channels = self.outgoing_channels.clone();
        let server_write_flags = self.server_write_flags.clone();
        let base_hashes = self.base_hashes.clone();
        let websocket_listeners = self.websocket_listeners.clone();
        
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
        
        // Main event loop - process file change events and daemon commands
        info!("Mothership Daemon is running!");
        info!("IPC server listening on http://localhost:7525");
        info!("Waiting for projects to be registered via CLI/GUI...");
        
        loop {
            tokio::select! {
                // Handle file change events
                file_event = file_change_receiver.recv() => {
                    match file_event {
                        Some(event) => {
                            if let Err(e) = Self::handle_file_change_static(event, &tracked_projects, &status, &outgoing_channels, &server_write_flags, &base_hashes).await {
                                error!("Error handling file change: {}", e);
                            }
                        }
                        None => {
                            info!("File change channel closed");
                            break;
                        }
                    }
                }
                
                // Handle daemon commands from system tray
                command = command_receiver.recv() => {
                    match command {
                        Some(DaemonCommand::ForceSync) => {
                            info!("Processing force sync command from system tray");
                            match Self::force_sync_all_static(&tracked_projects, &status, &outgoing_channels, &base_hashes).await {
                                Ok(count) => {
                                    info!("Force sync completed: {} files synced", count);
                                }
                                Err(e) => {
                                    error!("Force sync failed: {}", e);
                                }
                            }
                        }
                        Some(DaemonCommand::Stop) => {
                            info!("Processing stop tracking command from system tray");
                            // Stop tracking all projects but keep daemon running
                            Self::stop_all_tracking_static(&tracked_projects, &websocket_listeners, &outgoing_channels).await;
                            info!("Stopped tracking all projects - daemon still running");
                        }
                        Some(DaemonCommand::Restart) => {
                            info!("Processing restart command from system tray");
                            // Implement self-restart by spawning a new instance
                            match MothershipDaemon::restart_daemon().await {
                                Ok(()) => {
                                    info!("New daemon instance started, shutting down current instance");
                                    break;
                                }
                                Err(e) => {
                                    error!("Failed to restart daemon: {}", e);
                                    // Continue running the current instance
                                }
                            }
                        }
                        None => {
                            info!("Command channel closed");
                            break;
                        }
                    }
                }
            }
        }
        
        // Clean shutdown
        info!("Shutting down Mothership Daemon...");
        
        // Update status
        {
            let mut status_guard = status.write().await;
            status_guard.is_running = false;
        }
        
        // Stop all persistent WebSocket connections
        {
            let mut listeners = websocket_listeners.write().await;
            for (project_id, handle) in listeners.drain() {
                handle.abort();
                info!("Stopped persistent WebSocket for project {}", project_id);
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
        
        info!("Mothership Daemon shutdown complete");
        Ok(())
    }
    
    /// Handle a file change event (static version for use after moving fields)
    async fn handle_file_change_static(
        event: FileChangeEvent,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        status: &Arc<RwLock<DaemonStatus>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        server_write_flags: &Arc<RwLock<HashMap<Uuid, bool>>>,
        base_hashes: &BaseHashes,
    ) -> Result<()> {
        {
            let flags = server_write_flags.read().await;
            if flags.get(&event.project_id).copied().unwrap_or(false) {
                debug!("Skipping file change event during server write: {} in project {}", 
                    event.file_path.display(), event.project_id);
                return Ok(());
            }
        }
        
        // Get project info for better logging
        let project_name = {
            let projects = tracked_projects.read().await;
            projects.get(&event.project_id)
                .map(|p| p.project_name.clone())
                .unwrap_or_else(|| event.project_id.to_string())
        };
        
        info!("File {:?}: {} ({} bytes) in project '{}'", 
            event.change_type, event.file_path.display(), event.file_size, project_name);
        
        // Update sync status
        {
            let mut status_guard = status.write().await;
            status_guard.files_syncing += 1;
            status_guard.last_sync = Some(chrono::Utc::now());
        }
        
        // PERSISTENT WEBSOCKET: Send file change via persistent connection
        let sync_result = Self::send_file_change_via_persistent_websocket(&event, tracked_projects, outgoing_channels, base_hashes).await;
        match sync_result {
            Ok(()) => {
                info!("Successfully queued file change for persistent WebSocket");
                status.write().await.files_synced += 1;
            }
            Err(e) => {
                error!("Failed to queue file change for persistent WebSocket: {}", e);
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
        Self::handle_file_change_static(event, &self.tracked_projects, &self.status, &self.outgoing_channels, &self.server_write_flags, &self.base_hashes).await
    }
    
    /// Send file change via persistent WebSocket connection
    async fn send_file_change_via_persistent_websocket(
        event: &FileChangeEvent,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &BaseHashes,
    ) -> Result<()> {
        // Check if file requires streaming (empty content indicates streaming file)
        if event.content.is_empty() && event.file_size > 0 {
            return Self::send_file_via_streaming(event, tracked_projects, outgoing_channels, base_hashes).await;
        }
        
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
        
        // Base is the content this client last synced for the path; the server rejects edits made against a stale base
        let norm_rel = mothership_common::normalize_rel_path(&event.file_path);
        let norm_path = PathBuf::from(&norm_rel);
        let base_key = (event.project_id, norm_rel);
        let base = base_hashes.read().await.get(&base_key).cloned();
        let base_hash = base.as_ref().map(|b| b.hash.clone());

        // Content already matches the last synced base (e.g. a watcher echo of a write we just applied): nothing new to send
        if base_hash.as_deref() == Some(content_hash(&event.content).as_str()) {
            debug!("Skipping unchanged content for {}", event.file_path.display());
            return Ok(());
        }

        // Send only the changed lines when we still hold the base content. Anything the diff
        // cannot represent exactly falls back to full content.
        let line_diff = base
            .as_ref()
            .and_then(|b| b.content.as_deref())
            .map(|old| DiffEngine::new().generate_line_diff(old, &event.content))
            .filter(|diff| matches!(diff, FileDiff::LineDiff { .. }));

        let sync_message = match line_diff {
            Some(diff) => SyncMessage::FileDiffChanged {
                rift_id,
                path: norm_path,
                diff,
                file_size: event.content.len() as u64,
                timestamp: event.timestamp,
                base_hash,
            },
            None => SyncMessage::FileChanged {
                rift_id,
                path: norm_path,
                content: event.content.clone(),
                timestamp: event.timestamp,
                base_hash,
            },
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

        // The local file now matches what we just sent, so it becomes the new base
        base_hashes.write().await.insert(base_key, SyncBase::from_content(&event.content));
        
        Ok(())
    }

    /// Scans `project_path` and queues any local file edits that haven't been synced yet on the current WebSocket.
    pub async fn flush_unsynced_project_files(
        project_id: Uuid,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &BaseHashes,
    ) {
        if !outgoing_channels.read().await.contains_key(&project_id) {
            return;
        }
        let has_bases = base_hashes.read().await.keys().any(|(id, _)| *id == project_id);
        if !has_bases {
            return;
        }
        let Some(project_path) = tracked_projects.read().await.get(&project_id).map(|p| p.project_path.clone()) else {
            return;
        };
        let Ok(report) = mothership_common::client::scan_text_files(&project_path, MAX_DIRECT_SYNC_SIZE) else {
            return;
        };
        for (rel_path, content) in report.files {
            let norm = mothership_common::normalize_rel_path(&rel_path);
            let base_key = (project_id, norm.clone());
            let current_hash = content_hash(&content);
            let matches_base = base_hashes
                .read()
                .await
                .get(&base_key)
                .is_some_and(|b| b.hash == current_hash);
            if !matches_base {
                let event = FileChangeEvent {
                    project_id,
                    file_path: PathBuf::from(&norm),
                    content,
                    file_size: 0,
                    timestamp: chrono::Utc::now(),
                    change_type: crate::file_watcher::FileChangeType::Modified,
                };
                if let Err(e) = Self::send_file_change_via_persistent_websocket(
                    &event,
                    tracked_projects,
                    outgoing_channels,
                    base_hashes,
                )
                .await
                {
                    warn!("Failed to flush unsynced file {} before rift switch: {}", norm, e);
                } else {
                    info!("Flushed unsynced local file {} before rift switch", norm);
                }
            }
        }
    }
    
    /// Handle large file streaming
    async fn send_file_via_streaming(
        event: &FileChangeEvent,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &BaseHashes,
    ) -> Result<()> {
        // Get project metadata to determine rift_id and construct full file path
        let (rift_id, full_path) = {
            let projects = tracked_projects.read().await;
            let project = projects.get(&event.project_id)
                .ok_or_else(|| anyhow::anyhow!("Project not found in tracked projects: {}", event.project_id))?;
            
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
                event.project_id // Fallback to project_id
            };
            
            let full_path = project.project_path.join(&event.file_path);
            (rift_id, full_path)
        };
        
        info!("Starting file stream for {} ({} bytes)", event.file_path.display(), event.file_size);
        
        // Generate unique stream ID
        let stream_id = uuid::Uuid::new_v4();
        
        // Stream configuration - using reasonable defaults
        let config = StreamConfig {
            chunk_size: 64 * 1024, // 64KB chunks
            max_concurrent_streams: 10,
            streaming_threshold: 10 * 1024 * 1024,
            compression_threshold: 1024,
            timeout_seconds: 300, // 5 minutes
            temp_dir: std::env::temp_dir(),
            compression_enabled: true,
            hash_verification: true,
        };
        
        // Send stream start message
        let base_key = (event.project_id, event.file_path.to_string_lossy().to_string());
        let base_hash = base_hashes.read().await.get(&base_key).map(|b| b.hash.clone());
        let start_message = SyncMessage::StartFileStream {
            stream_id,
            rift_id,
            path: event.file_path.clone(),
            total_size: event.file_size,
            chunk_size: config.chunk_size,
            content_hash: String::new(), // Will be calculated during streaming
            is_binary: !event.file_path.to_string_lossy().ends_with(".txt"), // Simple heuristic
            timestamp: event.timestamp,
            base_hash,
        };
        
        // Send start message
        {
            let channels = outgoing_channels.read().await;
            let sender = channels.get(&event.project_id)
                .ok_or_else(|| anyhow::anyhow!("No persistent WebSocket connection found for project {}", event.project_id))?;
            
            sender.send(start_message)
                .map_err(|e| anyhow::anyhow!("Failed to send stream start message: {}", e))?;
        }
        
        // Read and stream file in chunks
        let result = Self::stream_file_chunks(
            stream_id,
            &full_path,
            &config,
            event.project_id,
            outgoing_channels
        ).await;
        
        // Send end message
        let end_message = SyncMessage::EndFileStream {
            stream_id,
            final_hash: match result {
                Ok(ref hash) => hash.clone(),
                Err(_) => String::new(),
            },
            success: result.is_ok(),
        };
        
        {
            let channels = outgoing_channels.read().await;
            if let Some(sender) = channels.get(&event.project_id) {
                sender.send(end_message)
                    .map_err(|e| anyhow::anyhow!("Failed to send stream end message: {}", e))?;
            }
        }
        
        match result {
            Ok(ref hash) => {
                info!("File stream completed for {}", event.file_path.display());
                // The streamed file now matches the server's copy, so it becomes the new base
                base_hashes.write().await.insert(base_key, SyncBase::hash_only(hash.clone()));
            }
            Err(ref e) => error!("File stream failed for {}: {}", event.file_path.display(), e),
        }
        
        result.map(|_| ())
    }
    
    /// Stream file in chunks
    async fn stream_file_chunks(
        stream_id: uuid::Uuid,
        file_path: &std::path::Path,
        config: &StreamConfig,
        project_id: uuid::Uuid,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    ) -> Result<String> {
        use std::io::Read;
        
        // Open file for reading
        let mut file = std::fs::File::open(file_path)
            .map_err(|e| anyhow::anyhow!("Failed to open file for streaming: {}", e))?;
        
        let mut hasher = sha2::Sha256::new();
        let mut chunk_index = 0;
        let mut buffer = vec![0u8; config.chunk_size as usize];
        
        loop {
            // Read chunk from file
            let bytes_read = file.read(&mut buffer)
                .map_err(|e| anyhow::anyhow!("Failed to read chunk from file: {}", e))?;
            
            if bytes_read == 0 {
                break; // End of file
            }
            
            let chunk_data = &buffer[..bytes_read];
            
            // Update hash
            use sha2::Digest;
            hasher.update(chunk_data);
            
            // Compress chunk if enabled
            let compressed_data = if config.compression_enabled {
                // Simple compression - use deflate algorithm
                use std::io::Write;
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(chunk_data).map_err(|e| anyhow::anyhow!("Compression failed: {}", e))?;
                encoder.finish().map_err(|e| anyhow::anyhow!("Compression finish failed: {}", e))?
            } else {
                chunk_data.to_vec()
            };
            
            // Create chunk message
            let chunk_message = SyncMessage::FileChunk {
                stream_id,
                chunk_index,
                data: compressed_data,
                is_compressed: config.compression_enabled,
                chunk_hash: format!("{:x}", sha2::Sha256::digest(chunk_data)), // Calculate chunk hash
            };
            
            // Send chunk
            {
                let channels = outgoing_channels.read().await;
                let sender = channels.get(&project_id)
                    .ok_or_else(|| anyhow::anyhow!("WebSocket connection lost during streaming"))?;
                
                sender.send(chunk_message)
                    .map_err(|e| anyhow::anyhow!("Failed to send chunk {}: {}", chunk_index, e))?;
            }
            
            debug!("Sent chunk {} ({} bytes, compressed: {})", 
                chunk_index, bytes_read, config.compression_enabled);
            
            chunk_index += 1;
        }
        
        // Calculate final hash
        use sha2::Digest;
        let hash = format!("{:x}", hasher.finalize());
        Ok(hash)
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
        info!("Registering project for tracking: '{}' at {} ({})", 
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
            self.server_write_flags.clone(),
            self.base_hashes.clone(),
        ).await {
            error!("Failed to start persistent WebSocket for project '{}': {}", project_name, e);
            // Don't fail the entire operation if WebSocket fails
        }
        
        info!("Successfully registered project '{}' for tracking", project_name);
        Ok(())
    }
    
    /// Remove a project from tracking (called via IPC from CLI/GUI)
    pub async fn remove_project(&self, project_id: Uuid) -> Result<()> {
        info!("Unregistering project from tracking: {}", project_id);
        
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
                info!("Stopped file watching for project '{}'", project_name);
            } else {
                warn!("Project '{}' was not being watched", project_name);
            }
        }
        
        // Stop persistent WebSocket connection
        {
            let mut listeners = self.websocket_listeners.write().await;
            if let Some(handle) = listeners.remove(&project_id) {
                handle.abort();
                info!("Stopped persistent WebSocket for project '{}'", project_name);
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
        
        info!("Successfully unregistered project '{}' from tracking", project_name);
        Ok(())
    }
    
    /// Check if a project is being tracked
    pub async fn is_project_tracked(&self, project_id: Uuid) -> bool {
        let projects = self.tracked_projects.read().await;
        projects.contains_key(&project_id)
    }
    
    /// Force sync all tracked projects
    pub async fn force_sync_all(&self) -> Result<usize> {
        Self::force_sync_all_static(&self.tracked_projects, &self.status, &self.outgoing_channels, &self.base_hashes).await
    }
    
    /// Force sync all tracked projects (static version)
    async fn force_sync_all_static(
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        status: &Arc<RwLock<DaemonStatus>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &BaseHashes,
    ) -> Result<usize> {
        info!("Force syncing all tracked projects...");
        
        let projects = tracked_projects.read().await.clone();
        let mut sync_count = 0;
        
        for (project_id, project) in projects.iter() {
            info!("Force syncing project: {} at {}", project.project_name, project.project_path.display());
            
            // Update sync status
            {
                let mut status_guard = status.write().await;
                status_guard.files_syncing += 1;
                status_guard.last_sync = Some(chrono::Utc::now());
            }
            
            match Self::force_sync_project_static(project_id, project, tracked_projects, outgoing_channels, base_hashes).await {
                Ok(file_count) => {
                    info!("Successfully synced {} files for project '{}'", file_count, project.project_name);
                    sync_count += file_count;
                    status.write().await.files_synced += file_count;
                }
                Err(e) => {
                    error!("Failed to sync project '{}': {}", project.project_name, e);
                }
            }
            
            // Update sync status
            {
                let mut status_guard = status.write().await;
                status_guard.files_syncing = status_guard.files_syncing.saturating_sub(1);
            }
        }
        
        info!("Force sync completed: {} files synced across {} projects", sync_count, projects.len());
        Ok(sync_count)
    }
    
    /// Force sync a single project by scanning all its files
    async fn force_sync_project(&self, project_id: &Uuid, project: &TrackedProject) -> Result<usize> {
        Self::force_sync_project_static(project_id, project, &self.tracked_projects, &self.outgoing_channels, &self.base_hashes).await
    }
    
    /// Force sync a single project by scanning all its files (static version)
    async fn force_sync_project_static(
        project_id: &Uuid, 
        project: &TrackedProject,
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &BaseHashes,
    ) -> Result<usize> {
        let mut file_count = 0;
        
        // Walk through all files in the project directory
        let mut entries = tokio::fs::read_dir(&project.project_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            // Skip hidden directories and files (like .git, .mothership)
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            
            if path.is_file() {
                // Read file content
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    // Create a synthetic file change event to trigger sync
                    let relative_path = path.strip_prefix(&project.project_path)
                        .unwrap_or(&path)
                        .to_path_buf();
                    
                    let event = FileChangeEvent {
                        project_id: *project_id,
                        file_path: relative_path,
                        change_type: crate::file_watcher::FileChangeType::Modified,
                        content,
                        file_size: entry.metadata().await.map(|m| m.len()).unwrap_or(0),
                        timestamp: chrono::Utc::now(),
                    };
                    
                    // Send the change via the existing sync mechanism
                    if let Err(e) = Self::send_file_change_via_persistent_websocket(&event, tracked_projects, outgoing_channels, base_hashes).await {
                        warn!("Failed to sync file {}: {}", path.display(), e);
                    } else {
                        file_count += 1;
                    }
                }
            } else if path.is_dir() {
                // Recursively sync subdirectories
                match Self::force_sync_directory_static(&path, project_id, &project.project_path, tracked_projects, outgoing_channels, base_hashes).await {
                    Ok(sub_count) => file_count += sub_count,
                    Err(e) => warn!("Failed to sync directory {}: {}", path.display(), e),
                }
            }
        }
        
        Ok(file_count)
    }
    
    /// Recursively force sync a directory
    async fn force_sync_directory(&self, dir_path: &std::path::Path, project_id: &Uuid, project_root: &std::path::Path) -> Result<usize> {
        Self::force_sync_directory_static(dir_path, project_id, project_root, &self.tracked_projects, &self.outgoing_channels, &self.base_hashes).await
    }
    
    /// Recursively force sync a directory (static version with boxing to handle recursion)
    fn force_sync_directory_static<'a>(
        dir_path: &'a std::path::Path, 
        project_id: &'a Uuid, 
        project_root: &'a std::path::Path,
        tracked_projects: &'a Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        outgoing_channels: &'a Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        base_hashes: &'a BaseHashes,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<usize>> + Send + 'a>> {
        Box::pin(async move {
            let mut file_count = 0;
            
            let mut entries = tokio::fs::read_dir(dir_path).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                
                // Skip hidden directories and files
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') {
                        continue;
                    }
                }
                
                if path.is_file() {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        let relative_path = path.strip_prefix(project_root)
                            .unwrap_or(&path)
                            .to_path_buf();
                        
                        let event = FileChangeEvent {
                            project_id: *project_id,
                            file_path: relative_path,
                            change_type: crate::file_watcher::FileChangeType::Modified,
                            content,
                            file_size: entry.metadata().await.map(|m| m.len()).unwrap_or(0),
                            timestamp: chrono::Utc::now(),
                        };
                        
                        if let Err(e) = Self::send_file_change_via_persistent_websocket(&event, tracked_projects, outgoing_channels, base_hashes).await {
                            warn!("Failed to sync file {}: {}", path.display(), e);
                        } else {
                            file_count += 1;
                        }
                    }
                } else if path.is_dir() {
                    match Self::force_sync_directory_static(&path, project_id, project_root, tracked_projects, outgoing_channels, base_hashes).await {
                        Ok(sub_count) => file_count += sub_count,
                        Err(e) => warn!("Failed to sync subdirectory {}: {}", path.display(), e),
                    }
                }
            }
            
            Ok(file_count)
        })
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
                        info!("Server connectivity check passed");
                    } else {
                        warn!("Server responded with status: {}", response.status());
                    }
                    is_ok
                }
                Err(e) => {
                    warn!("Server connectivity check failed: {}", e);
                    false
                }
            }
        } else {
            warn!("No active server URL found");
            false
        }
    }
    
    /// Start a persistent WebSocket listener for a project
    pub async fn start_websocket_listener(
        project_id: Uuid,
        tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        status: Arc<RwLock<DaemonStatus>>,
        websocket_listeners: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
        outgoing_channels: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        server_write_flags: Arc<RwLock<HashMap<Uuid, bool>>>,
        base_hashes: BaseHashes,
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
        
        debug!("Loaded auth token: {}...", &auth_token.chars().take(10).collect::<String>());
        
        // Get server URL
        let server_url = get_active_server_url()
            .ok_or_else(|| anyhow::anyhow!("No active server connection found"))?;
        
        debug!("Active server URL: {}", server_url);
        
        // Construct WebSocket URL
        let ws_url = if server_url.starts_with("https://") {
            let ws_base = server_url.replace("https://", "wss://");
            format!("{}/ws/{}?token={}", ws_base, rift_id, urlencoding::encode(&auth_token))
        } else if server_url.starts_with("http://") {
            let ws_base = server_url.replace("http://", "ws://");
            format!("{}/ws/{}?token={}", ws_base, rift_id, urlencoding::encode(&auth_token))
        } else {
            format!("wss://{}/ws/{}?token={}", server_url, rift_id, urlencoding::encode(&auth_token))
        };
        
        info!("Starting persistent WebSocket connection for project {} (rift: {})", project_id, rift_id);
        info!("WebSocket URL: {}", ws_url.replace(&auth_token, "***TOKEN***"));
        
        // Stop any existing WebSocket listener for this project before starting a new one
        {
            let mut listeners = websocket_listeners.write().await;
            if let Some(previous) = listeners.remove(&project_id) {
                previous.abort();
            }
        }

        // Create channel for outgoing messages
        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<SyncMessage>();
        {
            let mut channels = outgoing_channels.write().await;
            channels.insert(project_id, outgoing_tx);
        }

        let (initial_sync_tx, initial_sync_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

        let status_clone = status.clone();
        let listener_handle = tokio::spawn(async move {
            let mut initial_sync_tx = Some(initial_sync_tx);
            let ping_interval = Duration::from_secs(30);
            let health_log_interval = Duration::from_secs(300);
            let reconnect_delay = Duration::from_secs(5);
            let mut health = ConnectionHealth::new();
            
            // Set when the outgoing queue is closed and fully drained, so the listener exits instead of reconnecting
            let mut channel_closed = false;
            // CRITICAL FIX: Add reconnection loop
            loop {
                let mut next_ping = Instant::now() + ping_interval;
                let mut next_health_log = Instant::now() + health_log_interval;
                
                info!("Connecting to WebSocket: {}", ws_url);
                
                // CRITICAL FIX: Actually connect to the WebSocket server!
                match tokio_tungstenite::connect_async(&ws_url).await {
                    Ok((ws_stream, response)) => {
                        info!("WebSocket connected successfully!");
                        debug!("WebSocket response status: {}", response.status());
                        debug!("WebSocket response headers: {:?}", response.headers());
                        
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
                            debug!("Sending join message: {}", join_json);
                            if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Text(join_json)).await {
                                error!("Failed to send join message: {}", e);
                            } else {
                                info!("Sent rift join message");
                            }
                        }
                        
                        loop {
                            tokio::select! {
                                // Handle outgoing messages (from file watcher)
                                msg = outgoing_rx.recv() => {
                                    match msg {
                                        Some(mut sync_msg) => {
                                            // Always stamp outgoing changes with the rift_id this socket is connected to,
                                            // even if project.json on disk was already updated for an upcoming rift switch.
                                            match &mut sync_msg {
                                                SyncMessage::FileChanged { rift_id: msg_rift, .. }
                                                | SyncMessage::FileDiffChanged { rift_id: msg_rift, .. }
                                                | SyncMessage::StartFileStream { rift_id: msg_rift, .. } => {
                                                    *msg_rift = rift_id;
                                                }
                                                _ => {}
                                            }
                                            if let Ok(json) = serde_json::to_string(&sync_msg) {
                                                if let Err(e) = ws_sender.send(tokio_tungstenite::tungstenite::Message::Text(json)).await {
                                                    error!("Failed to send WebSocket message: {}", e);
                                                    health.record_error();
                                                    break;
                                                } else {
                                                    health.record_message_sent();
                                                    debug!("Sent sync message to server");
                                                }
                                            }
                                        }
                                        None => {
                                            info!("Outgoing channel closed, stopping WebSocket");
                                            let close_frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                                                reason: "Reconnecting".into(),
                                            };
                                            let _ = ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(Some(close_frame))).await;
                                            let _ = ws_sender.close().await;
                                            tokio::time::sleep(Duration::from_millis(100)).await;
                                            channel_closed = true;
                                            break;
                                        }
                                    }
                                }
                                
                                // Handle incoming messages (from server)
                                msg = ws_receiver.next() => {
                                    match msg {
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                            health.record_message_received();
                                            debug!("Received WebSocket message: {} chars", text.len());
                                            let is_rift_joined = text.contains("\"RiftJoined\"");
                                            
                                            // Handle incoming sync message
                                            let res = Self::handle_websocket_sync_message(&text, &project_path, &server_write_flags, &base_hashes, &outgoing_channels, project_id).await;
                                            if let Err(ref e) = res {
                                                error!("Failed to handle incoming sync message: {}", e);
                                            }
                                            // Handlers set this flag and clear it on success only; a failed write would otherwise leave local changes ignored
                                            server_write_flags.write().await.remove(&project_id);

                                            if is_rift_joined {
                                                if let Some(tx) = initial_sync_tx.take() {
                                                    let _ = tx.send(res.map_err(|e| e.to_string()));
                                                }
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
                                                debug!("Sent pong response");
                                            }
                                        }
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(_))) => {
                                            debug!("Received pong");
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
                                            debug!("Sent ping");
                                            health.record_message_sent();
                                        }
                                    }
                                    next_ping = Instant::now() + ping_interval;
                                }

                                // Log connection health periodically
                                _ = sleep_until(next_health_log) => {
                                    info!("Connection health: {}", health.get_health_report());
                                    next_health_log = Instant::now() + health_log_interval;
                                }
                            }
                        }
                        
                        info!("WebSocket connection closed");
                    }
                    Err(e) => {
                        error!("Failed to connect to WebSocket: {}", e);
                        
                        // Log more specific error details based on error string
                        let error_str = e.to_string();
                        
                        if error_str.contains("401") {
                            error!("  Authentication failed - token may be invalid or expired");
                            error!("  Try running 'mothership auth' to refresh your credentials");
                        } else if error_str.contains("404") {
                            error!("  WebSocket endpoint not found - rift may not exist");
                            error!("  Rift ID: {}", rift_id);
                            {
                                let mut projects = tracked_projects.write().await;
                                projects.remove(&project_id);
                                save_persisted_projects(&projects);
                                status_clone.write().await.projects_tracked = projects.len();
                            }
                            channel_closed = true;
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
                        if let Some(tx) = initial_sync_tx.take() {
                            let _ = tx.send(Err(error_str));
                        }
                    }
                }
                
                if channel_closed {
                    info!("Queued changes sent, WebSocket listener stopped");
                    break;
                }

                // Wait before reconnecting
                info!(" Waiting {} seconds before reconnecting...", reconnect_delay.as_secs());
                tokio::time::sleep(reconnect_delay).await;
                
                // Reset health on reconnection attempt
                health.record_reset();
            } // End of reconnection loop
        }); // End of spawned task

        // Store the handle for later cleanup
        {
            let mut listeners = websocket_listeners.write().await;
            listeners.insert(project_id, listener_handle);
        }
        
        info!("WebSocket listener started for project {}; waiting for initial rift sync...", project_id);
        match tokio::time::timeout(Duration::from_secs(15), initial_sync_rx).await {
            Ok(Ok(Ok(()))) => {
                info!("Initial rift sync complete for project {}", project_id);
            }
            Ok(Ok(Err(e))) => {
                warn!("Initial rift sync reported error for project {}: {}", project_id, e);
            }
            Ok(Err(_)) => {
                warn!("Initial rift sync channel closed early for project {}", project_id);
            }
            Err(_) => {
                warn!("Timed out waiting for initial rift sync for project {}", project_id);
            }
        }
        Ok(())
    }
    
    /// Handle incoming sync messages from the server
    async fn handle_incoming_sync_message(text: &str, project_path: &PathBuf, state: &Arc<MothershipDaemon>) -> Result<()> {
        let sync_message: SyncMessage = serde_json::from_str(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse sync message: {}", e))?;
        
        match sync_message {
            SyncMessage::FileChanged { path, content, .. } => {
                info!("Received file change: {} ({} bytes)", path.display(), content.len());
                
                // Write the file to disk
                let file_path = project_path.join(&path);
                
                // Create parent directories if needed
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                
                // Write file content
                tokio::fs::write(&file_path, &content).await?;
                info!("Wrote incoming file change: {}", path.display());
                Ok(())
            }
            SyncMessage::FilesChanged { changes, .. } => {
                info!("Received {} file changes", changes.len());
                
                for change in changes {
                    // For now, we'll need to get the content from the server
                    // This is a simplified implementation
                    info!("File change: {} ({:?})", change.path.display(), change.change_type);
                }
                Ok(())
            }
            SyncMessage::RiftJoined { current_files, .. } => {
                info!("Received current rift state with {} files", current_files.len());
                
                // Write all current files (initial sync)
                for (path, content) in current_files {
                    let file_path = project_path.join(&path);
                    
                    // Create parent directories if needed
                    if let Some(parent) = file_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    
                    // Write file content
                    tokio::fs::write(&file_path, &content).await?;
                    info!("Wrote initial file: {}", path.display());
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
                info!("Conflict detected for {}, accepting server version", path.display());
                
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
                info!("Begin transaction from {}: {}", author, description);
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
                info!("Conflict rift '{}' created successfully", conflict_rift_name);
                info!("Use 'mothership beam \"{}\"' to work on your changes", conflict_rift_name);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Handle WebSocket sync message (simplified version for static context)
    async fn handle_websocket_sync_message(
        text: &str, 
        project_path: &PathBuf,
        server_write_flags: &Arc<RwLock<HashMap<Uuid, bool>>>,
        base_hashes: &BaseHashes,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        project_id: Uuid,
    ) -> Result<()> {
        let sync_message: SyncMessage = serde_json::from_str(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse sync message: {}", e))?;
        
        match sync_message {
            SyncMessage::FileChanged { path, content, .. } => {
                let Some(rel_path) = mothership_common::beam::safe_relative_path(&path) else {
                    warn!("Skipping unsafe path from server: {}", path.display());
                    return Ok(());
                };
                let norm = mothership_common::normalize_rel_path(&rel_path);
                info!("Received file change from collaborator: {} ({} bytes)", norm, content.len());
                
                // Set server write flag to prevent file watcher loops
                {
                    let mut flags = server_write_flags.write().await;
                    flags.insert(project_id, true);
                }
                
                base_hashes.write().await.insert((project_id, norm.clone()), SyncBase::from_content(&content));

                // Write the file to disk
                let file_path = project_path.join(&rel_path);
                
                // Create parent directories if needed
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                
                // Write file content
                tokio::fs::write(&file_path, &content).await?;
                info!("Applied file change from collaborator: {}", norm);
                tokio::time::sleep(Duration::from_millis(120)).await;
                {
                    let mut flags = server_write_flags.write().await;
                    flags.remove(&project_id);
                }
                
                Ok(())
            }
            SyncMessage::RiftDiffUpdate { diff_changes, .. } => {
                info!("Received {} diff updates from collaborator", diff_changes.len());
                
                // Set server write flag to prevent file watcher loops
                {
                    let mut flags = server_write_flags.write().await;
                    flags.insert(project_id, true);
                }
                
                // Apply all diffs
                for change in diff_changes {
                    let Some(rel_path) = mothership_common::beam::safe_relative_path(&change.path) else {
                        continue;
                    };
                    let norm = mothership_common::normalize_rel_path(&rel_path);
                    let file_path = project_path.join(&rel_path);
                    
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
                            base_hashes.write().await.insert((project_id, norm.clone()), SyncBase::from_content(&new_content));
                            // Create parent directories if needed
                            if let Some(parent) = file_path.parent() {
                                tokio::fs::create_dir_all(parent).await?;
                            }
                            
                            // Write updated content
                            tokio::fs::write(&file_path, &new_content).await?;
                            info!("Applied diff to {}: {} -> {} bytes", 
                                norm, current_content.len(), new_content.len());
                        }
                        Err(e) => {
                            error!("Failed to apply diff to {}: {}", norm, e);
                        }
                    }
                }
                
                tokio::time::sleep(Duration::from_millis(120)).await;
                // Clear server write flag
                {
                    let mut flags = server_write_flags.write().await;
                    flags.remove(&project_id);
                }
                
                Ok(())
            }
            SyncMessage::RiftJoined { current_files, .. } => {
                info!("Received initial rift state with {} files", current_files.len());
                
                // Set server write flag to prevent file watcher loops
                {
                    let mut flags = server_write_flags.write().await;
                    flags.insert(project_id, true);
                }
                
                // Snapshot the previous rift's synced base hashes before clearing them
                let old_bases: HashMap<String, String> = {
                    let mut bases = base_hashes.write().await;
                    let old = bases
                        .iter()
                        .filter(|((id, _), _)| *id == project_id)
                        .map(|((_, p), b)| (p.clone(), b.hash.clone()))
                        .collect();
                    bases.retain(|(id, _), _| *id != project_id);
                    old
                };

                let recovery_stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
                let mut valid_files: Vec<(PathBuf, String, String)> = Vec::new();
                let mut current_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
                for (raw_path, content) in current_files {
                    let Some(rel_path) = mothership_common::beam::safe_relative_path(&raw_path) else {
                        warn!("Skipping unsafe path in RiftJoined: {}", raw_path.display());
                        continue;
                    };
                    let norm = mothership_common::normalize_rel_path(&rel_path);
                    current_paths.insert(norm.clone());
                    valid_files.push((rel_path, norm, content));
                }

                // Collect candidate stale paths from both previous base_hashes and non-ignored files on disk
                let mut stale_candidates: std::collections::HashSet<String> = old_bases
                    .keys()
                    .filter(|p| !current_paths.contains(*p))
                    .cloned()
                    .collect();
                if let Ok(rules) = mothership_common::ignore::IgnoreRules::load(&project_path) {
                    for entry in walkdir::WalkDir::new(&project_path)
                        .follow_links(false)
                        .into_iter()
                        .filter_entry(|e| !rules.is_ignored(e.path(), e.file_type().is_dir()))
                        .filter_map(|e| e.ok())
                    {
                        let p = entry.path();
                        if !p.is_file() {
                            continue;
                        }
                        if p.file_name().and_then(|n| n.to_str()) == Some(mothership_common::ignore::IGNORE_FILE_NAME) {
                            continue;
                        }
                        if let Ok(rel) = p.strip_prefix(&project_path) {
                            let norm = mothership_common::normalize_rel_path(rel);
                            if !current_paths.contains(&norm) {
                                stale_candidates.insert(norm);
                            }
                        }
                    }
                }

                for stale in stale_candidates {
                    let Some(rel_path) = mothership_common::beam::safe_relative_path(std::path::Path::new(&stale)) else {
                        continue;
                    };
                    let stale_path = project_path.join(&rel_path);
                    if stale_path.is_file() {
                        // Back up if it wasn't already synced to the previous rift
                        let should_backup = match tokio::fs::read_to_string(&stale_path).await {
                            Ok(text) => old_bases.get(&stale) != Some(&content_hash(&text)),
                            Err(_) => true,
                        };
                        if should_backup {
                            let backup = project_path.join(".mothership").join("local-backup").join(&recovery_stamp).join(&rel_path);
                            if let Some(parent) = backup.parent() {
                                tokio::fs::create_dir_all(parent).await?;
                            }
                            let _ = tokio::fs::copy(&stale_path, &backup).await;
                        }
                        match tokio::fs::remove_file(&stale_path).await {
                            Ok(()) => info!("Removed file not in this rift: {}", stale),
                            Err(e) => error!("Failed to remove {}: {}", stale, e),
                        }
                    }
                }

                // Write all current files (initial sync)
                for (rel_path, norm, content) in valid_files {
                    let file_path = project_path.join(&rel_path);
                    
                    if let Some(parent) = file_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    
                    // Only back up local file if it differs from the incoming rift content AND wasn't already synced on the old rift
                    if let Ok(local_bytes) = tokio::fs::read(&file_path).await {
                        if local_bytes.as_slice() != content.as_bytes() {
                            let was_synced_on_old_rift = std::str::from_utf8(&local_bytes)
                                .ok()
                                .is_some_and(|s| old_bases.get(&norm) == Some(&content_hash(s)));
                            if !was_synced_on_old_rift {
                                let backup = project_path
                                    .join(".mothership")
                                    .join("local-backup")
                                    .join(&recovery_stamp)
                                    .join(&rel_path);
                                if let Some(parent) = backup.parent() {
                                    tokio::fs::create_dir_all(parent).await?;
                                }
                                tokio::fs::write(&backup, &local_bytes).await?;
                                warn!("Backed up unsynced local {} to {} before overwriting", norm, backup.display());
                            }
                        }
                    }

                    // Set base_hash BEFORE writing to disk so any watcher echo sees matching base_hash
                    base_hashes.write().await.insert((project_id, norm.clone()), SyncBase::from_content(&content));
                    tokio::fs::write(&file_path, &content).await?;
                    info!("Wrote initial file: {}", norm);
                }
                
                // Keep server_write_flags set while asynchronous OS watcher notifications settle
                tokio::time::sleep(Duration::from_millis(150)).await;
                {
                    let mut flags = server_write_flags.write().await;
                    flags.remove(&project_id);
                }
                
                Ok(())
            }
            SyncMessage::ChatMessageReceived { message, .. } => {
                info!("[{}] {}: {}", message.timestamp.format("%H:%M:%S"), message.author_username, message.content);
                Ok(())
            }
            SyncMessage::FullContentRequired { path, base_hash, .. } => {
                // Server could not apply our diff (stale base). Resend the whole local file with the
                // rejected base so the server keeps our edit in a conflict rift instead of discarding it.
                let Some(rel_path) = mothership_common::beam::safe_relative_path(&path) else {
                    return Ok(());
                };
                let norm = mothership_common::normalize_rel_path(&rel_path);
                let norm_path = PathBuf::from(&norm);
                let file_path = project_path.join(&rel_path);
                let file_size = match tokio::fs::metadata(&file_path).await {
                    Ok(meta) => meta.len(),
                    Err(e) => {
                        warn!("Cannot send full content for {}: {}", norm, e);
                        return Ok(());
                    }
                };
                info!("Server requested full content for {}; resending", norm);

                let rift_id = {
                    let metadata_file = project_path.join(".mothership").join("project.json");
                    let metadata_content = tokio::fs::read_to_string(&metadata_file).await
                        .map_err(|e| anyhow!("Failed to read project metadata: {}", e))?;
                    let metadata: ProjectMetadata = serde_json::from_str(&metadata_content)
                        .map_err(|e| anyhow!("Failed to parse project metadata: {}", e))?;
                    match &metadata.rift_id {
                        Some(id) => Uuid::parse_str(id).map_err(|e| anyhow!("Invalid rift_id in metadata: {}", e))?,
                        None => project_id,
                    }
                };

                let key = (project_id, norm.clone());

                // Large files go through the streaming protocol. The server checks the same base_hash on stream start.
                if file_size > MAX_DIRECT_SYNC_SIZE {
                    let stream_id = uuid::Uuid::new_v4();
                    let config = StreamConfig {
                        chunk_size: 64 * 1024,
                        max_concurrent_streams: 10,
                        streaming_threshold: 10 * 1024 * 1024,
                        compression_threshold: 1024,
                        timeout_seconds: 300,
                        temp_dir: std::env::temp_dir(),
                        compression_enabled: true,
                        hash_verification: true,
                    };
                    let start_message = SyncMessage::StartFileStream {
                        stream_id,
                        rift_id,
                        path: norm_path,
                        total_size: file_size,
                        chunk_size: config.chunk_size,
                        content_hash: String::new(),
                        is_binary: !norm.ends_with(".txt"),
                        timestamp: chrono::Utc::now(),
                        base_hash,
                    };
                    {
                        let channels = outgoing_channels.read().await;
                        let sender = channels.get(&project_id)
                            .ok_or_else(|| anyhow!("No persistent WebSocket connection found for project {}", project_id))?;
                        sender.send(start_message).map_err(|e| anyhow!("Failed to send stream start message: {}", e))?;
                    }
                    let result = Self::stream_file_chunks(stream_id, &file_path, &config, project_id, outgoing_channels).await;
                    {
                        let channels = outgoing_channels.read().await;
                        if let Some(sender) = channels.get(&project_id) {
                            let _ = sender.send(SyncMessage::EndFileStream {
                                stream_id,
                                final_hash: result.as_ref().map(|h| h.clone()).unwrap_or_default(),
                                success: result.is_ok(),
                            });
                        }
                    }
                    if let Ok(hash) = &result {
                        base_hashes.write().await.insert(key, SyncBase::hash_only(hash.clone()));
                    }
                    return result.map(|_| ());
                }

                let content = match tokio::fs::read_to_string(&file_path).await {
                    Ok(content) => content,
                    Err(e) => {
                        warn!("Cannot send full content for {}: {}", norm, e);
                        return Ok(());
                    }
                };
                let sender_result = {
                    let channels = outgoing_channels.read().await;
                    match channels.get(&project_id) {
                        Some(sender) => sender.send(SyncMessage::FileChanged {
                            rift_id,
                            path: norm_path,
                            content: content.clone(),
                            timestamp: chrono::Utc::now(),
                            base_hash,
                        }),
                        None => return Err(anyhow!("No persistent WebSocket connection found for project {}", project_id)),
                    }
                };
                sender_result.map_err(|e| anyhow!("Failed to queue full content: {}", e))?;
                base_hashes.write().await.insert(key, SyncBase::from_content(&content));
                Ok(())
            }
            SyncMessage::ForceSync { path, server_content, .. } => {
                // Server replaced our base with its version (e.g. after a stale-edit conflict); adopt it
                let Some(rel_path) = mothership_common::beam::safe_relative_path(&path) else {
                    return Ok(());
                };
                let norm = mothership_common::normalize_rel_path(&rel_path);
                info!("Force syncing {} from server", norm);
                {
                    let mut flags = server_write_flags.write().await;
                    flags.insert(project_id, true);
                }
                base_hashes.write().await.insert((project_id, norm.clone()), SyncBase::from_content(&server_content));
                let file_path = project_path.join(&rel_path);
                if let Some(parent) = file_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                match mothership_common::preserve_local_version(&project_path, &rel_path, server_content.as_bytes()) {
                    Ok(Some(backup)) => warn!("Kept local version of {} at {} before adopting server content", norm, backup.display()),
                    Ok(None) => {}
                    Err(e) => warn!("Could not back up local {} before overwrite: {}", norm, e),
                }
                tokio::fs::write(&file_path, &server_content).await?;
                tokio::time::sleep(Duration::from_millis(120)).await;
                {
                    let mut flags = server_write_flags.write().await;
                    flags.remove(&project_id);
                }
                Ok(())
            }
            SyncMessage::ConflictRiftCreated { conflict_rift_name, new_rift_id, .. } => {
                // The conflict rift is not joined automatically; the user's edit is preserved there
                info!("Edit made against a stale base was saved to conflict rift '{}' ({}); main was kept", conflict_rift_name, new_rift_id);
                Ok(())
            }
            SyncMessage::Heartbeat => {
                debug!("Received heartbeat from server");
                Ok(())
            }
            _ => {
                debug!("Received sync message: {:?} (not handled in WebSocket context)", std::mem::discriminant(&sync_message));
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
        info!("Conflict detected for {}, accepting server version", path.display());
        
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
        info!("Force syncing {} from server", path.display());
        
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
    
    /// Stop tracking all projects but keep daemon running
    pub async fn stop_all_tracking_static(
        tracked_projects: &Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        websocket_listeners: &Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
        outgoing_channels: &Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    ) {
        info!("Stopping all project tracking...");
        
        // Clear tracked projects
        let project_count = {
            let mut projects = tracked_projects.write().await;
            let count = projects.len();
            projects.clear();
            count
        };
        
        // Stop all WebSocket listeners
        {
            let mut listeners = websocket_listeners.write().await;
            for (project_id, handle) in listeners.drain() {
                info!("Stopping WebSocket listener for project {}", project_id);
                handle.abort();
            }
        }
        
        // Clear outgoing channels
        {
            let mut channels = outgoing_channels.write().await;
            channels.clear();
        }
        
        info!("Stopped tracking {} projects", project_count);
    }
    
    /// Stop tracking all projects but keep daemon running (instance method)
    pub async fn stop_all_tracking(&self) {
        Self::stop_all_tracking_static(&self.tracked_projects, &self.websocket_listeners, &self.outgoing_channels).await;
    }
    
    /// Restart the daemon by spawning a new instance and exiting current one
    pub async fn restart_daemon() -> Result<()> {
        info!("Initiating daemon restart...");
        
        // Get current executable path
        let current_exe = std::env::current_exe()
            .map_err(|e| anyhow::anyhow!("Failed to get current executable path: {}", e))?;
        
        info!("Starting new daemon instance: {}", current_exe.display());
        
        // Spawn new daemon instance
        let mut command = std::process::Command::new(&current_exe);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        
        // Start the new process detached from the current one
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000008 | 0x00000200 | 0x08000000); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        }
        
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0); // Create new process group
        }
        
        let child = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn new daemon instance: {}", e))?;
        
        info!("New daemon instance started with PID: {}", child.id());
        
        // Give the new instance time to start up
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        
        Ok(())
    }
}

/// Project metadata structure (must match what's stored by CLI)
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
    #[serde(default)]
    rift_id: Option<String>, // CRITICAL FIX: Read rift_id for WebSocket connection
    #[serde(default)]
    current_rift: Option<String>,
}

/// Get the active server URL (shared with CLI and GUI)
pub(crate) fn get_active_server_url() -> Option<String> {
    Some(mothership_common::client::active_server_url())
}

/// Load stored authentication token for WebSocket connection
pub(crate) fn load_auth_token() -> Option<String> {
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