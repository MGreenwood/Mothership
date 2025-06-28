use anyhow::{anyhow, Result};
use colored::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;

use crate::{config::ConfigManager, print_api_error, print_info, print_success};

/// Server connection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConnection {
    pub name: String,
    pub url: String,
    pub auth_token: Option<String>,
    pub auth_method: String,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub capabilities: Option<ServerCapabilities>,
}

/// Server capabilities response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub auth_methods: Vec<String>,
    pub sso_domain: Option<String>,
    pub oauth_providers: Vec<String>,
    pub features: Vec<String>,
    pub name: String,
    pub version: String,
}

/// Connections configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionsConfig {
    pub active_server: Option<String>,
    pub servers: HashMap<String, ServerConnection>,
}

impl Default for ConnectionsConfig {
    fn default() -> Self {
        Self {
            active_server: None,
            servers: HashMap::new(),
        }
    }
}

/// Get the connections config file path
fn get_connections_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("Could not find config directory"))?
        .join("mothership");
    
    // Ensure config directory exists
    fs::create_dir_all(&config_dir)?;
    
    Ok(config_dir.join("connections.json"))
}

/// Load connections configuration
pub fn load_connections_config() -> Result<ConnectionsConfig> {
    let config_path = get_connections_config_path()?;
    
    if !config_path.exists() {
        return Ok(ConnectionsConfig::default());
    }
    
    let config_content = fs::read_to_string(&config_path)?;
    let config: ConnectionsConfig = serde_json::from_str(&config_content)?;
    
    Ok(config)
}

/// Save connections configuration
pub fn save_connections_config(config: &ConnectionsConfig) -> Result<()> {
    let config_path = get_connections_config_path()?;
    let config_json = serde_json::to_string_pretty(config)?;
    fs::write(&config_path, config_json)?;
    Ok(())
}

/// Get the currently active server connection
pub fn get_active_server() -> Result<Option<ServerConnection>> {
    let config = load_connections_config()?;
    
    if let Some(active_url) = config.active_server {
        if let Some(server) = config.servers.get(&active_url) {
            return Ok(Some(server.clone()));
        }
    }
    
    Ok(None)
}

/// Standard Mothership port (try first)
const MOTHERSHIP_DEFAULT_PORT: u16 = 7523;

/// Fallback ports to try if default doesn't work
const MOTHERSHIP_FALLBACK_PORTS: &[u16] = &[443, 80, 8080, 3000];

/// Smart server discovery with automatic port detection
async fn discover_server_with_ports(server_input: &str) -> Result<(String, ServerCapabilities)> {
    // If input already has a specific port, try it with both protocols first
    if server_input.contains(":") && !server_input.contains("://") {
        // Extract host and port from "hostname:port"
        let parts: Vec<&str> = server_input.split(':').collect();
        if parts.len() == 2 {
            let host = parts[0];
            let port = parts[1];
            
            // Try HTTPS first, then HTTP with the specific port
            for protocol in &["https", "http"] {
                let url = format!("{}://{}:{}", protocol, host, port);
                print_info(&format!("Trying {}...", url));
                
                if let Ok(capabilities) = discover_server_capabilities(&url).await {
                    print_success(&format!("Found Mothership server at {}!", url));
                    return Ok((url, capabilities));
                }
            }
        }
    }
    
    // If input already has protocol and port, try it directly
    if server_input.contains("://") {
        print_info(&format!("Trying {}...", server_input));
        if let Ok(capabilities) = discover_server_capabilities(server_input).await {
            print_success(&format!("Found Mothership server at {}!", server_input));
            return Ok((server_input.to_string(), capabilities));
        }
    }
    
    // Normalize the base hostname/domain
    let base_host = server_input
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(':')
        .next()
        .unwrap_or(server_input);
    
    // First, try the standard Mothership port (7523) with both HTTPS and HTTP
    for protocol in &["https", "http"] {
        let url = format!("{}://{}:{}", protocol, base_host, MOTHERSHIP_DEFAULT_PORT);
        print_info(&format!("Trying {}...", url));
        
        if let Ok(capabilities) = discover_server_capabilities(&url).await {
            print_success(&format!("Found Mothership server at {}!", url));
            return Ok((url, capabilities));
        }
    }
    
    // If standard port failed, try fallback ports
    for protocol in &["https", "http"] {
        for &port in MOTHERSHIP_FALLBACK_PORTS {
            let url = format!("{}://{}:{}", protocol, base_host, port);
            print_info(&format!("Trying {}...", url));
            
            if let Ok(capabilities) = discover_server_capabilities(&url).await {
                print_success(&format!("Found Mothership server at {}!", url));
                return Ok((url, capabilities));
            }
        }
    }
    
    // If no common ports worked, try the input as-is with HTTPS
    let fallback_url = if server_input.starts_with("http") {
        server_input.to_string()
    } else {
        format!("https://{}", server_input)
    };
    
    match discover_server_capabilities(&fallback_url).await {
        Ok(capabilities) => Ok((fallback_url, capabilities)),
        Err(_) => {
            let mut all_ports = vec![MOTHERSHIP_DEFAULT_PORT.to_string()];
            all_ports.extend(MOTHERSHIP_FALLBACK_PORTS.iter().map(|p| p.to_string()));
            Err(anyhow!("No Mothership server found at {} (tried ports: {})", 
                base_host, 
                all_ports.join(", ")
            ))
        }
    }
}

