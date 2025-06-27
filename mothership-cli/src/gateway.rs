use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    protocol::{ApiResponse, GatewayRequest},
    GatewayProject, Project,
};
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success};

pub async fn handle_gateway(config_manager: &ConfigManager, include_inactive: bool) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    let gateway_request = GatewayRequest {
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

    // Check if we're already inside a gateway
    if let Some(gateway_root) = find_gateway_root(&dir) {
        return Err(anyhow!(
            "Cannot create gateway inside another gateway.\nExisting gateway found at: {}\n\nTo create a new gateway, choose a directory outside of any existing gateway.",
            gateway_root.display()
        ));
    }

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    print_info(&format!("Creating gateway '{}' for directory: {}", name, dir.display()));

    // Create project request
    let create_request = CreateGatewayRequest {
        name: name.clone(),
        description: format!("Gateway for {}", dir.display()),
        project_path: dir.clone(),
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
    
    // Create .mothership directory with metadata
    if let Err(e) = create_gateway_metadata(&dir, &project, &config.mothership_url) {
        print_api_error(&format!("Warning: Failed to create .mothership directory: {}", e));
        print_info("Gateway was created successfully on the server, but local metadata may be incomplete.");
    }
    
    print_info("File watching will be implemented in the next phase");
    print_info(&format!("Use 'mothership beam {}' to start collaborating", project.id));

    Ok(())
}

#[derive(serde::Serialize)]
struct CreateGatewayRequest {
    name: String,
    description: String,
    project_path: PathBuf,
}

/// Local project metadata stored in .mothership directory
#[derive(Serialize, Deserialize)]
pub struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
}

/// Check if we're already inside a gateway by looking for .mothership directory
fn find_gateway_root(start_dir: &PathBuf) -> Option<PathBuf> {
    let mut current_dir = start_dir.clone();
    
    loop {
        let mothership_dir = current_dir.join(".mothership");
        if mothership_dir.exists() && mothership_dir.is_dir() {
            return Some(current_dir);
        }
        
        // Move to parent directory
        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            break;
        }
    }
    
    None
}

/// Create .mothership directory with project metadata
fn create_gateway_metadata(
    project_dir: &PathBuf,
    project: &Project,
    mothership_url: &str,
) -> Result<()> {
    let mothership_dir = project_dir.join(".mothership");
    
    // Create .mothership directory
    fs::create_dir_all(&mothership_dir)?;
    
    // Create project metadata file
    let metadata = ProjectMetadata {
        project_id: project.id.to_string(),
        project_name: project.name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        mothership_url: mothership_url.to_string(),
    };
    
    let metadata_file = mothership_dir.join("project.json");
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    fs::write(&metadata_file, metadata_json)?;
    
    // Note: Users should add .mothership/ to their project's main .gitignore
    
    print_info(&format!("Created .mothership directory at: {}", mothership_dir.display()));
    
    Ok(())
}

/// Read gateway metadata from .mothership directory
#[allow(dead_code)]
fn read_gateway_metadata(project_dir: &PathBuf) -> Result<ProjectMetadata> {
    let mothership_dir = project_dir.join(".mothership");
    let metadata_file = mothership_dir.join("project.json");
    
    if !metadata_file.exists() {
        return Err(anyhow!("No .mothership/project.json found. This directory is not a Mothership gateway."));
    }
    
    let metadata_content = fs::read_to_string(&metadata_file)?;
    let metadata: ProjectMetadata = serde_json::from_str(&metadata_content)?;
    
    Ok(metadata)
}

/// Check if the current directory is inside a gateway and return its metadata
#[allow(dead_code)]
fn find_current_gateway() -> Result<Option<(PathBuf, ProjectMetadata)>> {
    let current_dir = std::env::current_dir()?;
    
    if let Some(gateway_root) = find_gateway_root(&current_dir) {
        match read_gateway_metadata(&gateway_root) {
            Ok(metadata) => Ok(Some((gateway_root, metadata))),
            Err(_) => Ok(None), // Gateway directory exists but metadata is corrupted/missing
        }
    } else {
        Ok(None)
    }
} 