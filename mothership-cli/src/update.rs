use anyhow::Result;
use clap::Args;
use colored::*;
use mothership_common::protocol::ApiResponse;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

use crate::config::ConfigManager;
use crate::connections;

/// Get the server URL to use for updates
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
    let server_url = get_server_url(&config_manager)?;
    
    if args.list_versions {
        return list_available_versions(&server_url).await;
    }
    
    let current_version = env!("CARGO_PKG_VERSION");
    let platform = detect_platform();
    
    println!("{}", "Getting latest version...".blue());
    println!("Current version: {}", current_version.green());
    println!("Platform: {}", platform.cyan());
    println!("Server: {}", server_url.cyan());
    println!();
    
    // Always get the absolute latest version directly from server
    let latest_info = get_latest_version_direct(&server_url).await?;
    let latest_version = latest_info.version.clone();
    
    // Compare versions using semantic version comparison
    let update_available = current_version != latest_version;
    
    // Check if specific version was requested
    let version_specified = args.version.is_some();
    
    // Determine target version - use specified version or latest
    let target_version = args.version.unwrap_or(latest_version.clone());
    
    if !update_available && !args.force && !version_specified {
        println!("{}", "You're running the latest version!".green());
        return Ok(());
    }
    
    if args.check_only {
        if update_available || version_specified {
            println!("{}", format!("Update available: {} → {}", 
                current_version, target_version).yellow());
            
            if target_version == latest_version && !latest_info.changes.is_empty() {
                println!("\nChanges:");
                for change in &latest_info.changes {
                    println!("  • {}", change);
                }
            }
            
            println!("\nRun 'mothership update' to install the update");
        } else {
            println!("{}", "No updates available".green());
        }
        return Ok(());
    }
    
    // Perform update if needed or forced
    if args.force || update_available || version_specified {
        println!("{}", format!("⬇ Updating to version {}...", target_version).yellow());
        

        
        let restart_daemon = stop_daemon_for_update().await?;
        if let Err(e) = download_and_install_update(&server_url, &target_version, &platform, restart_daemon).await {
            if restart_daemon {
                let _ = crate::beam::ensure_daemon_running().await;
            }
            return Err(e);
        }
        // A CLI self-update exits the process inside download_and_install_update; its batch script restarts the daemon in that case.
        if restart_daemon {
            if let Err(e) = crate::beam::ensure_daemon_running().await {
                println!(" Could not restart daemon: {}", e);
            }
        }
        
        println!("{}", "Update completed successfully!".green());
        println!("Please restart any running mothership processes");
    }
    
    Ok(())
}

/// Get the latest version directly from the server (bypasses incremental updates)
async fn get_latest_version_direct(server_url: &str) -> Result<VersionInfo> {
    // Get authentication token
    let token = get_auth_token()?;
    
    let client = crate::http_client();
    
    let url = format!("{}/cli/latest", server_url);
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Server error: {}", response.status()));
    }

    let api_response: ApiResponse<VersionInfo> = response.json().await?;

    match api_response {
        ApiResponse { success: true, data: Some(data), .. } => Ok(data),
        ApiResponse { error: Some(err), .. } => Err(anyhow::anyhow!("Server error: {}", err)),
        _ => Err(anyhow::anyhow!("Unexpected response format")),
    }
}

