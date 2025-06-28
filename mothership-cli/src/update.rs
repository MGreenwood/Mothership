use anyhow::Result;
use clap::Args;
use colored::*;
use mothership_common::protocol::ApiResponse;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{error, info};

use crate::config::ConfigManager;

/// Update command arguments
#[derive(Args)]
pub struct UpdateArgs {
    /// Check for updates without installing
    #[arg(long)]
    pub check_only: bool,
    
    /// Force update even if current version seems newer
    #[arg(long)]
    pub force: bool,
    
    /// Show available versions
    #[arg(long)]
    pub list_versions: bool,
    
    /// Update to specific version
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCheckResponse {
    current_version: String,
    latest_version: String,
    update_available: bool,
    download_url: Option<String>,
    changes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct VersionInfo {
    version: String,
    platforms: Vec<String>,
    release_date: chrono::DateTime<chrono::Utc>,
    changes: Vec<String>,
}

/// Handle the update command
pub async fn handle_update(args: UpdateArgs) -> Result<()> {
    let config_manager = ConfigManager::new()?;
    let config = config_manager.load_config()?;
    let server_url = config.mothership_url;
    
    if args.list_versions {
        return list_available_versions(&server_url).await;
    }
    
    let current_version = env!("CARGO_PKG_VERSION");
    let platform = detect_platform();
    
    println!("{}", "🔍 Checking for updates...".blue());
    println!("Current version: {}", current_version.green());
    println!("Platform: {}", platform.cyan());
    println!("Server: {}", server_url.cyan());
    println!();
    
    // Check for updates
    let update_info = check_for_updates(&server_url, current_version, &platform).await?;
    
    if !update_info.update_available && !args.force {
        println!("{}", "✅ You're running the latest version!".green());
        return Ok(());
    }
    
    if args.check_only {
        if update_info.update_available {
            println!("{}", format!("🆕 Update available: {} → {}", 
                current_version, update_info.latest_version).yellow());
            
            if !update_info.changes.is_empty() {
                println!("\n📝 Changes:");
                for change in &update_info.changes {
                    println!("  • {}", change);
                }
            }
            
            println!("\n💡 Run 'mothership update' to install the update");
        } else {
            println!("{}", "✅ No updates available".green());
        }
        return Ok(());
    }
    
    // Perform update
    let target_version = args.version.unwrap_or(update_info.latest_version.clone());
    
    if args.force || update_info.update_available {
        println!("{}", format!("⬇️  Updating to version {}...", target_version).yellow());
        
        if !update_info.changes.is_empty() {
            println!("\n📝 What's new:");
            for change in &update_info.changes {
                println!("  • {}", change);
            }
            println!();
        }
        
        download_and_install_update(&server_url, &target_version, &platform).await?;
        
        println!("{}", "✅ Update completed successfully!".green());
        println!("🔄 Please restart any running mothership processes");
    }
    
    Ok(())
}

/// Check for updates from the server
async fn check_for_updates(
    server_url: &str,
    current_version: &str,
    platform: &str,
) -> Result<UpdateCheckResponse> {
    let client = reqwest::Client::new();
    let binary_name = if cfg!(windows) { "mothership.exe" } else { "mothership" };
    
    let url = format!("{}/cli/update-check", server_url);
    let response = client
        .get(&url)
        .query(&[
            ("current_version", current_version),
            ("platform", platform),
            ("binary", binary_name),
        ])
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Server error: {}", response.status()));
    }
    
    let api_response: ApiResponse<UpdateCheckResponse> = response.json().await?;
    
    match api_response {
        ApiResponse { success: true, data: Some(data), .. } => Ok(data),
        ApiResponse { error: Some(err), .. } => Err(anyhow::anyhow!("Server error: {}", err)),
        _ => Err(anyhow::anyhow!("Unexpected response format")),
    }
}

/// List all available versions
async fn list_available_versions(server_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    
    let url = format!("{}/cli/versions", server_url);
    let response = client.get(&url).send().await?;
    
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Server error: {}", response.status()));
    }
    
    let api_response: ApiResponse<Vec<VersionInfo>> = response.json().await?;
    
