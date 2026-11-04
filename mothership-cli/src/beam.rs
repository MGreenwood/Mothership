use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, BeamRequest, BeamResponse, SyncMessage},
    Project, ProjectId, RiftId,
    ClientConfig,
};
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};

use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success, connections};

/// Check if daemon is running and start it if needed
/// Try to start daemon from a specific path
pub fn try_start_daemon(daemon_path: &std::path::Path) -> Result<bool> {
    let mut cmd = std::process::Command::new(daemon_path);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        mothership_common::beam::clear_std_handle_inheritance();
    }

    match cmd.spawn() {
        Ok(_) => Ok(true),
        Err(e) => {
            print_info(&format!("Failed to start daemon from {}: {}", daemon_path.display(), e));
            Ok(false)
        }
    }
}

pub async fn ensure_daemon_running() -> Result<()> {
    let daemon_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    
    // First, check if daemon is already running
    match daemon_client.get("http://localhost:7525/health").send().await {
        Ok(response) if response.status().is_success() => {
            // Daemon is already running
            return Ok(());
        }
        _ => {
            // Daemon not running, need to start it
        }
    }
    
    print_info("Starting Mothership daemon in background...");
    
    // IMPROVED: Prefer the daemon binary next to the running CLI executable first, then build dirs, then PATH
    let daemon_binary = if cfg!(windows) { "mothership-daemon.exe" } else { "mothership-daemon" };
    let mut daemon_started = false;

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let candidates = [
                exe_dir.join(daemon_binary),
                exe_dir.join(format!("../target/debug/{}", daemon_binary)),
                exe_dir.join(format!("../target/release/{}", daemon_binary)),
                exe_dir.join(format!("../../target/debug/{}", daemon_binary)),
                exe_dir.join(format!("../../target/release/{}", daemon_binary)),
            ];
            for daemon_path in &candidates {
                if daemon_path.is_file() && try_start_daemon(daemon_path)? {
                    print_success(&format!("Started daemon from: {}", daemon_path.display()));
                    daemon_started = true;
                    break;
                }
            }
        }
    }

    if !daemon_started {
        if let Ok(path_result) = which::which(daemon_binary) {
            if try_start_daemon(&path_result)? {
                print_success(&format!("Started daemon from PATH: {}", path_result.display()));
                daemon_started = true;
            }
        }
    }

    if !daemon_started {
        return Err(anyhow!(
            "Could not find {}. Please ensure it's:\n  1. In the same directory as the mothership CLI\n  2. Installed via 'cargo install --path mothership-daemon'\n  3. Available in your PATH",
            daemon_binary
        ));
    }
    
    // Wait for daemon to start up
    print_info("Waiting for daemon to initialize...");
    let mut attempts = 0;
    while attempts < 10 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        
        match daemon_client.get("http://localhost:7525/health").send().await {
            Ok(response) if response.status().is_success() => {
                print_success("Mothership daemon started successfully!");
                return Ok(());
            }
            _ => {
                attempts += 1;
            }
        }
    }
    
    Err(anyhow!("Daemon started but failed to respond within 5 seconds"))
}

fn daemon_http_client() -> &'static reqwest::Client {
    static DAEMON_HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    DAEMON_HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| crate::http_client().clone())
    })
}

/// Register a project with the Mothership daemon for background file tracking
async fn register_project_with_daemon(
    project_id: &Uuid,
    project_name: &str,
    project_path: &PathBuf,
) -> Result<()> {
    let daemon_client = crate::http_client();
    
    #[derive(serde::Serialize)]
    struct AddProjectRequest {
        project_id: Uuid,
        project_name: String,
        project_path: PathBuf,
    }
    
    let request = AddProjectRequest {
        project_id: *project_id,
        project_name: project_name.to_string(),
        project_path: project_path.clone(),
    };
    
    let response = daemon_client
        .post("http://localhost:7525/projects/add")
        .json(&request)
        .send()
        .await?;
    
    if response.status().is_success() {
        Ok(())
    } else {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        Err(anyhow!("Daemon registration failed: {}", error_text))
    }
}

/// Check if a project exists locally using a pre-built project index
fn is_project_local(
    project_name: &str,
    project_index: &std::collections::HashMap<String, PathBuf>,
) -> bool {
    project_index.contains_key(project_name)
}

/// Local project metadata stored in .mothership directory
#[derive(Serialize, Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
    rift_id: Option<String>, // CRITICAL FIX: Store rift_id for daemon WebSocket connection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_rift: Option<String>,
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

