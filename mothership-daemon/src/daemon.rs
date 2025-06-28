use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::file_watcher::{FileWatcher, FileChangeEvent};
use crate::ipc_server::IpcServer;
use crate::system_tray::SystemTray;

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
        let system_tray = Some(SystemTray::new(status.clone())?);
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
            if let Err(e) = Self::handle_file_change_static(event, &self.tracked_projects, &self.status).await {
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
    ) -> Result<()> {
        // Get project info for better logging
        let project_name = {
            let projects = tracked_projects.read().await;
            projects.get(&event.project_id)
                .map(|p| p.project_name.clone())
                .unwrap_or_else(|| event.project_id.to_string())
        };
        
        info!("📝 File changed: {} in project '{}'", 
            event.file_path.display(), project_name);
        
        // Update sync status
        {
            let mut status_guard = status.write().await;
            status_guard.files_syncing += 1;
            status_guard.last_sync = Some(chrono::Utc::now());
        }
        
        // TODO: Send file change to Mothership server via WebSocket
        // This will be implemented when we integrate with the server sync logic
        
        // Simulate sync processing time
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        
        // Update sync status
        {
            let mut status_guard = status.write().await;
            status_guard.files_syncing = status_guard.files_syncing.saturating_sub(1);
        }
        
        Ok(())
    }

    /// Handle a file change event
    async fn handle_file_change(&self, event: FileChangeEvent) -> Result<()> {
        Self::handle_file_change_static(event, &self.tracked_projects, &self.status).await
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
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.projects_tracked = self.tracked_projects.read().await.len();
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
} 