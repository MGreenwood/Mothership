use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{
    client::split_upload_batches,
    ignore::write_default_ignore_file,
    protocol::{ApiResponse, GatewayRequest},
    GatewayProject, Project, ClientConfig,
};
use std::path::PathBuf;
use std::fs;
use std::collections::HashMap;
use std::io::{self, Write};
use serde::{Serialize, Deserialize};

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success, connections};

/// Local status of a project
#[derive(Debug, Clone)]
enum LocalStatus {
    Local,
    NotLocal,
}

/// Check if a project exists locally using a pre-built project index
fn check_project_local_status(
    project_name: &str,
    project_index: &HashMap<String, PathBuf>,
) -> (LocalStatus, String) {
    if let Some(found_path) = project_index.get(project_name) {
        (LocalStatus::Local, format!("(local at {})", found_path.display()))
    } else {
        (LocalStatus::NotLocal, "(not local)".to_string())
    }
}

pub async fn handle_gateway(config_manager: &ConfigManager, include_inactive: bool) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    // Get the active server connection (instead of hardcoded localhost)
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    let gateway_request = GatewayRequest {
        include_inactive,
    };

    let gateway_url = format!("{}/gateway", active_server.url);
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

    // Build local project index once per command invocation
    let project_index = crate::project_index::build_index(&crate::project_index::default_search_roots());

    // Display projects
    println!("\n{}", "Your Development Gateway".cyan().bold());
    println!("{}", "Available projects:".white());

    for gateway_project in projects {
        let project = &gateway_project.project;
        
        // Check if project exists locally
        let (local_status, local_info) = check_project_local_status(&project.name, &project_index);
        let (status_indicator, project_name_colored) = match local_status {
            LocalStatus::Local => ("".green(), project.name.green().bold()),
            LocalStatus::NotLocal => ("".red(), project.name.red().bold()),
        };
        
        println!("\n{} {} {}", status_indicator, project_name_colored, local_info.dimmed());
        println!("   {}", project.description.dimmed());
        
        if !gateway_project.your_rifts.is_empty() {
            println!("   {} Your rifts:", "".yellow());
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
            println!("   {} Active rifts:", "".yellow());
            for rift in &gateway_project.active_rifts {
                let collaborators = rift.collaborators.join(", ");
                println!("     • {} ({})", rift.name.cyan(), collaborators.dimmed());
            }
        }

        // Show beam command based on local status
        match local_status {
            LocalStatus::Local => {
                println!("   {} Beam into: {}", "".green(), format!("mothership beam \"{}\"", project.name).green());
            }
            LocalStatus::NotLocal => {
                println!("   {} Beam into: {}", "".red(), format!("mothership beam \"{}\" --local-dir <path>", project.name).yellow());
                println!("   {} {}", "".yellow(), "Requires --local-dir parameter (creates project directory automatically)".dimmed());
            }
        }
    }

    println!("\n{}", "Use 'mothership beam <project-id>' to start working on a project.".dimmed());

    Ok(())
}

pub async fn handle_gateway_create(
    config_manager: &ConfigManager, 
    name: String, 
    dir: PathBuf,
    is_private: bool,
) -> Result<Project> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Err(anyhow!("Not authenticated"));
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

    // Get the active server connection (instead of hardcoded localhost)
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    print_info(&format!("Creating gateway '{}' for directory: {}", name, dir.display()));
    print_info(&format!("Server: {}", active_server.url));

    // Create project request
    let create_request = CreateGatewayRequest {
        name: name.clone(),
        description: format!("Gateway for {}", dir.display()),
        project_path: dir.clone(),
        is_private,
    };

    let create_url = format!("{}/gateway/create", active_server.url);
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
    
    // Create .mothership directory with metadata (using active server URL)
    if let Err(e) = create_gateway_metadata(&dir, &project, &active_server.url) {
        print_api_error(&format!("Warning: Failed to create .mothership directory: {}", e));
        print_info("Gateway was created successfully on the server, but local metadata may be incomplete.");
    }
    
    // Upload initial files to the server. A failed upload must fail the command,
    // otherwise the project is created with no files and beaming it gives an empty directory.
    print_info("Scanning directory for initial files...");
    match write_default_ignore_file(&dir) {
        Ok(true) => print_info(&format!("Created {} with default ignore rules", mothership_common::ignore::IGNORE_FILE_NAME)),
        Ok(false) => {}
        Err(e) => print_api_error(&format!("Warning: could not create .mothershipignore: {}", e)),
    }
    if let Err(e) = upload_initial_files(&config, &project, &dir, &active_server.url).await {
        return Err(anyhow!(
            "Gateway '{}' was created on the server (Project ID: {}), but uploading its files failed: {}. \
             Run 'mothership delete {} --force' to remove it, fix the problem, then create it again.",
            name, project.id, e, name
        ));
    }
    
    print_info(&format!("Use 'mothership beam {}' to start collaborating", project.id));

    Ok(project)
}