/// Perform initial sync by connecting to WebSocket and requesting all files
async fn perform_initial_sync(
    websocket_url: &str,
    rift_id: &RiftId,
    project_path: &PathBuf,
    project_id: &ProjectId,
    project_name: &str,
    mothership_url: &str,
) -> Result<()> {
    print_info("Connecting to sync server...");
    
    // AUTHENTICATION FIX: Add auth token to WebSocket URL
    let auth_token = load_auth_token()
        .ok_or_else(|| anyhow!("No authentication token found. Please run 'mothership auth' first."))?;
    
    let authenticated_url = if websocket_url.contains('?') {
        format!("{}&token={}", websocket_url, urlencoding::encode(&auth_token))
    } else {
        format!("{}?token={}", websocket_url, urlencoding::encode(&auth_token))
    };
    
    // Connect to WebSocket with authentication
    let (ws_stream, _) = connect_async(&authenticated_url).await
        .map_err(|e| anyhow!("Failed to connect to WebSocket: {}", e))?;
    
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    
    // Send JoinRift message (server responds with RiftJoined containing all files)
    let join_rift = SyncMessage::JoinRift {
        rift_id: *rift_id,
        last_checkpoint: None, // Request all files from beginning
    };
    
    let join_json = serde_json::to_string(&join_rift)?;
    ws_sender.send(Message::Text(join_json)).await
        .map_err(|e| anyhow!("Failed to send join rift: {}", e))?;
    
    print_info("Requesting project files...");
    
    // Wait for SyncData response
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(sync_msg) = serde_json::from_str::<SyncMessage>(&text) {
                    match sync_msg {
                        SyncMessage::SyncData { files, .. } => {
                            print_success(&format!("Received {} files from server", files.len()));
                            
                            // Write files to disk
                            for file in files {
                                let file_path = project_path.join(&file.path);
                                
                                // Create parent directories if needed
                                if let Some(parent) = file_path.parent() {
                                    fs::create_dir_all(parent)?;
                                }
                                
                                // Keep any local edit that the server copy is about to replace
                                match mothership_common::preserve_local_version(project_path, &file.path, file.content.as_bytes()) {
                                    Ok(Some(backup)) => print_info(&format!("Kept local version of {} at {}", file.path.display(), backup.display())),
                                    Ok(None) => {}
                                    Err(e) => print_info(&format!("Could not back up local {}: {}", file.path.display(), e)),
                                }

                                // Write file content
                                fs::write(&file_path, &file.content)?;
                                print_info(&format!("Downloaded: {}", file.path.display()));
                            }
                            
                            // Create .mothership metadata
                            create_project_metadata(project_path, project_id, project_name, mothership_url, Some(rift_id))?;
                            
                            print_success("Project files synchronized successfully!");
                            
                            // CRITICAL FIX: Close this temporary connection gracefully with proper close frame
                            let close_frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                                reason: "Initial sync completed".into(),
                            };
                            let _ = ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(Some(close_frame))).await;
                            return Ok(());
                        }
                        SyncMessage::RiftJoined { current_files, .. } => {
                            print_success(&format!("Received {} files from rift", current_files.len()));
                            
                            let mut keep = std::collections::HashSet::new();
                            let mut valid_files = Vec::new();
                            for (raw_path, content) in current_files {
                                let Some(rel_path) = mothership_common::beam::safe_relative_path(&raw_path) else {
                                    continue;
                                };
                                keep.insert(mothership_common::normalize_rel_path(&rel_path));
                                valid_files.push((rel_path, content));
                            }
                            let _ = mothership_common::beam::remove_stale_local_files(project_path, &keep);

                            // Write only files that differ from the rift; identical files are left untouched
                            let mut unchanged = 0usize;
                            let mut updated = 0usize;
                            for (path, content) in valid_files {
                                let file_path = project_path.join(&path);

                                if let Ok(existing) = fs::read(&file_path) {
                                    if existing == content.as_bytes() {
                                        unchanged += 1;
                                        continue;
                                    }
                                }

                                // Create parent directories if needed
                                if let Some(parent) = file_path.parent() {
                                    fs::create_dir_all(parent)?;
                                }

                                // Back up any differing local file, then make it match the rift exactly
                                match mothership_common::preserve_local_version(project_path, &path, content.as_bytes()) {
                                    Ok(Some(backup)) => print_info(&format!("Backed up local {} to {}", path.display(), backup.display())),
                                    Ok(None) => {}
                                    Err(e) => print_info(&format!("Could not back up local {}: {}", path.display(), e)),
                                }

                                // Write file content
                                fs::write(&file_path, &content)?;
                                updated += 1;
                                print_info(&format!("Updated: {}", path.display()));
                            }
                            print_info(&format!("{} file(s) already up to date, {} updated", unchanged, updated));
                            
                            // Create .mothership metadata
                            create_project_metadata(project_path, project_id, project_name, mothership_url, Some(rift_id))?;
                            
                            print_success("Project files synchronized successfully!");
                            
                            // CRITICAL FIX: Close this temporary connection gracefully with proper close frame
                            let close_frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                                reason: "Initial sync completed".into(),
                            };
                            let _ = ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(Some(close_frame))).await;
                            return Ok(());
                        }
                        SyncMessage::Error { message, .. } => {
                            return Err(anyhow!("Sync error: {}", message));
                        }
                        _ => {
                            // Continue waiting for the right message
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                return Err(anyhow!("WebSocket connection closed unexpectedly"));
            }
            Err(e) => {
                return Err(anyhow!("WebSocket error: {}", e));
            }
            _ => {
                // Continue for other message types
            }
        }
    }
    
    Err(anyhow!("No sync data received"))
}

