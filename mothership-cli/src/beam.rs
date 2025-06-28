use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, BeamRequest, BeamResponse, SyncMessage},
    Project, ProjectId, RiftId,
};
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};

use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success};

/// Check if daemon is running and start it if needed
async fn ensure_daemon_running() -> Result<()> {
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
    
    // Try to start the daemon
    #[cfg(windows)]
    {
        // On Windows, try to use the executable in target/debug or target/release
        let current_exe = std::env::current_exe()?;
        let exe_dir = current_exe.parent().ok_or_else(|| anyhow!("Could not find executable directory"))?;
        
        let daemon_paths = [
            exe_dir.join("mothership-daemon.exe"),
            exe_dir.join("../target/debug/mothership-daemon.exe"),
            exe_dir.join("../target/release/mothership-daemon.exe"),
            exe_dir.join("../../target/debug/mothership-daemon.exe"),
            exe_dir.join("../../target/release/mothership-daemon.exe"),
        ];
        
        let mut daemon_started = false;
        for daemon_path in &daemon_paths {
            if daemon_path.exists() {
                match std::process::Command::new(daemon_path)
                    .creation_flags(0x08000000) // CREATE_NO_WINDOW
                    .spawn()
                {
                    Ok(_) => {
                        daemon_started = true;
                        break;
                    }
                    Err(e) => {
                        print_info(&format!("Failed to start daemon from {}: {}", daemon_path.display(), e));
                    }
                }
            }
        }
        
        if !daemon_started {
            return Err(anyhow!("Could not find or start mothership-daemon.exe. Please ensure it's built and in your PATH."));
        }
    }
    
    #[cfg(not(windows))]
    {
        // On Unix systems
        let current_exe = std::env::current_exe()?;
        let exe_dir = current_exe.parent().ok_or_else(|| anyhow!("Could not find executable directory"))?;
        
        let daemon_paths = [
            exe_dir.join("mothership-daemon"),
            exe_dir.join("../target/debug/mothership-daemon"),
            exe_dir.join("../target/release/mothership-daemon"),
            exe_dir.join("../../target/debug/mothership-daemon"),
            exe_dir.join("../../target/release/mothership-daemon"),
        ];
        
        let mut daemon_started = false;
        for daemon_path in &daemon_paths {
            if daemon_path.exists() {
                match std::process::Command::new(daemon_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => {
                        daemon_started = true;
                        break;
                    }
                    Err(e) => {
                        print_info(&format!("Failed to start daemon from {}: {}", daemon_path.display(), e));
                    }
                }
            }
        }
        
        if !daemon_started {
            return Err(anyhow!("Could not find or start mothership-daemon. Please ensure it's built and in your PATH."));
        }
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

/// Register a project with the Mothership daemon for background file tracking
async fn register_project_with_daemon(
    project_id: &Uuid,
    project_name: &str,
    project_path: &PathBuf,
) -> Result<()> {
    let daemon_client = reqwest::Client::new();
    
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

/// Check if a project exists locally
fn is_project_local(project_name: &str) -> bool {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    
    // Check if a directory with the project name exists in current directory
    let project_dir = current_dir.join(project_name);
    
    if project_dir.exists() && project_dir.is_dir() {
        // Check if it has .mothership directory (indicating it's a proper gateway)
        let mothership_dir = project_dir.join(".mothership");
        if mothership_dir.exists() && mothership_dir.is_dir() {
            return true;
        }
    }
    
    // Also check if current directory itself is this project
    if let Some(current_name) = current_dir.file_name().and_then(|n| n.to_str()) {
        if current_name == project_name {
            let mothership_dir = current_dir.join(".mothership");
            if mothership_dir.exists() && mothership_dir.is_dir() {
                return true;
            }
        }
    }
    
    false
}

/// Local project metadata stored in .mothership directory
#[derive(Serialize, Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
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
    
    // Connect to WebSocket
    let (ws_stream, _) = connect_async(websocket_url).await
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
                                
                                // Write file content
                                fs::write(&file_path, &file.content)?;
                                print_info(&format!("Downloaded: {}", file.path.display()));
                            }
                            
                            // Create .mothership metadata
                            create_project_metadata(project_path, project_id, project_name, mothership_url)?;
                            
                            print_success("Project files synchronized successfully!");
                            return Ok(());
                        }
                        SyncMessage::RiftJoined { current_files, .. } => {
                            print_success(&format!("Received {} files from rift", current_files.len()));
                            
                            // Write files to disk
                            for (path, content) in current_files {
                                let file_path = project_path.join(&path);
                                
                                // Create parent directories if needed
                                if let Some(parent) = file_path.parent() {
                                    fs::create_dir_all(parent)?;
                                }
                                
                                // Write file content
                                fs::write(&file_path, &content)?;
                                print_info(&format!("Downloaded: {}", path.display()));
                            }
                            
                            // Create .mothership metadata
                            create_project_metadata(project_path, project_id, project_name, mothership_url)?;
                            
                            print_success("Project files synchronized successfully!");
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
) -> Result<()> {
    let mothership_dir = project_path.join(".mothership");
    
    // Create .mothership directory if it doesn't exist
    if !mothership_dir.exists() {
        fs::create_dir_all(&mothership_dir)?;
    }
    
    // Create project metadata file
    let metadata = ProjectMetadata {
        project_id: project_id.to_string(),
        project_name: project_name.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        mothership_url: mothership_url.to_string(),
    };
    
    let metadata_file = mothership_dir.join("project.json");
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    fs::write(&metadata_file, metadata_json)?;
    
    print_info(&format!("Created .mothership directory at: {}", mothership_dir.display()));
    
    Ok(())
}

pub async fn handle_beam(
    config_manager: &ConfigManager,
    project: String,
    rift: Option<String>,
    local_dir: Option<std::path::PathBuf>,
    force_sync: bool,
) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {  
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    // First, determine the project name to check local status
    let project_name_for_check = if project.parse::<Uuid>().is_ok() {
        // If it's a UUID, we'll need to fetch the project name later
        None
    } else {
        // It's already a project name
        Some(project.clone())
    };

    // Check if project is local (if we have the name)
    let is_local = if let Some(name) = &project_name_for_check {
        is_project_local(name)
    } else {
        false // We'll check after fetching project details
    };

    // If project is not local and no local_dir provided, require it
    if !is_local && local_dir.is_none() && project_name_for_check.is_some() {
        print_api_error(&format!(
            "Project '{}' is not available locally. Please specify where to clone it:",
            project_name_for_check.unwrap()
        ));
        println!("  {}", format!("mothership beam \"{}\" --local-dir <path>", project).green());
        println!("  {}", "Example: mothership beam \"my-project\" --local-dir .".dimmed());
        return Ok(());
    }

    // Try to parse as UUID first, otherwise treat as project name
    let (project_id, project_name) = if let Ok(uuid) = project.parse::<Uuid>() {
        // It's a UUID - we need to fetch the project details to get the name
        print_info(&format!("Looking up project by ID: {}", uuid));
        
        let lookup_url = format!("{}/projects/{}", config.mothership_url, uuid);
        let response = client.get(&lookup_url).send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Project with ID '{}' not found.", uuid));
        }
        
        let project_response: ApiResponse<Project> = response.json().await?;
        let project_data = project_response.data.ok_or_else(|| {
            anyhow!("No project data received: {}", project_response.error.unwrap_or_else(|| "Unknown error".to_string()))
        })?;
        
        // Now check if this project is local (we have the name now)
        let project_is_local = is_project_local(&project_data.name);
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
        print_info(&format!("Looking up project by name: {}", project));
        
        let lookup_url = format!("{}/projects/by-name/{}", config.mothership_url, urlencoding::encode(&project));
        let response = client.get(&lookup_url).send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Project '{}' not found. Use 'mothership gateway list' to see available projects.", project));
        }
        
        let project_response: ApiResponse<Project> = response.json().await?;
        let project_data = project_response.data.ok_or_else(|| {
            anyhow!("No project data received: {}", project_response.error.unwrap_or_else(|| "Unknown error".to_string()))
        })?;
        
        (project_data.id, project_data.name.clone())
    };

    print_info(&format!("Beaming into project ID: {}", project_id));

    let beam_request = BeamRequest {
        project_id,
        rift_name: rift.clone(),
        force_sync,
    };

    let beam_url = format!("{}/projects/{}/beam", config.mothership_url, project_id);
    let response = client
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

    // 🔥 SMART RIFT DIRECTORY DETECTION
    let current_dir = std::env::current_dir()?;
    let project_path = if let Some(local_dir_path) = local_dir {
        // User specified a local directory to clone to - create project subdirectory
        let base_path = if local_dir_path.is_absolute() {
            local_dir_path
        } else {
            current_dir.join(local_dir_path)
        };
        
        // Create the project directory inside the specified path
        let project_dir = base_path.join(&project_name);
        
        if project_dir.exists() {
            if !project_dir.is_dir() {
                return Err(anyhow!("Project path exists but is not a directory: {}", project_dir.display()));
            }
            print_info(&format!("Using existing project directory: {}", project_dir.display()));
        } else {
            print_info(&format!("Creating project directory: {}", project_dir.display()));
            std::fs::create_dir_all(&project_dir)?;
        }
        
        project_dir
    } else {
        // No local_dir specified - use existing local project logic
        // Look for a directory with the project name in current directory
        let project_dir = current_dir.join(&project_name);
        
        if project_dir.exists() && project_dir.is_dir() {
            print_info(&format!("Found project directory: {}", project_dir.display()));
            project_dir
        } else {
            // Check if current directory name matches the project
            if let Some(current_name) = current_dir.file_name().and_then(|n| n.to_str()) {
                if current_name == project_name {
                    print_info(&format!("Using current directory as project directory: {}", current_dir.display()));
                    current_dir
                } else {
                    // Create the project directory
                    print_info(&format!("Creating project directory: {}", project_dir.display()));
                    std::fs::create_dir_all(&project_dir)?;
                    project_dir
                }
            } else {
                // Fallback to current directory
                print_info(&format!("Using current directory: {}", current_dir.display()));
                current_dir
            }
        }
    };

    // Create project metadata regardless of sync requirements
    create_project_metadata(&project_path, &project_id, &project_name, &config.mothership_url)?;
    
    // Note: Initial sync will be handled by the background daemon
    if beam_data.initial_sync_required {
        print_info("Initial sync will be handled by background daemon...");
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
    
    println!("\n{}", "🎉 Successfully beamed into project!".green().bold());
    println!("{}", format!("📁 Project location: {}", project_path.display()).dimmed());
    println!("{}", "🚀 Mothership daemon is now running in the background".dimmed());
    println!("{}", "💡 Files will sync automatically - edit and save to see real-time sync".dimmed());
    println!("{}", "🔧 Use 'mothership status' to check sync status".dimmed());
    println!("{}", "⏹️  The daemon will continue running until you restart your computer".dimmed());

    Ok(())
}

/// Handle disconnect command - remove project from daemon tracking
pub async fn handle_disconnect(
    config_manager: &ConfigManager,
    project: Option<String>,
) -> Result<()> {
    // Check if daemon is running
    let daemon_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    
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
    
    let config = config_manager.load_config()?;
    let client = get_http_client(&config);
    
    // Look up project by name to get ID
    let lookup_url = format!("{}/projects/by-name/{}", config.mothership_url, urlencoding::encode(&project_name));
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
    let daemon_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    
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
    let daemon_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    
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