/// Discover server capabilities
async fn discover_server_capabilities(server_url: &str) -> Result<ServerCapabilities> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    
    let capabilities_url = format!("{}/capabilities", server_url.trim_end_matches('/'));
    
    match client.get(&capabilities_url).send().await {
        Ok(response) if response.status().is_success() => {
            // Server returns capabilities wrapped in ApiResponse format
            #[derive(serde::Deserialize)]
            struct ApiResponse<T> {
                success: bool,
                data: T,
            }
            
            let api_response: ApiResponse<ServerCapabilities> = response.json().await?;
            if api_response.success {
                Ok(api_response.data)
            } else {
                Err(anyhow!("Server reported failure in capabilities response"))
            }
        }
        Ok(response) => {
            Err(anyhow!("Server returned error: HTTP {}", response.status()))
        }
        Err(e) => {
            Err(anyhow!("Could not connect to server: {}", e))
        }
    }
}

/// Authenticate with server using OAuth
async fn authenticate_with_server(_server_url: &str, capabilities: &ServerCapabilities) -> Result<String> {
    // For now, implement a simple flow - in real implementation this would 
    // handle various auth methods based on server capabilities
    
    if capabilities.oauth_providers.contains(&"google".to_string()) {
        print_info("🔐 Authenticating with Google OAuth...");
        // TODO: Implement OAuth flow
        // For now, return a placeholder token
        Ok("placeholder_oauth_token".to_string())
    } else if capabilities.sso_domain.is_some() {
        print_info("🔐 Authenticating with company SSO...");
        // TODO: Implement SSO flow
        Ok("placeholder_sso_token".to_string())
    } else {
        Err(anyhow!("No supported authentication methods available on this server"))
    }
}

/// Handle connect to server command
pub async fn handle_connect(_config_manager: &ConfigManager, server_url: String) -> Result<()> {
    print_info(&format!("Discovering Mothership server at {}...", server_url));
    
    // Try to discover server with smart port detection
    let (final_url, capabilities) = match discover_server_with_ports(&server_url).await {
        Ok((url, caps)) => (url, caps),
        Err(e) => {
            print_api_error(&format!("Failed to connect to server: {}", e));
            print_info("Make sure the server URL is correct and the server is running");
            print_info("💡 Try specifying the full URL: https://your-server.com:7523");
            return Ok(());
        }
    };
    
    print_success(&format!("Connected to {} ({})", capabilities.name, capabilities.version));
    print_info(&format!("Supported authentication: {}", capabilities.auth_methods.join(", ")));
    
    // Authenticate with server
    let auth_token = match authenticate_with_server(&final_url, &capabilities).await {
        Ok(token) => token,
        Err(e) => {
            print_api_error(&format!("Authentication failed: {}", e));
            return Ok(());
        }
    };
    
    // Create server connection
    let connection = ServerConnection {
        name: capabilities.name.clone(),
        url: final_url.clone(),
        auth_token: Some(auth_token),
        auth_method: "oauth".to_string(), // TODO: Use actual method
        connected_at: chrono::Utc::now(),
        capabilities: Some(capabilities),
    };
    
    // Save connection
    let mut config = load_connections_config()?;
    config.servers.insert(final_url.clone(), connection);
    config.active_server = Some(final_url.clone());
    save_connections_config(&config)?;
    
    print_success(&format!("Successfully connected to {}!", final_url));
    print_info("All future operations will sync to this server");
    print_info("Use 'mothership server disconnect' to switch back to local-only mode");
    
    // Offer to sync existing local projects
    offer_to_sync_existing_projects().await?;
    
    Ok(())
}

