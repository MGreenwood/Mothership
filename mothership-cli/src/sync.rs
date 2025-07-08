use anyhow::{anyhow, Result};
use colored::*;
use mothership_common::{Checkpoint, protocol::ApiResponse};
use serde::{Serialize, Deserialize};
use std::io::{self, Write};
use uuid;

use crate::{config::ConfigManager, get_http_client, print_api_error, print_info, print_success, connections};

/// Get the server URL to use for sync operations
/// Prioritizes active server connection over config file
fn get_server_url(config_manager: &ConfigManager) -> Result<String> {
    // First, check if there's an active server connection
    if let Some(server_url) = connections::get_active_server_url() {
        return Ok(server_url);
    }
    
    // Fallback to config file
    let config = config_manager.load_config()?;
    Ok(config.mothership_url)
}

pub async fn handle_status(config_manager: &ConfigManager) -> Result<()> {
    use reqwest::StatusCode;
    use std::fs;

    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_info("Not authenticated. Run 'mothership auth' to get started.");
        return Ok(());
    }

    // 1. Show current project and rift
    let project_metadata = crate::sync::find_current_project()
        .map(|(project_id, project_name)| (project_id, project_name))
        .ok();
    let local_metadata: Option<crate::sync::ProjectMetadata> = fs::read_to_string(".mothership/project.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    if let Some((project_id, ref project_name)) = project_metadata {
        println!("\n{} {}", "Project:".bold(), project_name.blue().bold());
        println!("{} {}", "ID:".bold(), project_id.to_string().dimmed());
        
        // Show rift info if available (from local metadata)
        if let Some(meta) = local_metadata {
            println!("{} {}", "Server:".bold(), meta.mothership_url.dimmed());
        }
    } else {
        println!("\n{} {}", "Project:".bold(), "Not in a project directory".red());
        println!("{}", "Run 'mothership beam <project>' to enter a project".dimmed());
    }

    // 2. Query daemon for status
    let daemon_status = reqwest::get("http://127.0.0.1:7525/status").await;
    match daemon_status {
        Ok(resp) if resp.status() == StatusCode::OK => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(data) = json.get("data") {
                println!("\n{}", "Daemon Status:".bold());
                println!("  {} {}", "Running:".dimmed(), data.get("is_running").unwrap_or(&serde_json::Value::Null));
                println!("  {} {}", "Projects Tracked:".dimmed(), data.get("projects_tracked").unwrap_or(&serde_json::Value::Null));
                println!("  {} {}", "Files Syncing:".dimmed(), data.get("files_syncing").unwrap_or(&serde_json::Value::Null));
                println!("  {} {}", "Last Sync:".dimmed(), data.get("last_sync").unwrap_or(&serde_json::Value::Null));
                println!("  {} {}", "Server Connected:".dimmed(), data.get("server_connected").unwrap_or(&serde_json::Value::Null));
            }
        }
        _ => {
            println!("\n{}", "Daemon not running or status unavailable.".yellow());
        }
    }

    // 3. Show recent checkpoints (last 3)
    if let Some((project_id, project_name)) = project_metadata {
        let config = config_manager.load_config()?;
        let server_url = get_server_url(config_manager)?;
        let client = get_http_client(&config);
        let history_url = format!("{}/projects/{}/history?limit=3", server_url, project_id);
        let response = client.get(&history_url).send().await;
        if let Ok(resp) = response {
            if resp.status().is_success() {
                let checkpoints: ApiResponse<Vec<Checkpoint>> = resp.json().await.unwrap_or(ApiResponse { 
                    success: false, 
                    data: None, 
                    error: Some("Failed to parse response".to_string()),
                    message: Some("Failed to parse response".to_string()),
                });
                if let Some(checkpoints) = checkpoints.data {
                    println!("\n{}", "Recent Checkpoints:".bold());
                    for checkpoint in checkpoints.iter() {
                        let age = crate::sync::format_time_ago(checkpoint.timestamp);
                        let message = checkpoint.message.as_deref().unwrap_or("(no message)");
                        let auto_marker = if checkpoint.auto_generated { " [auto]" } else { "" };
                        println!("  {} {} {}{}", checkpoint.id.to_string()[..8].yellow(), message.white(), age.dimmed(), auto_marker.dimmed());
                    }
                }
            }
        }
    }

    // 4. Show connected collaborators (if available)
    // Placeholder: This would require a new API endpoint or WebSocket presence tracking
    println!("\n{}", "Connected Collaborators:".bold());
    println!("  {}", "(Feature coming soon: will show live users in this rift)".dimmed());

    Ok(())
}

