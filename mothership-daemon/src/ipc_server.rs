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
use crate::snapshots::SnapshotRegistry;
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
    /// Maps project ID to last-synced content hashes (see daemon::BaseHashes)
    base_hashes: crate::daemon::BaseHashes,
    /// Beamed wormhole snapshot folders awaiting cleanup
    snapshots: Arc<SnapshotRegistry>,
}
#[derive(Debug, Deserialize)]
pub struct AddProjectRequest {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct RegisterSnapshotRequest {
    pub folder: PathBuf,
    pub wormhole_id: Uuid,
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
        base_hashes: crate::daemon::BaseHashes,
        snapshots: Arc<SnapshotRegistry>,
    ) -> Result<Self> {
        Ok(Self {
            status,
            tracked_projects,
            file_change_sender,
            file_watchers: Arc::new(RwLock::new(HashMap::new())),
            websocket_listeners,
            outgoing_channels,
            server_write_flags,
            base_hashes,
            snapshots,
        })
    }

    /// Start the IPC server
    pub async fn start(self) -> Result<()> {
        info!("Starting Mothership Daemon IPC server on port 7525...");

        let state = Arc::new(self);

        // Restore previously tracked projects across daemon restarts
        {
            let restore_state = state.clone();
            tokio::spawn(async move {
                for proj in crate::daemon::load_persisted_projects() {
                    info!("Restoring persisted tracked project '{}' at {}", proj.project_name, proj.project_path.display());
                    let _ = register_project_internal(&restore_state, proj.project_id, proj.project_name, proj.project_path).await;
                }
            });
        }

        let app = Router::new()
            .route("/health", get(health_check))
            .route("/status", get(get_status))
            .route("/projects", get(list_projects))
            .route("/projects/add", post(add_project))
            .route("/projects/:id/remove", post(remove_project))
            .route("/projects/:id/pause", post(pause_project))
            .route("/projects/:id/reconnect", post(reconnect_project))
            .route("/snapshots/register", post(register_snapshot))
            .route("/shutdown", post(shutdown_daemon))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:7525").await?;
        info!("IPC server listening on http://127.0.0.1:7525");

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

/// Internal helper to register/re-register a project with file watcher and WebSocket sync
async fn register_project_internal(
    server: &Arc<IpcServer>,
    project_id: Uuid,
    project_name: String,
    project_path: PathBuf,
) -> std::result::Result<String, String> {
    if !project_path.exists() {
        return Err(format!("Project path does not exist: {}", project_path.display()));
    }
    let mothership_dir = project_path.join(".mothership");
    if !mothership_dir.exists() {
        return Err(format!("No .mothership directory found at: {}", project_path.display()));
    }

    // Drain any existing WebSocket listener for this project before replacing
    let old_listener = server.websocket_listeners.write().await.remove(&project_id);
    server.outgoing_channels.write().await.remove(&project_id);
    if let Some(handle) = old_listener {
        let abort = handle.abort_handle();
        if tokio::time::timeout(std::time::Duration::from_secs(5), handle).await.is_err() {
            abort.abort();
        }
    }

    let rift_id = crate::daemon::load_project_rift_id(&project_path).ok().flatten();
    let tracked_project = TrackedProject {
        project_id,
        project_name: project_name.clone(),
        project_path: project_path.clone(),
        added_at: chrono::Utc::now(),
        rift_id,
    };

    {
        let mut projects = server.tracked_projects.write().await;
        projects.insert(project_id, tracked_project);
        crate::daemon::save_persisted_projects(&projects);
    }

    {
        let mut status = server.status.write().await;
        status.projects_tracked = server.tracked_projects.read().await.len();
    }

    let file_watcher = crate::file_watcher::FileWatcher::new(
        project_path.clone(),
        project_id,
        server.file_change_sender.clone(),
    )
    .await
    .map_err(|e| format!("Failed to start file watcher for '{}': {}", project_name, e))?;

    {
        let mut watchers = server.file_watchers.write().await;
        watchers.insert(project_id, file_watcher);
    }

    info!("File watcher started and stored for project '{}'", project_name);

    // Start WebSocket listener and wait until initial RiftJoined sync completes
    if let Err(e) = crate::daemon::MothershipDaemon::start_websocket_listener(
        project_id,
        server.tracked_projects.clone(),
        server.status.clone(),
        server.websocket_listeners.clone(),
        server.outgoing_channels.clone(),
        server.server_write_flags.clone(),
        server.base_hashes.clone(),
    )
    .await
    {
        error!("Failed to start WebSocket listener for project {}: {}", project_id, e);
    }

    server.server_write_flags.write().await.remove(&project_id);

    info!("Project '{}' added for tracking with active file watcher and WebSocket sync", project_name);
    Ok(format!("Project '{}' successfully added for tracking", project_name))
}

/// Add a project for tracking
async fn add_project(
    State(server): State<Arc<IpcServer>>,
    Json(req): Json<AddProjectRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    info!("Adding project for tracking: {} at {}", req.project_name, req.project_path.display());
    match register_project_internal(&server, req.project_id, req.project_name, req.project_path).await {
        Ok(msg) => Ok(Json(ApiResponse::success(msg))),
        Err(err) => Ok(Json(ApiResponse::error(err))),
    }
}

/// Flushes unsynced changes for a tracked project and pauses its watcher/WebSocket before re-beaming
async fn pause_project(
    State(server): State<Arc<IpcServer>>,
    Path(project_id): Path<Uuid>,
) -> Json<ApiResponse<String>> {
    if !server.tracked_projects.read().await.contains_key(&project_id) {
        return Json(ApiResponse::success("Project not currently tracked".to_string()));
    }

    crate::daemon::MothershipDaemon::flush_unsynced_project_files(
        project_id,
        &server.tracked_projects,
        &server.outgoing_channels,
        &server.base_hashes,
    )
    .await;

    server.server_write_flags.write().await.insert(project_id, true);
    server.file_watchers.write().await.remove(&project_id);

    let old_listener = server.websocket_listeners.write().await.remove(&project_id);
    server.outgoing_channels.write().await.remove(&project_id);
    if let Some(handle) = old_listener {
        let abort = handle.abort_handle();
        if tokio::time::timeout(std::time::Duration::from_secs(10), handle).await.is_err() {
            abort.abort();
        }
    }

    Json(ApiResponse::success(format!("Paused project {}", project_id)))
}

/// Restart a project's WebSocket so it picks up a changed rift_id from project.json
async fn reconnect_project(
    State(server): State<Arc<IpcServer>>,
    Path(project_id): Path<Uuid>,
) -> Json<ApiResponse<String>> {
    // If the project is not currently in memory (e.g. after a daemon restart), try to locate and register it
    if !server.tracked_projects.read().await.contains_key(&project_id) {
        let found = crate::daemon::load_persisted_projects()
            .into_iter()
            .find(|p| p.project_id == project_id)
            .map(|p| (p.project_name, p.project_path))
            .or_else(|| {
                let index = mothership_common::beam::index_local_projects(
                    &mothership_common::beam::default_search_roots(),
                );
                let path = index.get(&project_id)?.clone();
                let meta = mothership_common::beam::read_project_metadata(&path)?;
                Some((meta.project_name, path))
            });
        match found {
            Some((name, path)) => match register_project_internal(&server, project_id, name, path).await {
                Ok(_) => return Json(ApiResponse::success(format!("Reconnected project {}", project_id))),
                Err(e) => return Json(ApiResponse::error(e)),
            },
            None => {
                return Json(ApiResponse::error(format!(
                    "Project {} is not tracked by the daemon",
                    project_id
                )))
            }
        }
    }

    // Flush any unsynced local file edits to the old rift before closing its WebSocket!
    crate::daemon::MothershipDaemon::flush_unsynced_project_files(
        project_id,
        &server.tracked_projects,
        &server.outgoing_channels,
        &server.base_hashes,
    )
    .await;

    // Close the old queue so its listener sends what is already queued, then stops.
    // Waiting for it keeps those edits from being dropped, and the new listener's
    // RiftJoined handler must not overwrite local files until they are sent.
    let old_listener = server.websocket_listeners.write().await.remove(&project_id);
    server.outgoing_channels.write().await.remove(&project_id);
    if let Some(handle) = old_listener {
        let abort = handle.abort_handle();
        match tokio::time::timeout(std::time::Duration::from_secs(15), handle).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!("Old WebSocket for project {} did not drain in time; aborting it", project_id);
                abort.abort();
            }
        }
    }

