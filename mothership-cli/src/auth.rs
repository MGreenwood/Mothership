use anyhow::Result;
use colored::*;
use mothership_common::auth::{AuthRequest, AuthResponse, TokenRequest, TokenResponse, OAuthRequest, OAuthResponse, OAuthProvider, OAuthSource};

use uuid;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::io::Write;

use crate::config::ConfigManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentials {
    access_token: String,
    user_email: Option<String>,
    user_name: Option<String>,
    stored_at: String,
}

// Removed AuthState - no longer needed for token paste approach

/// Handle authentication with different methods
pub async fn handle_auth(config_manager: &ConfigManager, method: Option<crate::AuthMethod>) -> Result<()> {
    match method {
        Some(crate::AuthMethod::Google) => handle_oauth_auth(config_manager, OAuthProvider::Google).await,
        Some(crate::AuthMethod::Github) => handle_oauth_auth(config_manager, OAuthProvider::GitHub).await,
        Some(crate::AuthMethod::Device) | None => {
            handle_device_auth(config_manager).await
        }
    }
}

/// Handle device flow authentication using local auth server
async fn handle_device_auth(config_manager: &ConfigManager) -> Result<()> {
    println!("{}", "🔐 Starting device authentication...".cyan().bold());

    // Try auto-login first
    if let Ok(true) = try_auto_login(config_manager).await {
        println!("{}", "✅ Already authenticated! Using stored credentials.".green().bold());
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let machine_info = get_machine_info();
    
    // Step 1: Request device code from Mothership server
    println!("{}", "📱 Requesting device authorization...".dimmed());
    
    let auth_request = AuthRequest {
        machine_id: machine_info.machine_id.clone(),
        machine_name: machine_info.machine_name.clone(),
        platform: machine_info.platform.clone(),
        hostname: machine_info.hostname.clone(),
    };
    
    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/auth/start", config.mothership_url))
        .json(&auth_request)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow::anyhow!("Device authorization request failed: {}", error_text));
    }

    let device_response: mothership_common::protocol::ApiResponse<AuthResponse> = response.json().await?;
    
    if !device_response.success {
        return Err(anyhow::anyhow!("Device authorization failed: {}", 
            device_response.error.unwrap_or_default()));
    }

    let auth_data = device_response.data.unwrap();
    let device_code = &auth_data.device_code;
    let verification_url = &auth_data.auth_url;

    // Step 2: Show user the verification URL
    println!();
    println!("{}", "🌐 Complete authorization in your browser:".green().bold());
    println!("{}", format!("   {}", verification_url).cyan());
    println!();
    println!("{}", "⏳ Waiting for authorization... (Press Ctrl+C to cancel)".dimmed());

    // Try to open browser automatically
    if let Err(e) = open::that(verification_url) {
        println!("{}", format!("⚠️  Failed to open browser automatically: {}", e).yellow());
    }

    // Step 3: Poll for authorization completion
    let poll_interval = auth_data.interval;
    let expires_in = auth_data.expires_in;
    
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(expires_in);
    
    loop {
        if start_time.elapsed() > timeout {
            return Err(anyhow::anyhow!("Device authorization timed out. Please try again."));
        }

        tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;

        // Poll the token endpoint
        let token_request = TokenRequest {
            device_code: device_code.clone(),
        };
        
        let poll_response = client
            .post(&format!("{}/auth/token", config.mothership_url))
            .json(&token_request)
            .send()
            .await?;

        let poll_result: mothership_common::protocol::ApiResponse<TokenResponse> = poll_response.json().await?;

        if poll_result.success {
            // Success! Extract the access token
            let token_data = poll_result.data.unwrap();
            
            let user_email = None; // Device flow doesn't provide email
            let user_name = Some(token_data.username);

            // Save credentials
            save_credentials(config_manager, &token_data.access_token, user_email, user_name).await?;
            
            println!();
            println!("{}", "✅ Authentication successful!".green().bold());
            println!("{}", "   Device authorized and credentials saved".dimmed());
            
            return Ok(());
        } else {
            // Check for specific error codes
            let error = poll_result.error.as_deref().unwrap_or("");
            match error {
                "Authorization pending" => {
                    // Still waiting for user to authorize
                    print!(".");
                    std::io::Write::flush(&mut std::io::stdout()).unwrap();
                    continue;
                }
                "Access denied" => {
                    return Err(anyhow::anyhow!("Authorization was denied. Please try again."));
                }
                "Expired token" => {
                    return Err(anyhow::anyhow!("Device code expired. Please try again."));
                }
                _ => {
                    return Err(anyhow::anyhow!("Authorization failed: {}", error));
                }
            }
        }
    }
}