pub async fn handle_delete(
    config_manager: &ConfigManager,
    project_name: String,
    force: bool,
) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Please run 'mothership auth' first.");
        return Ok(());
    }

    // Get the active server connection (instead of hardcoded localhost)
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;

    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    // First, get the project by name to verify it exists
    let project_url = format!("{}/projects/name/{}", active_server.url, urlencoding::encode(&project_name));
    let response = client.get(&project_url).send().await?;

    if !response.status().is_success() {
        if response.status() == 404 {
            print_api_error(&format!("Project '{}' not found", project_name));
        } else {
            print_api_error(&format!("Failed to find project: {}", response.status()));
        }
        return Ok(());
    }

    let project_response: ApiResponse<Project> = response.json().await?;
    let project = project_response.data.ok_or_else(|| {
        anyhow!("No project data received")
    })?;

    // Show warning and confirmation unless forced
    if !force {
        println!("\n{}", " PROJECT DELETION WARNING".red().bold());
        println!("{}", format!("Project: {}", project.name.yellow().bold()));
        println!("{}", format!("Description: {}", project.description.dimmed()));
        println!("{}", format!("Project ID: {}", project.id.to_string().dimmed()));
        
        println!("\n{}", "This will permanently delete:".yellow());
        println!("{}", "  • The project from Mothership servers".dimmed());
        println!("{}", "  • All project history and checkpoints".dimmed());
        println!("{}", "  • All associated rifts and collaboration data".dimmed());
        
        println!("\n{}", "Local files will NOT be deleted - they remain on your machine.".green());
        
        print!("\n{}", "Are you sure you want to delete this project? (y/N): ".white().bold());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        if !input.trim().to_lowercase().starts_with('y') {
            print_info("Deletion cancelled.");
            return Ok(());
        }
    }

    print_info(&format!("Deleting project '{}' from server...", project.name));

    // Delete the project
    let delete_url = format!("{}/projects/{}", active_server.url, project.id);
    let response = client.delete(&delete_url).send().await?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to delete project: {}", error_text));
    }

    print_success(&format!("Project '{}' successfully deleted from Mothership server!", project.name));
    
    // Check if there's a local .mothership directory and offer to clean it up
    let current_dir = std::env::current_dir()?;
    
    // Check current directory
    let mothership_dir = current_dir.join(".mothership");
    if mothership_dir.exists() {
        if let Ok(metadata) = read_gateway_metadata(&current_dir) {
            if metadata.project_id == project.id.to_string() {
                println!("\n{}", "Local .mothership directory detected for this project.".yellow());
                print!("{}", "Would you like to remove the .mothership directory? (y/N): ".white());
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                
                if input.trim().to_lowercase().starts_with('y') {
                    match std::fs::remove_dir_all(&mothership_dir) {
                        Ok(()) => print_success("Local .mothership directory removed."),
                        Err(e) => print_api_error(&format!("Failed to remove .mothership directory: {}", e)),
                    }
                } else {
                    print_info("Local .mothership directory kept (project files remain unchanged).");
                }
            }
        }
    }

    // Also check if there's a subdirectory with the project name
    let project_subdir = current_dir.join(&project.name);
    if project_subdir.exists() && project_subdir.is_dir() {
        let subdir_mothership = project_subdir.join(".mothership");
        if subdir_mothership.exists() {
            if let Ok(metadata) = read_gateway_metadata(&project_subdir) {
                if metadata.project_id == project.id.to_string() {
                    println!("\n{}", format!("Found project directory: {}", project_subdir.display()).yellow());
                    print!("{}", "Would you like to remove the .mothership directory from it? (y/N): ".white());
                    io::stdout().flush()?;

                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    
                    if input.trim().to_lowercase().starts_with('y') {
                        match std::fs::remove_dir_all(&subdir_mothership) {
                            Ok(()) => print_success(&format!("Removed .mothership from {}", project_subdir.display())),
                            Err(e) => print_api_error(&format!("Failed to remove .mothership directory: {}", e)),
                        }
                    } else {
                        print_info(&format!("Project files in {} remain unchanged.", project_subdir.display()));
                    }
                }
            }
        }
    }

    println!("\n{}", "Project deleted successfully!".green().bold());
    println!("{}", "Your local files are safe and remain on your machine.".dimmed());

    Ok(())
}