    match crate::daemon::MothershipDaemon::start_websocket_listener(
        project_id,
        server.tracked_projects.clone(),
        server.status.clone(),
        server.websocket_listeners.clone(),
        server.outgoing_channels.clone(),
        server.server_write_flags.clone(),
        server.base_hashes.clone(),
    )
    .await
    {
        Ok(()) => {
            info!("Reconnected WebSocket for project {}", project_id);
            Json(ApiResponse::success(format!("Reconnected project {}", project_id)))
        }
        Err(e) => {
            error!("Failed to reconnect WebSocket for project {}: {}", project_id, e);
            Json(ApiResponse::error(format!("Failed to reconnect: {}", e)))
        }
    }
}

/// Record a beamed wormhole snapshot folder so the daemon can clean it up later
async fn register_snapshot(
    State(server): State<Arc<IpcServer>>,
    Json(req): Json<RegisterSnapshotRequest>,
) -> Json<ApiResponse<String>> {
    match server.snapshots.register(req.folder.clone(), req.wormhole_id) {
        Ok(()) => Json(ApiResponse::success(format!(
            "Registered snapshot {}",
            req.folder.display()
        ))),
        Err(e) => {
            error!("Failed to register snapshot {}: {}", req.folder.display(), e);
            Json(ApiResponse::error(format!("Failed to register snapshot: {}", e)))
        }
    }
}

