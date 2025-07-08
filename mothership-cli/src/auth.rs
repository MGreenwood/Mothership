use anyhow::Result;
use colored::*;
use mothership_common::auth::{AuthRequest, AuthResponse, TokenRequest, OAuthRequest, OAuthResponse, OAuthProvider, OAuthSource};
use mothership_common::protocol::ApiResponse;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::env;
use std::io::{self, Write};
use std::process::Command;
use tracing::{info, warn, error};
use open;

use crate::config::ConfigManager;
use crate::connections;

use uuid;
use hostname;

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
        println!("{}", format!("🌐 Using connected server: {}", server_url).dimmed());
        return Ok(server_url);
    }
    
    // Fallback to config file
    let config = config_manager.load_config()?;
    println!("{}", format!("🌐 Using config server: {}", config.mothership_url).dimmed());
    Ok(config.mothership_url)
}

/// Handle authentication with different methods
pub async fn handle_auth(config_manager: &ConfigManager, method: Option<crate::AuthMethod>) -> Result<()> {
    match method {
        Some(crate::AuthMethod::Google) | None => handle_oauth_auth(config_manager, OAuthProvider::Google).await,
        Some(crate::AuthMethod::Github) => handle_oauth_auth(config_manager, OAuthProvider::GitHub).await,
    }
}

/// Handle OAuth authentication with local callback server (like GUI)
async fn handle_oauth_auth(config_manager: &ConfigManager, provider: OAuthProvider) -> Result<()> {
    let provider_name = match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::GitHub => "GitHub",
    };

    let server_url = get_server_url(config_manager)?;

    // Start OAuth flow
    let oauth_request = OAuthRequest {
        provider,
        source: OAuthSource::CLI,
        machine_id: crate::get_machine_id()?,
        machine_name: crate::get_machine_name()?,
        platform: env::consts::OS.to_string(),
        hostname: hostname::get()?.to_string_lossy().to_string(),
        callback_url: None,
    };

    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/auth/oauth/start", server_url))
        .json(&oauth_request)
        .send()
        .await?;

    let oauth_response: ApiResponse<OAuthResponse> = response.json().await?;

    if !oauth_response.success {
        return Err(anyhow::anyhow!(oauth_response.error.unwrap_or_else(|| "Unknown error".to_string())));
    }

    let oauth_data = oauth_response.data.unwrap();

    // Open browser for OAuth flow
    println!("🔐 Opening browser for authentication...");
    if let Err(e) = open::that(&oauth_data.auth_url) {
        println!("❌ Failed to open browser automatically: {}", e);
        println!("Please open this URL manually:");
        println!("{}", oauth_data.auth_url);
    }

    println!("{}", "⏳ Please complete the login process in your browser".yellow());
    println!("{}", "   After logging in, you'll see a success page with your token".dimmed());
    println!();
    println!("{}", "📋 Copy the token from the success page and paste it here:".cyan().bold());
    print!("{}", "Token: ".cyan());

    // Read token from user input
    let mut token_input = String::new();
    std::io::stdin().read_line(&mut token_input)
        .map_err(|e| anyhow::anyhow!("Failed to read token input: {}", e))?;

    let access_token = token_input.trim();

    if access_token.is_empty() {
        return Err(anyhow::anyhow!("No token provided. Please try again."));
    }

    if access_token.len() < 50 {
        return Err(anyhow::anyhow!("Token seems too short. Please make sure you copied the full token."));
    }

    println!("{}", "🔍 Validating token with server...".dimmed());

    // Validate the token before saving
    let client = reqwest::Client::new();
    let response = client
        .get(&format!("{}/auth/check", server_url))
        .bearer_auth(access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Invalid token. Please try the authentication process again."));
    }

    // Save credentials in the same format as the GUI
    save_credentials(config_manager, access_token, None, None).await?;

    println!("{}", "✅ Authentication successful!".green().bold());
    println!("{}", format!("   Logged in via {}", provider_name).dimmed());
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
    let client = reqwest::Client::new();
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

/// Get machine information for OAuth
fn get_machine_info() -> OAuthRequest {
    let machine_id = uuid::Uuid::new_v4().to_string();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    
    OAuthRequest {
        provider: OAuthProvider::Google, // Will be overridden
        machine_id,
        machine_name: format!("{}-mothership-cli", hostname),
        platform: std::env::consts::OS.to_string(),
        hostname,
        source: OAuthSource::CLI,
        callback_url: None,
    }
}

/// Handle logout (clear stored credentials)
pub async fn handle_logout(config_manager: &ConfigManager) -> Result<()> {
    println!("{}", "🗑️  Clearing stored credentials...".dimmed());
    
    // Clear stored credentials
    clear_stored_credentials(config_manager).await?;
    
    println!("{}", "✅ Logged out successfully!".green().bold());
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

/// Authenticate with the Mothership server
pub async fn authenticate(config_manager: &crate::ConfigManager) -> Result<()> {
    // Use the same OAuth flow as handle_oauth_auth
    handle_oauth_auth(config_manager, OAuthProvider::Google).await
}

/// Check if we have a valid auth token
pub fn check_auth(config_manager: &ConfigManager) -> bool {
    config_manager.get_auth().is_ok()
} 