/// Offer to sync existing local projects to the newly connected server
async fn offer_to_sync_existing_projects() -> Result<()> {
    // TODO: Scan for .mothership directories and offer to sync them
    // For now, just show info message
    print_info("💡 Tip: Run 'mothership deploy' in existing project directories to sync them to the server");
    Ok(())
}

/// Handle server status command
pub async fn handle_server_status(_config_manager: &ConfigManager) -> Result<()> {
    let config = load_connections_config()?;
    
    match config.active_server {
        Some(active_url) => {
            if let Some(server) = config.servers.get(&active_url) {
                print_success(&format!("Connected to: {} ({})", server.name, server.url));
                print_info(&format!("Connected since: {}", server.connected_at.format("%Y-%m-%d %H:%M:%S UTC")));
                print_info(&format!("Authentication: {}", server.auth_method));
                
                if let Some(capabilities) = &server.capabilities {
                    print_info(&format!("Server version: {}", capabilities.version));
                    print_info(&format!("Features: {}", capabilities.features.join(", ")));
                }
                
                // Test connection
                print_info("Testing connection...");
                match test_server_connection(&server.url).await {
                    Ok(()) => print_success("✅ Server is reachable"),
                    Err(e) => {
                        print_api_error(&format!("❌ Server connection failed: {}", e));
                        print_info("Use 'mothership server disconnect' if the server is no longer available");
                    }
                }
            } else {
                print_api_error("Active server configuration is corrupted");
            }
        }
        None => {
            print_info("Not connected to any server");
            print_info("Operating in local-only mode");
            print_info("Use 'mothership connect <server-url>' to connect to a server");
        }
    }
    
    Ok(())
}

/// Test connection to a server
async fn test_server_connection(server_url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    
    let health_url = format!("{}/health", server_url.trim_end_matches('/'));
    
    match client.get(&health_url).send().await {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(anyhow!("HTTP {}", response.status())),
        Err(e) => Err(anyhow!("Connection error: {}", e)),
    }
}

/// Handle server disconnect command
pub async fn handle_server_disconnect(_config_manager: &ConfigManager) -> Result<()> {
    let mut config = load_connections_config()?;
    
    match config.active_server.take() {
        Some(server_url) => {
            if let Some(server) = config.servers.get(&server_url) {
                print_success(&format!("Disconnected from {} ({})", server.name, server.url));
            } else {
                print_success("Disconnected from server");
            }
            
            save_connections_config(&config)?;
            
            print_info("Switched to local-only mode");
            print_info("All future operations will be stored locally");
            print_info("Existing projects remain available locally");
        }
        None => {
            print_info("Not currently connected to any server");
            print_info("Already operating in local-only mode");
        }
    }
    
    Ok(())
}

/// Handle server list command
pub async fn handle_server_list(_config_manager: &ConfigManager) -> Result<()> {
    let config = load_connections_config()?;
    
    if config.servers.is_empty() {
        print_info("No servers configured");
        print_info("Use 'mothership connect <server-url>' to add a server");
        return Ok(());
    }
    
    println!("\n{}", "🌐 Configured Servers".cyan().bold());
    
    for (url, server) in &config.servers {
        let is_active = config.active_server.as_ref() == Some(url);
        let status_indicator = if is_active { "🟢" } else { "⚪" };
        let server_name = if is_active { 
            server.name.green().bold() 
        } else { 
            server.name.dimmed() 
        };
        
        println!("\n{} {} ({})", status_indicator, server_name, url.dimmed());
        println!("   Connected: {}", server.connected_at.format("%Y-%m-%d %H:%M:%S"));
        println!("   Auth: {}", server.auth_method);
        
        if let Some(capabilities) = &server.capabilities {
            println!("   Version: {}", capabilities.version);
        }
        
        if is_active {
            println!("   {} Currently active", "🟢".green());
        }
    }
    
    println!("\n{}", "Use 'mothership connect <server-url>' to switch servers".dimmed());
    println!("{}", "Use 'mothership server disconnect' to switch to local-only mode".dimmed());
    
    Ok(())
}

/// Check if we're connected to a server
pub fn is_connected_to_server() -> bool {
    get_active_server().unwrap_or(None).is_some()
}

/// Get the active server URL for API calls
pub fn get_active_server_url() -> Option<String> {
    get_active_server().ok().flatten().map(|s| s.url)
}

/// Get auth token for the active server
pub fn get_active_server_token() -> Option<String> {
    get_active_server().ok().flatten().and_then(|s| s.auth_token)
} 