#[derive(serde::Serialize)]
struct CreateGatewayRequest {
    name: String,
    description: String,
    project_path: PathBuf,
    is_private: bool,
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

/// Upload initial files from a directory to the server
async fn upload_initial_files(
    config: &ClientConfig,
    project: &Project,
    dir: &PathBuf,
    server_url: &str,
) -> Result<()> {
    let client = get_http_client(config);
    // Query the server for max upload size
    let max_size_url = format!("{}/max-upload-size", server_url);
    let max_size_resp = client.get(&max_size_url).send().await?;
    if !max_size_resp.status().is_success() {
        let error_text = max_size_resp.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to get max upload size: {}", error_text));
    }
    let max_size_json: ApiResponse<u64> = max_size_resp.json().await?;
    let max_size = max_size_json.data.unwrap_or(5 * 1024 * 1024); // fallback to 5MB

    let report = mothership_common::client::scan_text_files(dir, max_size)?;
    for path in &report.skipped_too_large {
        print_info(&format!("Skipped (too large): {}", path.display()));
    }
    for path in &report.skipped_binary {
        print_info(&format!("Skipped (binary): {}", path.display()));
    }
    for path in &report.skipped_unreadable {
        print_info(&format!("Skipped (unreadable): {}", path.display()));
    }
    let skipped_files = report.skipped_too_large.len();
    let files = report.files;
    let file_count = files.len();
    for path in files.keys() {
        print_info(&format!("Found: {}", path.display()));
    }

    if files.is_empty() {
        print_info("No text files found to upload");
        if skipped_files > 0 {
            print_info(&format!("{} files were skipped for exceeding the max upload size.", skipped_files));
        }
        return Ok(());
    }

    print_info(&format!("Uploading {} files to server in batches...", file_count));

    let upload_url = format!("{}/projects/{}/files", server_url, project.id);
    let mut uploaded = 0;
    let total = files.len();
    let mut batch_num = 1;

    let file_vec: Vec<_> = files.into_iter().collect();
    for batch in split_upload_batches(file_vec) {
        let batch_map: HashMap<_, _> = batch.iter().cloned().collect();
        let upload_request = UploadInitialFilesRequest {
            project_id: project.id,
            files: batch_map,
        };
        print_info(&format!("Uploading batch {} ({} files)...", batch_num, batch.len()));
        let response = client
            .post(&upload_url)
            .json(&upload_request)
            .send()
            .await?;
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Failed to upload initial files (batch {}): {}", batch_num, error_text));
        }
        uploaded += batch.len();
        print_success(&format!("Uploaded {}/{} files...", uploaded, total));
        batch_num += 1;
    }

    print_success(&format!("Successfully uploaded {} files to server!", total));
    if skipped_files > 0 {
        print_info(&format!("{} files were skipped for exceeding the max upload size.", skipped_files));
    }
    Ok(())
}

#[derive(Serialize)]
struct UploadInitialFilesRequest {
    project_id: uuid::Uuid,
    files: HashMap<PathBuf, String>,
} 

#[cfg(test)]
mod profiling_tests {
    use super::*;
    use crate::profiling::{build_project_tree, walk_entry_count, TempTree};
    use std::time::Instant;

    #[test]
    #[ignore = "profiling: run with --ignored --nocapture"]
    fn profile_gateway_project_lookup() {
        let tree = TempTree::new("gateway");
        let root = tree.0.clone();
        let names = build_project_tree(&root, 20, 200);
        let entries = walk_entry_count(&root, 3);
        let roots = vec![root];

        // Single pruned index walk (equivalent to a miss check or full index build).
        const ITERS: u32 = 5;
        let start = Instant::now();
        for _ in 0..ITERS {
            let index = crate::project_index::build_index(&roots);
            assert!(!index.contains_key("missing"));
        }
        let per_walk = start.elapsed() / ITERS;

        // Listing pattern: build the index once, then look up every project from the map.
        let start = Instant::now();
        let index = crate::project_index::build_index(&roots);
        for name in &names {
            let (status, _) = check_project_local_status(name, &index);
            assert!(matches!(status, LocalStatus::Local));
        }
        let listing = start.elapsed();

        eprintln!(
            "[profile] gateway lookup: {} projects, {entries} total entries in tree, pruned index walk = {per_walk:?}, \
             listing all projects = {listing:?} (1 walk total)",
            names.len(),
        );
    }
}