/// Check for updates from the server (legacy function for backward compatibility)
async fn check_for_updates(
    server_url: &str,
    current_version: &str,
    platform: &str,
) -> Result<UpdateCheckResponse> {
    // Get authentication token
    let token = get_auth_token()?;
    
    let client = crate::http_client();
    let binary_name = if cfg!(windows) { "mothership.exe" } else { "mothership" };
    
    let url = format!("{}/cli/update-check", server_url);
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
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
    // Get authentication token
    let token = get_auth_token()?;
    
    let client = crate::http_client();
    
    let url = format!("{}/cli/versions", server_url);
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Server error: {}", response.status()));
    }

    let api_response: ApiResponse<Vec<VersionInfo>> = response.json().await?;

    match api_response {
        ApiResponse { success: true, data: Some(versions), .. } => {
            println!("{}", "Available versions:".blue());
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
    restart_daemon: bool,
) -> Result<()> {
    // Get authentication token
    let token = get_auth_token()?;
    
    let client = crate::http_client();
    
    // Download CLI binary
    let cli_binary = if cfg!(windows) { "mothership.exe" } else { "mothership" };
    let daemon_binary = if cfg!(windows) { "mothership-daemon.exe" } else { "mothership-daemon" };
    
    // Verify binaries exist on server before downloading
    let cli_url = format!("{}/cli/download/{}/{}/{}", server_url, version, platform, cli_binary);
    let daemon_url = format!("{}/cli/download/{}/{}/{}", server_url, version, platform, daemon_binary);
    
    // Check CLI binary
    let cli_response = client.head(&cli_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;
    
    if !cli_response.status().is_success() {
        return Err(anyhow::anyhow!("CLI binary not found on server for version {} ({})", version, platform));
    }
    
    // Check daemon binary
    let daemon_response = client.head(&daemon_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;
    
    if !daemon_response.status().is_success() {
        return Err(anyhow::anyhow!("Daemon binary not found on server for version {} ({})", version, platform));
    }
    
    println!("⬇ Downloading CLI...");
    download_binary_safe(&client, server_url, version, platform, cli_binary, &token).await?;
    
    println!("⬇ Downloading daemon...");
    download_binary_safe(&client, server_url, version, platform, daemon_binary, &token).await?;
    
    // Handle self-update for CLI binary
    let cli_install_path = get_binary_install_path(cli_binary)?;
    if is_self_update(&cli_install_path)? {
        let daemon_restart_path = if restart_daemon {
            Some(get_binary_install_path(daemon_binary)?)
        } else {
            None
        };
        return perform_self_update(&cli_install_path, daemon_restart_path).await;
    }
    
    Ok(())
}

/// Check if this is a self-update (CLI updating itself)
fn is_self_update(install_path: &PathBuf) -> Result<bool> {
    let current_exe = std::env::current_exe()?;
    Ok(current_exe == *install_path)
}

/// Perform a self-update using a restart script
async fn perform_self_update(install_path: &PathBuf, daemon_restart_path: Option<PathBuf>) -> Result<()> {
    println!("This is a self-update. Creating restart script...");
    
    let temp_dir = std::env::temp_dir();
    let new_binary_path = temp_dir.join("mothership-new.exe");
    let restart_script_path = temp_dir.join("mothership-restart.bat");
    
    // Move the downloaded binary to temp location
    let temp_download_path = temp_dir.join("mothership-downloaded.exe");
    if temp_download_path.exists() {
        std::fs::rename(&temp_download_path, &new_binary_path)?;
    } else {
        return Err(anyhow::anyhow!("Downloaded binary not found in temp location"));
    }
    
    // Create restart script with properly escaped paths
    let restart_script = format!("@echo off\r\n\
echo Waiting for mothership process to exit...\r\n\
timeout /t 2 /nobreak >nul\r\n\
\r\n\
echo Replacing mothership binary...\r\n\
copy /Y \"{new_binary}\" \"{install_path}\" >nul\r\n\
if errorlevel 1 (\r\n\
    echo Failed to replace binary. Please try again.\r\n\
    pause\r\n\
    exit /b 1\r\n\
)\r\n\
\r\n\
echo Cleaning up...\r\n\
echo Update completed successfully!\r\n\
echo Press enter to use the new version of mothership.\r\n\
del \"{new_binary}\" >nul 2>&1\r\n\
del \"%~f0\" >nul 2>&1\r\n\
\r\n\
pause\r\n", 
        new_binary = new_binary_path.to_str().unwrap().replace("/", "\\"),
        install_path = install_path.to_str().unwrap().replace("/", "\\")
    );
    
    // Write script with Windows line endings
    use std::io::Write;
    let mut file = std::fs::File::create(&restart_script_path)?;
    file.write_all(restart_script.as_bytes())?;
    file.flush()?;
    
    println!("Update downloaded successfully!");
    println!("Running update script...");

    // Start the daemon before this process exits; the new binary is already in place.
    if let Some(path) = daemon_restart_path {
        crate::beam::try_start_daemon(&path)?;
    }
    
    // Run the restart script
    let _status = std::process::Command::new("cmd")
        .arg("/C")
        .arg(&restart_script_path)
        .spawn()?;
        
    // Exit this process to allow the script to replace the binary
    std::process::exit(0)
}

/// Stops the running daemon so its binary can be replaced. Returns true if it was running.
async fn stop_daemon_for_update() -> Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let health_url = "http://localhost:7525/health";

    if !is_daemon_healthy(&client, health_url).await {
        return Ok(false);
    }

    println!(" Stopping daemon before update...");
    let _ = client.post("http://localhost:7525/shutdown").send().await;

    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if !is_daemon_healthy(&client, health_url).await {
            // Let the process finish exiting so it releases the binary file.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            return Ok(true);
        }
    }

    Err(anyhow::anyhow!(
        "Daemon did not stop within 9 seconds. Run 'mothership daemon stop' and retry the update."
    ))
}

async fn is_daemon_healthy(client: &reqwest::Client, url: &str) -> bool {
    matches!(client.get(url).send().await, Ok(response) if response.status().is_success())
}

/// Download a single binary with safe self-update handling
async fn download_binary_safe(
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
    
    // Check if this is a self-update
    if is_self_update(&install_path)? {
        // For self-update, download to temp location first
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join("mothership-downloaded.exe");
        
        // Create backup of current binary
        if install_path.exists() {
            let backup_path = install_path.with_extension(format!("{}.backup", 
                install_path.extension().and_then(|e| e.to_str()).unwrap_or("")));
            fs::copy(&install_path, &backup_path)?;
            info!("Created backup: {}", backup_path.display());
        }
        
        // Write new binary to temp location
        fs::write(&temp_path, binary_data)?;
        println!("Downloaded to temp location: {}", temp_path.display());
    } else {
        // For non-self-update (like daemon), proceed normally
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
        
        println!("Updated: {}", install_path.display());
    }
    
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