/// Create .mothership directory with project metadata
fn create_project_metadata(
    project_path: &PathBuf,
    project_id: &ProjectId,
    project_name: &str,
    mothership_url: &str,
    rift_id: Option<&uuid::Uuid>,
) -> Result<()> {
    let mothership_dir = project_path.join(".mothership");
    
    // Create .mothership directory if it doesn't exist
    if !mothership_dir.exists() {
        fs::create_dir_all(&mothership_dir)?;
    }
    
    // Create project metadata file (preserve existing current_rift when present)
    let existing_rift_name = project_metadata_at(project_path).and_then(|m| m.current_rift);
    let metadata = ProjectMetadata {
        project_id: project_id.to_string(),
        project_name: project_name.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        mothership_url: mothership_url.to_string(),
        rift_id: rift_id.map(|id| id.to_string()), // CRITICAL FIX: Store rift_id
        current_rift: existing_rift_name,
    };
    
    let metadata_file = mothership_dir.join("project.json");
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    fs::write(&metadata_file, metadata_json)?;
    
    print_info(&format!("Created .mothership directory at: {}", mothership_dir.display()));
    
    Ok(())
}

/// Check if the current directory is a Mothership project and return its metadata
fn get_current_project_metadata() -> Option<ProjectMetadata> {
    let current_dir = std::env::current_dir().ok()?;
    project_metadata_at(&current_dir)
}

/// Reads `.mothership/project.json` from a project directory, if present.
fn project_metadata_at(dir: &std::path::Path) -> Option<ProjectMetadata> {
    let metadata_path = dir.join(".mothership").join("project.json");
    let content = std::fs::read_to_string(&metadata_path).ok()?;
    serde_json::from_str::<ProjectMetadata>(&content).ok()
}

