use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, BeamRequest, BeamResponse},
    Project, ProjectId,
};
use uuid::Uuid;

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success};

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

    // TODO: Start background sync process
    print_info("Real-time sync would start here (not yet implemented)");
    print_info(&format!("Checkpoints available: {}", beam_data.checkpoint_count));

    // For now, just show success
    println!("\n{}", "🎉 You're now connected to the project!".green().bold());
    println!("{}", "In a full implementation:".dimmed());
    println!("{}", "  • Files would sync to your local workspace".dimmed());
    println!("{}", "  • Real-time collaboration would be active".dimmed());
    println!("{}", "  • Auto-checkpointing would be running".dimmed());

    Ok(())
} 