/// Handle OAuth authentication with local callback server (like GUI)
async fn handle_oauth_auth(config_manager: &ConfigManager, provider: OAuthProvider) -> Result<()> {
    let provider_name = match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::GitHub => "GitHub",
    };

    println!("{}", format!("🔐 Authenticating with {}...", provider_name).cyan().bold());

    // Try auto-login first
    if let Ok(true) = try_auto_login(config_manager).await {
        println!("{}", "✅ Already authenticated! Using stored credentials.".green().bold());
        return Ok(());
    }

    let config = config_manager.load_config()?;
    let machine_info = get_machine_info();
    
    // Step 1: Start OAuth flow
    let oauth_request = OAuthRequest {
        provider,
        machine_id: machine_info.machine_id.clone(),
        machine_name: machine_info.machine_name.clone(),
        platform: machine_info.platform.clone(),
        hostname: machine_info.hostname.clone(),
        source: OAuthSource::CLI,  // Indicate this is CLI-initiated OAuth
    };

    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/auth/oauth/start", config.mothership_url))
        .json(&oauth_request)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow::anyhow!("OAuth start failed: {}", error_text));
    }

    let oauth_response: mothership_common::protocol::ApiResponse<OAuthResponse> = response.json().await?;
    
    if !oauth_response.success {
        return Err(anyhow::anyhow!("OAuth start failed: {}", oauth_response.error.unwrap_or_default()));
    }

    let oauth_data = oauth_response.data.unwrap();

    // Step 2: Open browser  
    println!("{}", format!("🌐 Opening {} login in your browser...", provider_name).green());
    println!("{}", format!("If the browser doesn't open automatically, visit: {}", oauth_data.auth_url).dimmed());
    
    if let Err(e) = open::that(&oauth_data.auth_url) {
        println!("{}", format!("⚠️  Failed to open browser automatically: {}", e).yellow());
        println!("{}", format!("Please manually open: {}", oauth_data.auth_url).cyan());
    }

    // Step 3: Ask user to paste the token
    println!();
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
        .get(&format!("{}/auth/check", config.mothership_url))
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

/// Try to use stored credentials for automatic login (like GUI)
async fn try_auto_login(config_manager: &ConfigManager) -> Result<bool> {
    println!("{}", "🔍 Checking for stored credentials...".dimmed());
    
    let credentials = match load_credentials(config_manager)? {
        Some(creds) => creds,
        None => {
            println!("{}", "   No stored credentials found".dimmed());
            return Ok(false);
        }
    };
    
    println!("{}", "   Found stored credentials, validating...".dimmed());
    
    // Validate token with server
    let config = config_manager.load_config()?;
    let client = reqwest::Client::new();
    
    let response = client
        .get(&format!("{}/auth/check", config.mothership_url))
        .bearer_auth(&credentials.access_token)
        .send()
        .await?;

    if response.status().is_success() {
        println!("{}", "   ✅ Stored credentials are valid!".green());
        Ok(true)
    } else {
        println!("{}", "   ❌ Stored credentials are invalid, removing...".yellow());
        clear_stored_credentials(config_manager).await?;
        Ok(false)
    }
}

// Removed callback server functions - using manual token paste instead

/// Save credentials in the same format as the GUI
async fn save_credentials(
    config_manager: &ConfigManager, 
    access_token: &str, 
    user_email: Option<String>, 
    user_name: Option<String>
) -> Result<()> {
    let credentials = StoredCredentials {
        access_token: access_token.to_string(),
        user_email,
        user_name,
        stored_at: chrono::Utc::now().to_rfc3339(),
    };
    
    let credentials_path = get_credentials_file_path(config_manager)?;
    
    // Create directory if it doesn't exist
    if let Some(parent) = credentials_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let credentials_json = serde_json::to_string_pretty(&credentials)?;
    fs::write(&credentials_path, credentials_json)?;
    
    println!("{}", format!("💾 Credentials saved to: {}", credentials_path.display()).dimmed());
    Ok(())
}

/// Load credentials in the same format as the GUI
fn load_credentials(config_manager: &ConfigManager) -> Result<Option<StoredCredentials>> {
    let credentials_path = get_credentials_file_path(config_manager)?;
    
    if !credentials_path.exists() {
        return Ok(None);
    }
    
    let credentials_content = fs::read_to_string(&credentials_path)?;
    let credentials: StoredCredentials = serde_json::from_str(&credentials_content)?;
    
    Ok(Some(credentials))
}

/// Clear stored credentials
async fn clear_stored_credentials(config_manager: &ConfigManager) -> Result<()> {
    let credentials_path = get_credentials_file_path(config_manager)?;
    
    if credentials_path.exists() {
        fs::remove_file(&credentials_path)?;
    }
    
    // Also clear the old config format
    config_manager.clear_auth()?;
    
    Ok(())
}

/// Get credentials file path (same location as GUI)
fn get_credentials_file_path(_config_manager: &ConfigManager) -> Result<PathBuf> {
    let app_data_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
        .join("mothership");
    
    Ok(app_data_dir.join("credentials.json"))
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
    }
}

// Legacy functions no longer needed - using `open` crate and `hostname` crate instead 