/// Handle beam command - either with explicit project name or auto-detect from current directory
pub async fn handle_beam(
    _config_manager: &ConfigManager,
    project: String,
    local_dir: Option<std::path::PathBuf>,
    force_sync: bool,
) -> Result<()> {
    // Build local project index once per command invocation
    let project_index = crate::project_index::build_index(&crate::project_index::default_search_roots());

    // If no project specified (empty string), try to detect from current directory
    let (project_name, project_path) = if project.is_empty() {
        if let Some(metadata) = get_current_project_metadata() {
            print_info("Detected Mothership project in current directory");
            (metadata.project_name, std::env::current_dir()?)
        } else {
            return Err(anyhow!("No project specified and not in a Mothership project directory.\nPlease specify a project name or run this command from a project directory."));
        }
    } else {
        // Check if project exists locally first
        let is_local = is_project_local(&project, &project_index);
        
        // Determine project path based on local_dir or existing project
        let path = if let Some(ref dir) = local_dir {
            if !dir.exists() {
                return Err(anyhow!("Specified directory does not exist: {}", dir.display()));
            }
            // If the directory already is this project, use it directly;
            // otherwise create a subdirectory named after the project.
            let already_project = project_metadata_at(dir)
                .map(|m| m.project_id == project || m.project_name == project)
                .unwrap_or(false);
            if already_project {
                dir.clone()
            } else {
                dir.join(&project)
            }
        } else if is_local {
            // If project exists locally, resolve its path from the pre-built index
            project_index.get(&project).cloned().unwrap_or_else(|| {
                // Fallback to current directory behavior
                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if current_dir.file_name().and_then(|n| n.to_str()) == Some(&project) {
                    current_dir
                } else {
                    current_dir.join(&project)
                }
            })
        } else {
            // Project doesn't exist locally and no directory specified
            print_info("Project doesn't exist locally. Please specify a directory:");
            println!("  {}", format!("mothership beam \"{}\" --local-dir <path>", project).green());
            println!("  {}", "Example: mothership beam \"my-project\" --local-dir .".dimmed());
            return Ok(());
        };
        
        (project, path)
    };

    // Get active server configuration
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Ensure daemon is running
    ensure_daemon_running().await?;
    
    // Try to parse as UUID first, otherwise treat as project name
    let (project_id, project_name) = if let Ok(uuid) = project_name.parse::<Uuid>() {
        // It's a UUID - we need to fetch the project details to get the name
        print_info(&format!("Looking up project by ID: {}", uuid));
        
        let client_config = ClientConfig {
            mothership_url: active_server.url.clone(),
            local_workspace: PathBuf::from("."),
            auth_token: active_server.auth_token.clone(),
            user_id: Some(Uuid::new_v4()),
        };
        
        let lookup_url = format!("{}/projects/{}", active_server.url, uuid);
        let response = get_http_client(&client_config).get(&lookup_url).send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Project ID {} not found", uuid));
        }
        
        let project_response: ApiResponse<Project> = response.json().await?;
        let project_data = project_response.data.ok_or_else(|| {
            anyhow!("No project data received: {}", project_response.error.unwrap_or_else(|| "Unknown error".to_string()))
        })?;
        
        // Now check if this project is local (we have the name now)
        let project_is_local = is_project_local(&project_data.name, &project_index);
        if !project_is_local && local_dir.is_none() {
            print_api_error(&format!(
                "Project '{}' is not available locally. Please specify where to clone it:",
                project_data.name
            ));
            println!("  {}", format!("mothership beam \"{}\" --local-dir <path>", project_data.name).green());
            println!("  {}", "Example: mothership beam \"my-project\" --local-dir .".dimmed());
            return Ok(());
        }
        
        (uuid, project_data.name)
    } else {
        // It's a project name - look it up
        print_info(&format!("Looking up project by name: {}", project_name));
        
        let client_config = ClientConfig {
            mothership_url: active_server.url.clone(),
            local_workspace: PathBuf::from("."),
            auth_token: active_server.auth_token.clone(),
            user_id: Some(Uuid::new_v4()),
        };
        
        let lookup_url = format!("{}/projects?name={}", active_server.url, project_name);
        let response = get_http_client(&client_config).get(&lookup_url).send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Project '{}' not found. Use 'mothership gateway list' to see available projects.", project_name));
        }
        
        let project_response: ApiResponse<Vec<Project>> = response.json().await?;
        let projects = project_response.data.ok_or_else(|| {
            anyhow!("No project data received: {}", project_response.error.unwrap_or_else(|| "Unknown error".to_string()))
        })?;
        
        let project_data = projects.into_iter()
            .find(|p| p.name == project_name)
            .ok_or_else(|| anyhow!("Project '{}' not found", project_name))?;
        
        (project_data.id, project_data.name.clone())
    };

    print_info(&format!("Beaming into project ID: {}", project_id));

    // Pause any active watcher/WebSocket in the daemon for this project (flushing pending edits on the old rift first)
    let _ = mothership_common::daemon::pause_project(project_id).await;

    // Use the rift this folder is currently on (from .mothership metadata); None means the server default
    let rift = project_metadata_at(&project_path).and_then(|m| m.current_rift);

    let beam_request = BeamRequest {
        project_id,
        rift_name: rift.clone(),
        force_sync,
    };

    let beam_url = format!("{}/projects/{}/beam", active_server.url, project_id);
    let client_config = ClientConfig {
        mothership_url: active_server.url.clone(),
        local_workspace: PathBuf::from("."),
        auth_token: active_server.auth_token.clone(),
        user_id: Some(Uuid::new_v4()),
    };
    
    let response = get_http_client(&client_config)
        .post(&beam_url)
        .json(&beam_request)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Beam request failed: {}", response.status()));
    }

    let beam_response: ApiResponse<BeamResponse> = response.json().await?;
    
    let beam_data = beam_response.data.ok_or_else(|| {
        anyhow!("No beam data received: {}", beam_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    print_success("Successfully beamed into project!");
    print_info(&format!("Rift ID: {}", beam_data.rift_id));
    print_info(&format!("WebSocket URL: {}", beam_data.websocket_url));

    // Create the project directory first
    tokio::fs::create_dir_all(&project_path).await?;

    // Create project metadata regardless of sync requirements (using active server URL)
    create_project_metadata(&project_path, &project_id, &project_name, &active_server.url, Some(&beam_data.rift_id))?;
    
    // CRITICAL FIX: Perform initial sync if required (download all files)
    if beam_data.initial_sync_required {
        print_info("Performing initial file download...");
        
        // Perform initial sync by downloading all project files
        if let Err(e) = perform_initial_sync(
            &beam_data.websocket_url,
            &beam_data.rift_id,
            &project_path,
            &project_id,
            &project_name,
            &active_server.url,
        ).await {
            print_api_error(&format!("Failed to download project files: {}", e));
            print_info("Project structure created, but files may be missing");
            print_info("Try running 'mothership sync' in the project directory");
        }
    }
    
    // Ensure daemon is running and register project with it
    print_info("Setting up background file synchronization...");
    
    match ensure_daemon_running().await {
        Ok(()) => {
            // Daemon is running, now register the project
            if let Err(e) = register_project_with_daemon(&project_id, &project_name, &project_path).await {
                print_api_error(&format!("Failed to register with daemon: {}", e));
                print_info("File changes will not be synced automatically");
            } else {
                print_success("Project registered with daemon for automatic background sync!");
            }
        }
        Err(e) => {
            print_api_error(&format!("Failed to start daemon: {}", e));
            print_info("You can start the daemon manually with 'mothership-daemon'");
            print_info("File changes will not be synced automatically until the daemon is running");
        }
    }
    
    println!("\n{}", "Successfully beamed into project!".green().bold());
    println!("{}", format!("Project location: {}", project_path.display()).dimmed());
    println!("{}", "Mothership daemon is now running in the background".dimmed());
    println!("{}", "Files will sync automatically - edit and save to see real-time sync".dimmed());
    println!("{}", "Use 'mothership status' to check sync status".dimmed());
    println!("{}", " The daemon will continue running until you restart your computer".dimmed());

    Ok(())
}

/// Handle disconnect command - remove project from daemon tracking
pub async fn handle_disconnect(
    config_manager: &ConfigManager,
    project: Option<String>,
) -> Result<()> {
    // Check if daemon is running
    let daemon_client = daemon_http_client();
    
    match daemon_client.get("http://localhost:7525/health").send().await {
        Ok(response) if response.status().is_success() => {
            // Daemon is running
        }
        _ => {
            print_api_error("Mothership daemon is not running. Nothing to disconnect from.");
            return Ok(());
        }
    }
    
    // Determine which project to disconnect from
    let project_name = if let Some(name) = project {
        name
    } else {
        // Try to determine current project from directory
        let current_dir = std::env::current_dir()?;
        let mothership_dir = current_dir.join(".mothership");
        
        if mothership_dir.exists() {
            let metadata_file = mothership_dir.join("project.json");
            if metadata_file.exists() {
                let metadata_content = std::fs::read_to_string(&metadata_file)?;
                let metadata: ProjectMetadata = serde_json::from_str(&metadata_content)?;
                metadata.project_name
            } else {
                return Err(anyhow!("No .mothership/project.json found. Please specify project name."));
            }
        } else {
            return Err(anyhow!("Not in a Mothership project directory. Please specify project name."));
        }
    };
    
    print_info(&format!("Disconnecting from project: {}", project_name));
    
    // Get project ID (we need to call the server to get this)
    if !config_manager.is_authenticated()? {
        return Err(anyhow!("Not authenticated. Please run 'mothership auth' first."));
    }
    
    // Get the active server connection (instead of hardcoded localhost)
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);
    
    // Look up project by name to get ID
    let lookup_url = format!("{}/projects/name/{}", active_server.url, urlencoding::encode(&project_name));
    let response = client.get(&lookup_url).send().await?;
    
    if !response.status().is_success() {
        return Err(anyhow!("Project '{}' not found on server.", project_name));
    }
    
    let project_response: ApiResponse<Project> = response.json().await?;
    let project_data = project_response.data.ok_or_else(|| {
        anyhow!("No project data received")
    })?;
    
    // Remove from daemon
    let remove_url = format!("http://localhost:7525/projects/{}/remove", project_data.id);
    let response = daemon_client.post(&remove_url).send().await?;
    
    if response.status().is_success() {
        print_success(&format!("Successfully disconnected from project '{}'", project_name));
        print_info("The project is no longer being tracked by the background daemon");
        print_info("Files will not sync automatically until you beam back in");
    } else {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to disconnect from daemon: {}", error_text));
    }
    
    Ok(())
}

/// Handle daemon status command
pub async fn handle_daemon_status() -> Result<()> {
    let daemon_client = daemon_http_client();
    
    match daemon_client.get("http://localhost:7525/health").send().await {
        Ok(response) if response.status().is_success() => {
            print_success("Mothership daemon is running");
            
            // Get detailed status
            match daemon_client.get("http://localhost:7525/status").send().await {
                Ok(status_response) if status_response.status().is_success() => {
                    let status_text = status_response.text().await?;
                    print_info("Daemon Status:");
                    println!("{}", status_text);
                }
                _ => {
                    print_info("Could not get detailed daemon status");
                }
            }
            
            // List tracked projects
            match daemon_client.get("http://localhost:7525/projects").send().await {
                Ok(projects_response) if projects_response.status().is_success() => {
                    let projects_text = projects_response.text().await?;
                    print_info("Tracked Projects:");
                    println!("{}", projects_text);
                }
                _ => {
                    print_info("Could not get tracked projects list");
                }
            }
        }
        _ => {
            print_api_error("Mothership daemon is not running");
            print_info("Use 'mothership beam <project>' to start the daemon and begin tracking");
        }
    }
    
    Ok(())
}

/// Handle daemon stop command
pub async fn handle_daemon_stop() -> Result<()> {
    let daemon_client = daemon_http_client();
    
    match daemon_client.get("http://localhost:7525/health").send().await {
        Ok(response) if response.status().is_success() => {
            // Daemon is running, try to stop it
            print_info("Sending shutdown signal to daemon...");
            
            // Send shutdown request (we'll need to implement this endpoint)
            match daemon_client.post("http://localhost:7525/shutdown").send().await {
                Ok(_) => {
                    print_success("Daemon shutdown signal sent");
                    print_info("All background file tracking has stopped");
                    print_info("Use 'mothership beam <project>' to restart daemon and tracking");
                }
                Err(_) => {
                    print_api_error("Failed to send shutdown signal to daemon");
                    print_info("You may need to manually kill the daemon process");
                }
            }
        }
        _ => {
            print_info("Mothership daemon is not running - nothing to stop");
        }
    }
    
    Ok(())
}

/// Handle daemon restart command
pub async fn handle_daemon_restart() -> Result<()> {
    print_info("Stopping daemon...");
    handle_daemon_stop().await?;
    
    // Wait a moment for cleanup
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    
    print_info("Starting daemon...");
    ensure_daemon_running().await?;
    
    print_success("Daemon restarted successfully!");
    print_info("Previous project tracking sessions have been cleared");
    print_info("Use 'mothership beam <project>' to re-register projects");
    
    Ok(())
} 

#[cfg(test)]
mod profiling_tests {
    use super::*;
    use crate::profiling::{build_project_tree, walk_entry_count, TempTree};
    use std::time::Instant;

    #[test]
    #[ignore = "profiling: run with --ignored --nocapture"]
    fn profile_beam_project_lookup() {
        let tree = TempTree::new("beam");
        let root = tree.0.clone();
        let names = build_project_tree(&root, 20, 200);
        let entries = walk_entry_count(&root, 3);
        let roots = vec![root];

        const ITERS: u32 = 5;
        let start = Instant::now();
        for _ in 0..ITERS {
            let index = crate::project_index::build_index(&roots);
            assert!(!is_project_local("missing", &index));
        }
        let per_walk = start.elapsed() / ITERS;

        // beam builds the index once, then checks local status and resolves the path from the map.
        let start = Instant::now();
        let index = crate::project_index::build_index(&roots);
        for name in &names {
            assert!(is_project_local(name, &index));
            assert!(index.get(name).is_some());
        }
        let resolve_twice = start.elapsed();

        eprintln!(
            "[profile] beam lookup: {} projects, {entries} total entries in tree, pruned index walk = {per_walk:?}, \
             check+resolve for all projects = {resolve_twice:?} (1 walk total)",
            names.len(),
        );
    }
}