    match api_response {
        ApiResponse { success: true, data: Some(versions), .. } => {
            println!("{}", "📦 Available versions:".blue());
            println!();
            
            for version in versions.iter().rev() { // Show newest first
                println!("{} {}", "Version:".bold(), version.version.green());
                println!("  Released: {}", version.release_date.format("%Y-%m-%d %H:%M UTC"));
                println!("  Platforms: {}", version.platforms.join(", ").cyan());
                
                if !version.changes.is_empty() {
                    println!("  Changes:");
                    for change in &version.changes {
                        println!("    • {}", change);
                    }
                }
                println!();
            }
        }
        ApiResponse { error: Some(err), .. } => {
            return Err(anyhow::anyhow!("Server error: {}", err));
        }
        _ => {
            return Err(anyhow::anyhow!("Unexpected response format"));
        }
    }
    
    Ok(())
}

/// Download and install update
async fn download_and_install_update(
    server_url: &str,
    version: &str,
    platform: &str,
) -> Result<()> {
    // Get authentication token
    let token = get_auth_token()?;
    
    let client = reqwest::Client::new();
    
    // Download CLI binary
    let cli_binary = if cfg!(windows) { "mothership.exe" } else { "mothership" };
    let daemon_binary = if cfg!(windows) { "mothership-daemon.exe" } else { "mothership-daemon" };
    
    println!("⬇️  Downloading CLI...");
    download_binary(&client, server_url, version, platform, cli_binary, &token).await?;
    
    println!("⬇️  Downloading daemon...");
    download_binary(&client, server_url, version, platform, daemon_binary, &token).await?;
    
    Ok(())
}

/// Download a single binary
async fn download_binary(
    client: &reqwest::Client,
    server_url: &str,
    version: &str,
    platform: &str,
    binary_name: &str,
    token: &str,
) -> Result<()> {
    let url = format!("{}/cli/download/{}/{}/{}", server_url, version, platform, binary_name);
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Failed to download {}: {}", binary_name, response.status()));
    }
    
    let binary_data = response.bytes().await?;
    
    // Determine installation path
    let install_path = get_binary_install_path(binary_name)?;
    
    // Create backup of current binary
    if install_path.exists() {
        let backup_path = install_path.with_extension(format!("{}.backup", 
            install_path.extension().and_then(|e| e.to_str()).unwrap_or("")));
        fs::copy(&install_path, &backup_path)?;
        info!("Created backup: {}", backup_path.display());
    }
    
    // Write new binary
    fs::write(&install_path, binary_data)?;
    
    // Make executable on Unix systems
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&install_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&install_path, perms)?;
    }
    
    println!("✅ Updated: {}", install_path.display());
    
    Ok(())
}

/// Get the installation path for a binary
fn get_binary_install_path(binary_name: &str) -> Result<PathBuf> {
    // Try to find the current binary location
    if let Ok(current_exe) = std::env::current_exe() {
        let parent = current_exe.parent()
            .ok_or_else(|| anyhow::anyhow!("Could not determine parent directory"))?;
        return Ok(parent.join(binary_name));
    }
    
    // Fallback to standard locations
    if cfg!(windows) {
        let local_app_data = std::env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
        Ok(PathBuf::from(local_app_data).join("Mothership").join(binary_name))
    } else {
        Ok(PathBuf::from("/usr/local/bin").join(binary_name))
    }
}

/// Detect the current platform
fn detect_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    
    match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu".to_string(),
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu".to_string(),
        ("macos", "x86_64") => "x86_64-apple-darwin".to_string(),
        ("macos", "aarch64") => "aarch64-apple-darwin".to_string(),
        ("windows", "x86_64") => "x86_64-pc-windows-msvc".to_string(),
        _ => format!("{}-{}", arch, os), // Fallback
    }
}

/// Get authentication token from stored credentials
fn get_auth_token() -> Result<String> {
    #[derive(serde::Deserialize)]
    struct StoredCredentials {
        access_token: String,
    }
    
    let credentials_path = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
        .join("mothership")
        .join("credentials.json");
    
    if !credentials_path.exists() {
        // Try old config format
        let config_manager = ConfigManager::new()?;
        let config = config_manager.load_config()?;
        
        return config.auth_token.ok_or_else(|| {
            anyhow::anyhow!("No authentication token found. Please run 'mothership auth' first.")
        });
    }
    
    let credentials_content = std::fs::read_to_string(&credentials_path)?;
    let credentials: StoredCredentials = serde_json::from_str(&credentials_content)?;
    
    Ok(credentials.access_token)
} 