/// Remove a project from tracking
async fn remove_project(
    State(server): State<Arc<IpcServer>>,
    Path(project_id): Path<Uuid>,
) -> Json<ApiResponse<String>> {
    info!("Removing project from tracking: {}", project_id);

    let project_name = {
        let mut projects = server.tracked_projects.write().await;
        let removed = projects.remove(&project_id)
            .map(|p| p.project_name)
            .unwrap_or_else(|| project_id.to_string());
        crate::daemon::save_persisted_projects(&projects);
        removed
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
            info!("Stopped file watcher for project '{}'", project_name);
        }
    }

    // CRITICAL: Remove WebSocket listener and channels to stop sync
    server.outgoing_channels.write().await.remove(&project_id);
    server.base_hashes.write().await.retain(|(id, _), _| *id != project_id);
    server.server_write_flags.write().await.remove(&project_id);
    {
        let mut listeners = server.websocket_listeners.write().await;
        if let Some(handle) = listeners.remove(&project_id) {
            handle.abort();
            info!("Stopped WebSocket listener for project '{}'", project_name);
        }
    }

    info!("Project '{}' removed from tracking ({} projects remaining)", project_name, projects_remaining);
    Json(ApiResponse::success(format!(
        "Project '{}' successfully removed from tracking ({} projects remaining)",
        project_name, projects_remaining
    )))
}

/// Shutdown the daemon gracefully
async fn shutdown_daemon(State(_server): State<Arc<IpcServer>>) -> Json<ApiResponse<String>> {
    info!("Received shutdown request from CLI");
    
    // Schedule shutdown after a brief delay to allow response to be sent
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        info!("Initiating graceful shutdown...");
        std::process::exit(0);
    });
    
    Json(ApiResponse::success(
        "Shutdown signal received - daemon will stop in 500ms".to_string()
    ))
} 