pub async fn handle_checkpoint(config_manager: &ConfigManager, message: Option<String>) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Run 'mothership auth' to get started.");
        return Ok(());
    }

    // Find the current project
    let (project_id, project_name) = find_current_project()?;
    let checkpoint_msg = message.unwrap_or_else(|| "Manual checkpoint".to_string());
    
    print_info(&format!("Creating checkpoint for {}: {}", project_name, checkpoint_msg));

    let config = config_manager.load_config()?;
    let server_url = get_server_url(config_manager)?;
    let client = get_http_client(&config);

    // Create checkpoint via API
    let checkpoint_url = format!("{}/projects/{}/checkpoint", server_url, project_id);
    let response = client
        .post(&checkpoint_url)
        .json(&serde_json::json!({
            "message": checkpoint_msg,
            "timestamp": chrono::Utc::now()
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to create checkpoint: {}", response.status()));
    }

    let checkpoint_response: ApiResponse<CheckpointData> = response.json().await?;
    let checkpoint_data = checkpoint_response.data.ok_or_else(|| {
        anyhow!("No checkpoint data received: {}", checkpoint_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    print_success(&format!("✅ Checkpoint {} created", &checkpoint_data.checkpoint_id.to_string()[..8]));
    print_info(&format!("📸 Captured {} file changes", checkpoint_data.file_count));
    
    Ok(())
}

pub async fn handle_sync(config_manager: &ConfigManager) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_info("Not authenticated. Run 'mothership auth' to get started.");
        return Ok(());
    }

    print_info("Syncing with remote Mothership...");
    println!("{}", "Sync functionality not yet implemented".dimmed());
    println!("{}", "In a full implementation, this would:".dimmed());
    println!("{}", "  • Pull latest changes from server".dimmed());
    println!("{}", "  • Push local changes to server".dimmed());
    println!("{}", "  • Resolve any conflicts".dimmed());
    println!("{}", "  • Update collaboration state".dimmed());

    Ok(())
}

pub async fn handle_history(config_manager: &ConfigManager, limit: usize) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Run 'mothership auth' to get started.");
        return Ok(());
    }

    // Find the current project
    let (project_id, project_name) = find_current_project()?;
    print_info(&format!("Loading history for project: {}", project_name));

    let config = config_manager.load_config()?;
    let server_url = get_server_url(config_manager)?;
    let client = get_http_client(&config);

    // Get checkpoint history from server
    let history_url = format!("{}/projects/{}/history?limit={}", server_url, project_id, limit);
    let response = client.get(&history_url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to load history: {}", response.status()));
    }

    let history_response: ApiResponse<Vec<Checkpoint>> = response.json().await?;
    let checkpoints = history_response.data.ok_or_else(|| {
        anyhow!("No history data received: {}", history_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    if checkpoints.is_empty() {
        print_info("No checkpoints found. Create your first checkpoint with 'mothership checkpoint \"message\"'");
        return Ok(());
    }

    // Display checkpoint history
    println!("\n{}", "📜 Project History".cyan().bold());
    println!("{}", format!("Showing {} most recent checkpoints for {}", checkpoints.len(), project_name.blue().bold()));

    for (i, checkpoint) in checkpoints.iter().enumerate() {
        let age = format_time_ago(checkpoint.timestamp);
        let message = checkpoint.message.as_deref().unwrap_or("(no message)");
        let auto_marker = if checkpoint.auto_generated { " [auto]" } else { "" };
        
        println!("\n{} {} {} {}", 
            if i == 0 { "●".green() } else { "○".dimmed() },
            checkpoint.id.to_string()[..8].yellow().bold(),
            message.white(),
            auto_marker.dimmed()
        );
        println!("   {} • {} file{} changed",
            age.dimmed(),
            checkpoint.changes.len(),
            if checkpoint.changes.len() == 1 { "" } else { "s" }
        );
        
        // Show file changes (first few)
        let display_changes = checkpoint.changes.iter().take(3);
        for change in display_changes {
            let change_icon = match change.change_type {
                mothership_common::ChangeType::Created => "+".green(),
                mothership_common::ChangeType::Modified => "~".yellow(),
                mothership_common::ChangeType::Deleted => "-".red(),
                mothership_common::ChangeType::Moved { .. } => "→".blue(),
            };
            println!("     {} {}", change_icon, change.path.display().to_string().dimmed());
        }
        
        if checkpoint.changes.len() > 3 {
            println!("     {} {} more files...", "...".dimmed(), checkpoint.changes.len() - 3);
        }
    }

    println!("\n{}", "💡 Use 'mothership restore <checkpoint-id>' to restore to a specific point".dimmed());
    Ok(())
}

pub async fn handle_restore(config_manager: &ConfigManager, checkpoint_id: String, force: bool) -> Result<()> {
    // Check if authenticated
    if !config_manager.is_authenticated()? {
        print_api_error("Not authenticated. Run 'mothership auth' to get started.");
        return Ok(());
    }

    // Find the current project
    let (project_id, project_name) = find_current_project()?;

    // Parse checkpoint ID
    let checkpoint_uuid = uuid::Uuid::parse_str(&checkpoint_id)
        .map_err(|_| anyhow!("Invalid checkpoint ID format. Use the full checkpoint ID from 'mothership history'"))?;

    if !force {
        println!("\n{}", "⚠️  This will overwrite your current files with the checkpoint state.".yellow().bold());
        println!("{}", format!("Project: {}", project_name.blue().bold()));
        println!("{}", format!("Checkpoint: {}", checkpoint_id.yellow()));
        print!("{}", "Are you sure you want to continue? (y/N): ".white().bold());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        if !input.trim().to_lowercase().starts_with('y') {
            print_info("Restore cancelled.");
            return Ok(());
        }
    }

    let config = config_manager.load_config()?;
    let server_url = get_server_url(config_manager)?;
    let client = get_http_client(&config);

    print_info(&format!("Restoring to checkpoint {}...", &checkpoint_id[..8]));

    // Request checkpoint files from server
    let restore_url = format!("{}/projects/{}/restore/{}", server_url, project_id, checkpoint_uuid);
    let response = client.post(&restore_url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to restore checkpoint: {}", response.status()));
    }

    let restore_response: ApiResponse<RestoreData> = response.json().await?;
    let restore_data = restore_response.data.ok_or_else(|| {
        anyhow!("No restore data received: {}", restore_response.error.unwrap_or_else(|| "Unknown error".to_string()))
    })?;

    // Get current directory (should be project root)
    let current_dir = std::env::current_dir()?;

    print_info(&format!("Restoring {} files...", restore_data.files.len()));

    // Write files to disk
    for (relative_path, content) in restore_data.files {
        let file_path = current_dir.join(&relative_path);
        
        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Write file content
        std::fs::write(&file_path, &content)?;
        print_info(&format!("Restored: {}", relative_path.display()));
    }

    print_success(&format!("Successfully restored to checkpoint {} ({})", 
        &checkpoint_id[..8], 
        restore_data.checkpoint.message.as_deref().unwrap_or("no message")
    ));
    
    print_info("Files have been restored. Use 'mothership status' to see current state.");
    Ok(())
}

/// Find the current project by looking for .mothership/project.json
fn find_current_project() -> Result<(uuid::Uuid, String)> {
    let current_dir = std::env::current_dir()?;
    let mothership_dir = current_dir.join(".mothership");
    let project_file = mothership_dir.join("project.json");

    if !project_file.exists() {
        return Err(anyhow!(
            "Not in a Mothership project directory.\n\
            Run this command from a project directory, or use 'mothership beam <project>' to enter a project."
        ));
    }

    let project_content = std::fs::read_to_string(&project_file)?;
    let project_metadata: ProjectMetadata = serde_json::from_str(&project_content)?;

    let project_id = uuid::Uuid::parse_str(&project_metadata.project_id)?;
    Ok((project_id, project_metadata.project_name))
}

/// Format timestamp as "X minutes/hours/days ago"
fn format_time_ago(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(timestamp);

    if let Ok(std_duration) = duration.to_std() {
        let seconds = std_duration.as_secs();
        
        if seconds < 60 {
            format!("{}s ago", seconds)
        } else if seconds < 3600 {
            format!("{}m ago", seconds / 60)
        } else if seconds < 86400 {
            format!("{}h ago", seconds / 3600)
        } else {
            format!("{}d ago", seconds / 86400)
        }
    } else {
        "just now".to_string()
    }
}

#[derive(Serialize, Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
}

#[derive(Serialize, Deserialize)]
struct RestoreData {
    checkpoint: Checkpoint,
    files: std::collections::HashMap<std::path::PathBuf, String>,
}

#[derive(Serialize, Deserialize)]
struct CheckpointData {
    checkpoint_id: uuid::Uuid,
    file_count: usize,
} 