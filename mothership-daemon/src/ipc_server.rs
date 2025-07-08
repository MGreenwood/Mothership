use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, error};
use uuid::Uuid;

use crate::daemon::{DaemonStatus, TrackedProject};
use crate::file_watcher::FileChangeEvent;
use mothership_common::protocol::SyncMessage;

/// IPC server for communication between CLI/GUI and daemon
pub struct IpcServer {
    /// Daemon status
    status: Arc<RwLock<DaemonStatus>>,
    /// Tracked projects registry
    tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
    /// Channel for sending file change events
    file_change_sender: mpsc::UnboundedSender<FileChangeEvent>,
    /// Active file watchers (CRITICAL: Must be kept alive!)
    file_watchers: Arc<RwLock<HashMap<Uuid, crate::file_watcher::FileWatcher>>>,
    /// Maps project ID to WebSocket listener task handles
    websocket_listeners: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
    /// Maps project ID to outgoing message channels (for sending to WebSocket)
    outgoing_channels: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
    /// Maps project ID to server write flags (prevents file watcher loops)
    server_write_flags: Arc<RwLock<HashMap<Uuid, bool>>>,
}

/// Request to add a project for tracking
#[derive(Debug, Deserialize)]
pub struct AddProjectRequest {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_path: PathBuf,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub daemon_status: DaemonStatus,
}

/// API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub status: String,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self {
            status: "success".to_string(),
            data: Some(data),
            error: None,
        }
    }

    fn error(message: String) -> Self {
        Self {
            status: "error".to_string(),
            data: None,
            error: Some(message),
        }
    }
}

impl IpcServer {
    /// Create a new IPC server
    pub async fn new(
        status: Arc<RwLock<DaemonStatus>>,
        tracked_projects: Arc<RwLock<HashMap<Uuid, TrackedProject>>>,
        file_change_sender: mpsc::UnboundedSender<FileChangeEvent>,
        websocket_listeners: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
        outgoing_channels: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<SyncMessage>>>>,
        server_write_flags: Arc<RwLock<HashMap<Uuid, bool>>>,
    ) -> Result<Self> {
        Ok(Self {
            status,
            tracked_projects,
            file_change_sender,
            file_watchers: Arc::new(RwLock::new(HashMap::new())),
            websocket_listeners,
            outgoing_channels,
            server_write_flags,
        })
    }

