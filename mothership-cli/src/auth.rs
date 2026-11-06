use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};
use std::fs;
use open;

use crate::config::ConfigManager;
use crate::connections;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentials {
    access_token: String,
    user_email: Option<String>,
    user_name: Option<String>,
    stored_at: String,
}

/// Get the server URL to use for authentication
/// Prioritizes active server connection over config file
fn get_server_url(config_manager: &ConfigManager) -> Result<String> {
    // First, check if there's an active server connection
    if let Some(server_url) = connections::get_active_server_url() {
        println!("{}", format!("Using connected server: {}", server_url).dimmed());
        return Ok(server_url);
    }
    
    // Fallback to config file
    let config = config_manager.load_config()?;
    println!("{}", format!("Using config server: {}", config.mothership_url).dimmed());
    Ok(config.mothership_url)
}

/// Handle authentication. Google and GhostID both sign in through the browser callback flow.
pub async fn handle_auth(config_manager: &ConfigManager, method: Option<crate::AuthMethod>) -> Result<()> {
    match method {
        None => handle_browser_login(config_manager, None).await,
        Some(crate::AuthMethod::Ghostid) => handle_browser_login(config_manager, Some("ghostid")).await,
        Some(crate::AuthMethod::Google) => handle_browser_login(config_manager, Some("google")).await,
    }
}

/// Browser login with a loopback callback. The CLI opens the browser and receives the token on
/// the callback. Nothing is pasted.
async fn handle_browser_login(config_manager: &ConfigManager, provider: Option<&str>) -> Result<()> {
    let server_url = get_server_url(config_manager)?;
    let login = mothership_common::client::start_browser_login(&server_url, provider).await?;

    println!("{}", "Opening your browser to sign in...".cyan());
    if let Err(e) = open::that(&login.auth_url) {
        println!("Could not open the browser ({}). Open this link manually:", e);
        println!("{}", login.auth_url);
    }
    println!("{}", "Waiting for sign-in to finish (up to 5 minutes)...".dimmed());

    let result = login.wait_for_token().await?;
    let username = result.username.clone();
    save_credentials(config_manager, &result.token, result.email.clone(), username.clone()).await?;

    println!("{}", "Authentication successful!".green().bold());
    println!("{}", format!("   Signed in as {}", username.unwrap_or_else(|| "unknown".to_string())).dimmed());
    println!("{}", "   Credentials saved for future use".dimmed());
    Ok(())
}

/// Try to auto-login using stored credentials
pub async fn try_auto_login(config_manager: &ConfigManager) -> Result<bool> {
    let creds_path = config_manager.get_credentials_path()?;
    
    if !creds_path.exists() {
        return Ok(false);
    }
    
    let creds_json = fs::read_to_string(creds_path)?;
    let creds: StoredCredentials = serde_json::from_str(&creds_json)?;
    
    // Verify the token is still valid
    let server_url = get_server_url(config_manager)?;
    let client = crate::http_client();
    let response = client
        .post(&format!("{}/auth/verify", server_url))
        .json(&creds.access_token)
        .send()
        .await?;
        
    Ok(response.status().is_success())
}

/// Save credentials to disk
async fn save_credentials(
    config_manager: &ConfigManager,
    access_token: &str,
    user_email: Option<String>,
    user_name: Option<String>,
) -> Result<()> {
    let creds = StoredCredentials {
        access_token: access_token.to_string(),
        user_email,
        user_name,
        stored_at: chrono::Utc::now().to_rfc3339(),
    };
    
    let creds_json = serde_json::to_string(&creds)?;
    let creds_path = config_manager.get_credentials_path()?;
    
    // Ensure parent directory exists
    if let Some(parent) = creds_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    fs::write(creds_path, creds_json)?;
    Ok(())
}

/// Handle logout (clear stored credentials)
pub async fn handle_logout(config_manager: &ConfigManager) -> Result<()> {
    println!("{}", " Clearing stored credentials...".dimmed());
    
    // Clear stored credentials
    clear_stored_credentials(config_manager).await?;
    
    println!("{}", "Logged out successfully!".green().bold());
    println!("{}", "   All stored credentials have been removed".dimmed());
    println!("{}", "   Use 'mothership auth' to sign in again".dimmed());
    
    Ok(())
}

/// Clear stored credentials
async fn clear_stored_credentials(config_manager: &ConfigManager) -> Result<()> {
    let creds_path = config_manager.get_credentials_path()?;
    
    if creds_path.exists() {
        fs::remove_file(&creds_path)?;
    }
    
    // Also clear the old config format
    config_manager.clear_auth()?;
    
    Ok(())
}

/// Check if we have a valid auth token
pub fn check_auth(config_manager: &ConfigManager) -> bool {
    config_manager.get_auth().is_ok()
} 