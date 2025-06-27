use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, BeamRequest, BeamResponse},
    Project, ProjectId,
};

use uuid::Uuid;

use crate::{config::ConfigManager, file_watcher::FileWatcher, get_http_client, print_api_error, print_info, print_success};

pub async fn handle_beam(
    config_manager: &ConfigManager,
    project: String,
    rift: Option<String>,
    force_sync: bool,
) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {  
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    // Try to parse as UUID first, otherwise treat as project name
    let project_id: ProjectId = if let Ok(uuid) = project.parse::<Uuid>() {
        // It's a UUID
        uuid
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
        
        project_data.id
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

    if beam_data.initial_sync_required {
        print_info("Initial sync required - this would download project files");
        // TODO: Implement file sync
    }

    // Start file watcher for real-time sync
    let current_dir = std::env::current_dir()?;
    let project_path = current_dir; // Use current directory as project path
    
    print_info(&format!("Starting file watcher for: {}", project_path.display()));
    
    let file_watcher = FileWatcher::new(
        project_path,
        beam_data.rift_id.to_string(),
        config.mothership_url.clone(),
    );
    
    // Start watching (this will run indefinitely)
    println!("\n{}", "🎉 You're now connected to the project!".green().bold());
    println!("{}", "🔍 File watcher is starting...".cyan());
    println!("{}", "💡 Edit files in this directory - changes will sync automatically".dimmed());
    println!("{}", "⏹️  Press Ctrl+C to stop watching".dimmed());
    
    // This will block and run the file watcher
    file_watcher.start_watching().await?;

    Ok(())
} 