    /// Start the IPC server
    pub async fn start(self) -> Result<()> {
        info!("🌐 Starting Mothership Daemon IPC server on port 7525...");

        let app = Router::new()
            .route("/health", get(health_check))
            .route("/status", get(get_status))
            .route("/projects", get(list_projects))
            .route("/projects/add", post(add_project))
            .route("/projects/:id/remove", post(remove_project))
            .route("/shutdown", post(shutdown_daemon))
            .with_state(Arc::new(self));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:7525").await?;
        info!("✅ IPC server listening on http://127.0.0.1:7525");

        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Health check endpoint
async fn health_check(State(server): State<Arc<IpcServer>>) -> Json<HealthResponse> {
    let daemon_status = server.status.read().await.clone();
    
    Json(HealthResponse {
        status: "ok".to_string(),
        service: "mothership-daemon".to_string(),
        daemon_status,
    })
}

/// Get daemon status
async fn get_status(State(server): State<Arc<IpcServer>>) -> Json<ApiResponse<DaemonStatus>> {
    let status = server.status.read().await.clone();
    Json(ApiResponse::success(status))
}

/// List tracked projects
async fn list_projects(State(server): State<Arc<IpcServer>>) -> Json<ApiResponse<Vec<TrackedProject>>> {
    let projects = server.tracked_projects.read().await;
    let project_list: Vec<TrackedProject> = projects.values().cloned().collect();
    Json(ApiResponse::success(project_list))
}

/// Add a project for tracking
async fn add_project(
    State(server): State<Arc<IpcServer>>,
    Json(req): Json<AddProjectRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    info!("📁 Adding project for tracking: {} at {}", req.project_name, req.project_path.display());

    // Validate project path exists
    if !req.project_path.exists() {
        let error_msg = format!("Project path does not exist: {}", req.project_path.display());
        return Ok(Json(ApiResponse::error(error_msg)));
    }
    
    // Validate .mothership directory exists (critical for file watcher)
    let mothership_dir = req.project_path.join(".mothership");
    if !mothership_dir.exists() {
        let error_msg = format!("No .mothership directory found at: {}", req.project_path.display());
        return Ok(Json(ApiResponse::error(error_msg)));
    }

    // Create tracked project
    let tracked_project = TrackedProject {
        project_id: req.project_id,
        project_name: req.project_name.clone(),
        project_path: req.project_path.clone(),
        added_at: chrono::Utc::now(),
    };

    // Add to registry
    {
        let mut projects = server.tracked_projects.write().await;
        projects.insert(req.project_id, tracked_project);
    }

    // Update daemon status
    {
        let mut status = server.status.write().await;
        status.projects_tracked = server.tracked_projects.read().await.len();
    }

    // CRITICAL FIX: Actually start file watcher for this project!
    let file_watcher = match crate::file_watcher::FileWatcher::new(
        req.project_path.clone(),
        req.project_id,
        server.file_change_sender.clone(),
    ).await {
        Ok(watcher) => watcher,
        Err(e) => {
            let error_msg = format!("Failed to start file watcher for '{}': {}", req.project_name, e);
            return Ok(Json(ApiResponse::error(error_msg)));
        }
    };
    
    // CRITICAL: Store the file watcher to keep it alive!
    {
        let mut watchers = server.file_watchers.write().await;
        watchers.insert(req.project_id, file_watcher);
    }
    
    info!("🔍 File watcher started and stored for project '{}'", req.project_name);

    // CRITICAL FIX: Start WebSocket listener for real-time sync
    let websocket_handle = {
        let project_id = req.project_id;
        let tracked_projects = server.tracked_projects.clone();
        let status = server.status.clone();
        let websocket_listeners = server.websocket_listeners.clone();
        let outgoing_channels = server.outgoing_channels.clone();
        let server_write_flags = server.server_write_flags.clone();
        
        tokio::spawn(async move {
            info!("🔄 Starting WebSocket listener for project {}", project_id);
            if let Err(e) = crate::daemon::MothershipDaemon::start_websocket_listener(
                project_id,
                tracked_projects,
                status,
                websocket_listeners,
                outgoing_channels,
                server_write_flags,
            ).await {
                error!("Failed to start WebSocket listener for project {}: {}", project_id, e);
            }
        })
    };
    
    // Store the WebSocket listener handle
    {
        let mut listeners = server.websocket_listeners.write().await;
        listeners.insert(req.project_id, websocket_handle);
    }
    
    info!("🔄 WebSocket listener started for project '{}'", req.project_name);

    info!("✅ Project '{}' added for tracking with active file watcher and WebSocket sync", req.project_name);
    Ok(Json(ApiResponse::success(format!(
        "Project '{}' successfully added for tracking",
        req.project_name
    ))))
}

/// Remove a project from tracking
async fn remove_project(
    State(server): State<Arc<IpcServer>>,
    Path(project_id): Path<Uuid>,
) -> Json<ApiResponse<String>> {
    info!("🗑️ Removing project from tracking: {}", project_id);

    let project_name = {
        let mut projects = server.tracked_projects.write().await;
        projects.remove(&project_id)
            .map(|p| p.project_name)
            .unwrap_or_else(|| project_id.to_string())
    };

    // Update daemon status and check if we should shutdown
    let projects_remaining = {
        let mut status = server.status.write().await;
        let projects_count = server.tracked_projects.read().await.len();
        status.projects_tracked = projects_count;
        projects_count
    };

    // CRITICAL: Remove file watcher to stop watching
    {
        let mut watchers = server.file_watchers.write().await;
        if watchers.remove(&project_id).is_some() {
            info!("🔍 Stopped file watcher for project '{}'", project_name);
        }
    }

    // CRITICAL: Remove WebSocket listener to stop sync
    {
        let mut listeners = server.websocket_listeners.write().await;
        if let Some(handle) = listeners.remove(&project_id) {
            handle.abort();
            info!("🔄 Stopped WebSocket listener for project '{}'", project_name);
        }
    }

    info!("✅ Project '{}' removed from tracking", project_name);
    
    // If no projects remain, automatically shutdown the daemon
    if projects_remaining == 0 {
        info!("🔄 No projects remaining - initiating automatic daemon shutdown...");
        
        // Schedule shutdown after a brief delay to allow response to be sent
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            info!("💤 Auto-shutdown: No projects to track - daemon stopping...");
            std::process::exit(0);
        });
        
        Json(ApiResponse::success(format!(
            "Project '{}' removed. No projects remaining - daemon will auto-shutdown in 1 second",
            project_name
        )))
    } else {
        Json(ApiResponse::success(format!(
            "Project '{}' successfully removed from tracking ({} projects remaining)",
            project_name, projects_remaining
        )))
    }
}

/// Shutdown the daemon gracefully
async fn shutdown_daemon(State(_server): State<Arc<IpcServer>>) -> Json<ApiResponse<String>> {
    info!("🛑 Received shutdown request from CLI");
    
    // Schedule shutdown after a brief delay to allow response to be sent
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        info!("🔄 Initiating graceful shutdown...");
        std::process::exit(0);
    });
    
    Json(ApiResponse::success(
        "Shutdown signal received - daemon will stop in 500ms".to_string()
    ))
} 