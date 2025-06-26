use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, GatewayRequest},
    GatewayProject, Project,
};
use std::path::PathBuf;

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success};

pub async fn handle_gateway(config_manager: &ConfigManager, include_inactive: bool) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    let user_id = config.user_id.ok_or_else(|| anyhow!("No user ID found"))?;

    let gateway_request = GatewayRequest {
        user_id,
        include_inactive,
    };

    let gateway_url = format!("{}/gateway", config.mothership_url);
    let response = client
        .post(&gateway_url)
        .json(&gateway_request)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Gateway request failed: {}", response.status()));
    }

    let gateway_response: ApiResponse<Vec<GatewayProject>> = response.json().await?;
    
    let projects = gateway_response.data.ok_or_else(|| {
        anyhow!("No gateway data received: {}", gateway_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    if projects.is_empty() {
        print_info("No projects available. Contact your administrator to get access to projects.");
        return Ok(());
    }

    // Display projects
    println!("\n{}", "🌌 Your Development Gateway".cyan().bold());
    println!("{}", "Available projects:".white());

    for gateway_project in projects {
        let project = &gateway_project.project;
        println!("\n{} {}", "🚀".green(), project.name.blue().bold());
        println!("   {}", project.description.dimmed());
        
        if !gateway_project.your_rifts.is_empty() {
            println!("   {} Your rifts:", "📂".yellow());
            for rift in &gateway_project.your_rifts {
                let collaborators = if rift.collaborators.len() > 1 {
                    format!(" (with {})", rift.collaborators[1..].join(", "))
                } else {
                    String::new()
                };
                println!("     • {}{}", rift.name.cyan(), collaborators.dimmed());
            }
        }

        if !gateway_project.active_rifts.is_empty() {
            println!("   {} Active rifts:", "⚡".yellow());
            for rift in &gateway_project.active_rifts {
                let collaborators = rift.collaborators.join(", ");
                println!("     • {} ({})", rift.name.cyan(), collaborators.dimmed());
            }
        }

        // Show beam command using project name
        println!("   {} Beam into: {}", "🔧".white(), format!("mothership beam \"{}\"", project.name).green());
    }

    println!("\n{}", "Use 'mothership beam <project-id>' to start working on a project.".dimmed());

    Ok(())
}

pub async fn handle_gateway_create(
    config_manager: &ConfigManager, 
    name: String, 
    dir: PathBuf
) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    // Validate directory
    if !dir.exists() {
        return Err(anyhow!("Directory does not exist: {}", dir.display()));
    }

    if !dir.is_dir() {
        return Err(anyhow!("Path is not a directory: {}", dir.display()));
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);
    let user_id = config.user_id.ok_or_else(|| anyhow!("No user ID found"))?;

    print_info(&format!("Creating gateway '{}' for directory: {}", name, dir.display()));

    // Create project request
    let create_request = CreateGatewayRequest {
        name: name.clone(),
        description: format!("Gateway for {}", dir.display()),
        project_path: dir.clone(),
        user_id,
    };

    let create_url = format!("{}/gateway/create", config.mothership_url);
    let response = client
        .post(&create_url)
        .json(&create_request)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Gateway creation failed: {}", error_text));
    }

    let create_response: ApiResponse<Project> = response.json().await?;
    
    let project = create_response.data.ok_or_else(|| {
        anyhow!("No project data received: {}", create_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    print_success(&format!("Gateway '{}' created successfully!", name));
    print_info(&format!("Project ID: {}", project.id));
    print_info(&format!("Tracking directory: {}", dir.display()));
    
    // TODO: Start file watching and change tracking
    print_info("File watching will be implemented in the next phase");
    print_info(&format!("Use 'mothership beam {}' to start collaborating", project.id));

    Ok(())
}

#[derive(serde::Serialize)]
struct CreateGatewayRequest {
    name: String,
    description: String,
    project_path: PathBuf,
    user_id: uuid::Uuid,
} 