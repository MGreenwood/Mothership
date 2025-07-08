use mothership_common::ClientConfig;
use clap::{Parser, Subcommand};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};
use colored::Colorize;

mod auth;
mod beam;
mod config;
mod connections;
mod gateway;
mod sync;
mod update;

use crate::config::ConfigManager;

#[derive(Parser)]
#[command(name = "mothership")]
#[command(about = "Mothership - Frictionless Version Control")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate this machine with Mothership
    Auth {
        /// Authentication method
        #[clap(subcommand)]
        method: Option<AuthMethod>,
    },
    /// Gateway operations (list, create projects)
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
    /// Initialize a new project in current directory
    Init {
        /// Project name (optional, defaults to directory name)
        name: Option<String>,
    },
    /// Beam into a project for development
    Beam {
        /// Project name or ID to beam into (optional if in project directory)
        #[arg(default_value = "")]
        project: String,
        
        /// Optional rift ID to join
        #[arg(long)]
        rift: Option<String>,
        
        /// Local directory to use (required for new projects)
        #[arg(long)]
        local_dir: Option<PathBuf>,
    },
    /// Status of current Mothership environment
    Status,
    /// Create a checkpoint (commit changes)
    Checkpoint {
        /// Checkpoint message
        message: String,
    },
    /// Sync with remote Mothership
    Sync,
    /// View project history and checkpoints
    History {
        /// Limit number of checkpoints to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Restore to a specific checkpoint
    Restore {
        /// Checkpoint ID to restore to
        checkpoint_id: String,
        /// Force restore without confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Delete a gateway project
    Delete {
        /// Project name to delete
        project_name: String,
        /// Force deletion without confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Connect to a Mothership server
    Connect {
        /// Server URL (e.g., https://mothership.company.com)
        server_url: String,
    },
    /// Disconnect from the current Mothership server (switch to local-only mode)
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },
    /// Daemon management operations
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Logout (clear stored credentials)
    Logout,
    /// Update the CLI to the latest version
    Update {
        #[command(flatten)]
        args: update::UpdateArgs,
    },
    /// Rift management operations
    Rift {
        #[command(subcommand)]
        action: RiftAction,
    },
}

#[derive(Subcommand)]
pub enum AuthMethod {
    /// Login with Google OAuth
    Google,
    /// Login with GitHub OAuth
    Github,
}

#[derive(Subcommand)]
enum GatewayAction {
    /// List available projects (default)
    List {
        /// Include inactive projects
        #[arg(long)]
        include_inactive: bool,
    },
    /// Create a new gateway project
    Create {
        /// Gateway name
        name: String,
        /// Project directory path
        #[arg(short, long)]
        dir: std::path::PathBuf,
    },
    /// Disconnect from a project (stop background tracking)
    Disconnect {
        /// Project name to disconnect from (optional, defaults to current project)
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum ServerAction {
    /// Show current server connection status
    Status,
    /// Disconnect from current server (switch to local-only mode)
    Disconnect,
    /// List all configured servers
    List,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Show daemon status and tracked projects
    Status,
    /// Stop the background daemon
    Stop,
    /// Restart the background daemon
    Restart,
}

#[derive(Subcommand)]
enum RiftAction {
    /// List all rifts in the current project
    List {
        /// Show detailed information about each rift
        #[arg(short, long)]
        detailed: bool,
    },
    /// Create a new rift
    New {
        /// Name of the new rift
        name: String,
        
        /// Description of the rift's purpose
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Switch to a different rift
    Switch {
        /// Name or ID of the rift to switch to
        name: String,
    },
    /// Show current rift status
    Status,
    /// Compare rifts (flexible arguments)
    Diff {
        /// First rift to compare (optional)
        from: Option<String>,
        
        /// Second rift to compare (optional)
        to: Option<String>,
    },
}

// Local types
#[derive(Debug, Serialize, Deserialize)]
struct RiftDiff {
    path: PathBuf,
    change_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct RiftInfo {
    id: Uuid,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    author: String,
    file_count: usize,
    is_conflict_rift: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();
    
    // Check for verbose help before parsing with clap
    let args: Vec<String> = std::env::args().collect();
    if check_verbose_help(&args) {
        print_verbose_help();
        return Ok(());
    }
    
    let cli = Cli::parse();
    let config_manager = ConfigManager::new()?;

    match cli.command {
        Commands::Auth { method: _ } => {
            println!("{}", "🔐 Starting Mothership authentication...".cyan().bold());
            auth::authenticate(&config_manager).await?;
        }
        Commands::Gateway { action } => {
            // Validate authentication before gateway operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            match action {
                GatewayAction::List { include_inactive } => {
                    println!("{}", "🌌 Accessing your development gateway...".cyan().bold());
                    gateway::handle_gateway(&config_manager, include_inactive).await?;
                }
                GatewayAction::Create { name, dir } => {
                    println!("{}", format!("🏗️  Creating new gateway: {}...", name).cyan().bold());
                    gateway::handle_gateway_create(&config_manager, name, dir).await?;
                }
                GatewayAction::Disconnect { project } => {
                    println!("{}", "🔌 Disconnecting from project...".cyan().bold());
                    beam::handle_disconnect(&config_manager, project).await?;
                }
            }
        }
        Commands::Init { name } => {
            // Validate authentication before init operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            let current_dir = std::env::current_dir()?;
            let project_name = name.unwrap_or_else(|| {
                current_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("mothership-project")
                    .to_string()
            });

            println!("{}", format!("🚀 Initializing {}...", project_name).cyan().bold());
            
            // Create the gateway/project (CRITICAL FIX: Properly handle errors)
            match gateway::handle_gateway_create(&config_manager, project_name.clone(), current_dir).await {
                Ok(_project) => {
                    // Automatically beam into the newly created project
                    println!("\n{}", "🎯 Automatically beaming into your new project...".cyan().bold());
                    if let Err(e) = beam::handle_beam(&config_manager, project_name, None, None, false).await {
                        print_api_error(&format!("Failed to beam into project: {}", e));
                        print_info("You can manually beam into your project later.");
                    }
                }
                Err(e) => {
                    print_api_error(&format!("Failed to initialize project: {}", e));
                    return Err(e);
                }
            }
        }
        Commands::Beam { project, rift, local_dir } => {
            // Validate authentication before beam operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("🚀 Beaming into {}...", project).cyan().bold());
            beam::handle_beam(&config_manager, project, rift, local_dir, false).await?;
        }
        Commands::Status => {
            // Validate authentication before status operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "📊 Checking sync status...".cyan().bold());
            sync::handle_status(&config_manager).await?;
        }
        Commands::Checkpoint { message } => {
            // Validate authentication before checkpoint operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "📸 Creating checkpoint...".cyan().bold());
            sync::handle_checkpoint(&config_manager, Some(message)).await?;
        }
        Commands::Sync => {
            println!("{}", "📦 Syncing with remote Mothership...".cyan().bold());
            handle_sync_internal().await?;
        }
        Commands::History { limit } => {
            // Validate authentication before history operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "📜 Loading project history...".cyan().bold());
            sync::handle_history(&config_manager, limit).await?;
        }
        Commands::Restore { checkpoint_id, force } => {
            // Validate authentication before restore operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("🔄 Restoring to checkpoint {}...", checkpoint_id).cyan().bold());
            sync::handle_restore(&config_manager, checkpoint_id, force).await?;
        }
        Commands::Delete { project_name, force } => {
            // Validate authentication before delete operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("🗑️  Deleting project {}...", project_name).cyan().bold());
            gateway::handle_delete(&config_manager, project_name, force).await?;
        }
        Commands::Connect { server_url } => {
            println!("{}", format!("🔗 Connecting to {}...", server_url).cyan().bold());
            connections::handle_connect(&config_manager, server_url).await?;
        }
        Commands::Server { action } => {
            match action {
                ServerAction::Status => {
                    println!("{}", "📡 Checking server connection status...".cyan().bold());
                    connections::handle_server_status(&config_manager).await?;
                }
                ServerAction::Disconnect => {
                    println!("{}", "🔌 Disconnecting from server...".cyan().bold());
                    connections::handle_server_disconnect(&config_manager).await?;
                }
                ServerAction::List => {
                    println!("{}", "📋 Listing configured servers...".cyan().bold());
                    connections::handle_server_list(&config_manager).await?;
                }
            }
        }
        Commands::Daemon { action } => {
            match action {
                DaemonAction::Status => {
                    println!("{}", "🤖 Checking daemon status...".cyan().bold());
                    beam::handle_daemon_status().await?;
                }
                DaemonAction::Stop => {
                    println!("{}", "⏹️  Stopping daemon...".cyan().bold());
                    beam::handle_daemon_stop().await?;
                }
                DaemonAction::Restart => {
                    println!("{}", "🔄 Restarting daemon...".cyan().bold());
                    beam::handle_daemon_restart().await?;
                }
            }
        }
        Commands::Logout => {
            println!("{}", "🔓 Logging out...".cyan().bold());
            auth::handle_logout(&config_manager).await?;
        }
        Commands::Update { args } => {
            update::handle_update(args).await?;
        }
        Commands::Rift { action } => {
            match action {
                RiftAction::List { detailed } => {
                    handle_rifts_command(detailed).await?;
                }
                RiftAction::New { name, description } => {
                    handle_create_rift_command(name, description).await?;
                }
                RiftAction::Switch { name } => {
                    handle_switch_rift_command(name).await?;
                }
                RiftAction::Status => {
                    handle_rift_status_command().await?;
                }
                RiftAction::Diff { from, to } => {
                    handle_rift_diff_command(from, to).await?;
                }
            }
        }
    }

    Ok(())
}

/// Validate authentication by checking both local credentials and server connectivity
async fn validate_authentication(config_manager: &ConfigManager) -> Result<()> {
    // First check if we have local credentials
    if !config_manager.is_authenticated()? {
        return Err(anyhow!("Not authenticated locally. Please run 'mothership auth' first."));
    }

    // Get server URL (prioritize active connection over config)
    let server_url = if let Some(url) = connections::get_active_server_url() {
        url
    } else {
        let config = config_manager.load_config()?;
        config.mothership_url
    };

    // Then validate with server
    let config = config_manager.load_config()?;
    let client = get_http_client(&config);
    
    // Try a simple auth check endpoint
    let auth_check_url = format!("{}/auth/check", server_url);
    let response = client.get(&auth_check_url).send().await;

    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                Ok(())
            } else if resp.status() == 401 {
                Err(anyhow!("Authentication token expired or invalid. Please run 'mothership auth' again."))
            } else if resp.status() == 404 {
                Err(anyhow!("User not found on server. Please run 'mothership auth' again."))
            } else {
                Err(anyhow!("Authentication validation failed: HTTP {}", resp.status()))
            }
        }
        Err(_) => {
            Err(anyhow!("Cannot connect to Mothership server at {}. Is the server running?", server_url))
        }
    }
}

/// Helper function to get HTTP client with optional auth
fn get_http_client(config: &ClientConfig) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    
    // First try to get token from new OAuth credentials format
    let token = if let Some(oauth_token) = get_oauth_token() {
        Some(oauth_token)
    } else {
        // Fallback to old config format
        config.auth_token.clone()
    };
    
    if let Some(token) = token {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("Bearer invalid")),
        );
    }

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Helper function to get OAuth token from credentials.json
fn get_oauth_token() -> Option<String> {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredCredentials {
        access_token: String,
        user_email: Option<String>,
        user_name: Option<String>,
        stored_at: String,
    }
    
    let credentials_path = dirs::config_dir()?
        .join("mothership")
        .join("credentials.json");
        
    if !credentials_path.exists() {
        return None;
    }
    
    let credentials_content = std::fs::read_to_string(&credentials_path).ok()?;
    let credentials: StoredCredentials = serde_json::from_str(&credentials_content).ok()?;
    
    Some(credentials.access_token)
}

/// Pretty print authentication errors with helpful instructions
fn print_auth_error(error: &str) {
    eprintln!("{} {}", "🔒 Authentication Error:".red().bold(), error);
    eprintln!("{}", "");
    eprintln!("{} {}", "💡 To fix this:".yellow().bold(), "Run 'mothership auth' to authenticate");
    eprintln!("{} {}", "   ".dimmed(), "This will open your browser and guide you through the login process");
}

/// Pretty print API errors
fn print_api_error(error: &str) {
    eprintln!("{} {}", "❌ Error:".red().bold(), error);
}

/// Pretty print success messages
fn print_success(message: &str) {
    println!("{} {}", "✅".green().bold(), message);
}

/// Pretty print info messages
fn print_info(message: &str) {
    println!("{} {}", "ℹ️".blue().bold(), message);
}

async fn handle_rifts_command(detailed: bool) -> Result<()> {
    let rifts = get_rifts().await?;
    
    if rifts.is_empty() {
        println!("No rifts found in current project");
        return Ok(());
    }

    if detailed {
        println!("\nRift Details:");
        println!("{:-<50}", "");
        for rift in rifts {
            println!("Name: {}", rift.name);
            println!("ID: {}", rift.id);
            if let Some(desc) = rift.description {
                println!("Description: {}", desc);
            }
            println!("Created: {}", rift.created_at.format("%Y-%m-%d %H:%M:%S"));
            println!("Author: {}", rift.author);
            println!("Files: {}", rift.file_count);
            if rift.is_conflict_rift {
                println!("⚠️ This is a conflict resolution rift");
            }
            println!("{:-<50}", "");
        }
    } else {
        println!("\nAvailable Rifts:");
        for rift in rifts {
            let conflict_marker = if rift.is_conflict_rift { " ⚠️" } else { "" };
            println!("- {}{}", rift.name, conflict_marker);
        }
    }

    Ok(())
}

async fn handle_create_rift_command(name: String, description: Option<String>) -> Result<()> {
    // Validate rift name
    if !is_valid_rift_name(&name) {
        anyhow::bail!("Invalid rift name. Use only letters, numbers, dashes, and underscores.");
    }

    let _rift_id = create_rift(&name, description).await?;
    println!("✨ Created new rift: {}", name);
    println!("🔀 Switch to it with: mothership switch-rift \"{}\"", name);

    Ok(())
}

async fn handle_switch_rift_command(rift: String) -> Result<()> {
    let current_rift = get_current_rift().await?;
    
    // Check if we're already in this rift
    if let Some(current) = current_rift {
        if current.name == rift {
            println!("Already in rift: {}", rift);
            return Ok(());
        }
    }

    switch_to_rift(&rift).await?;
    println!("🔄 Switched to rift: {}", rift);
    
    // If this is a conflict rift, show the README
    let readme_path = format!(".mothership/rifts/{}/CONFLICT_README.md", rift);
    if let Ok(content) = tokio::fs::read_to_string(readme_path).await {
        println!("\n{}", content);
    }

    Ok(())
}

async fn handle_rift_status_command() -> Result<()> {
    let current = get_current_rift().await?;
    
    match current {
        Some(rift) => {
            println!("Current Rift: {}", rift.name);
            if let Some(desc) = rift.description {
                println!("Description: {}", desc);
            }
            println!("Created: {}", rift.created_at.format("%Y-%m-%d %H:%M:%S"));
            println!("Files: {}", rift.file_count);
            
            if rift.is_conflict_rift {
                println!("\n⚠️ This is a conflict resolution rift");
                println!("Use 'mothership rift-diff --to main' to see differences from main rift");
            }
        }
        None => println!("Not currently in any rift"),
    }

    Ok(())
}

async fn handle_rift_diff_command(from: Option<String>, to: Option<String>) -> Result<()> {
    let (from_rift, to_rift) = match (from, to) {
        // No args: current rift vs main
        (None, None) => {
            let current = get_current_rift().await?
                .ok_or_else(|| anyhow!("Not currently in any rift"))?;
            (current.name, "main".to_string())
        }
        // One arg: <to>'s main vs <to>
        (None, Some(to_name)) => {
            ("main".to_string(), to_name)
        }
        // Two args: <from> vs <to>
        (Some(from_name), Some(to_name)) => {
            (from_name, to_name)
        }
        // One arg in from position (shouldn't happen with our CLI structure, but handle it)
        (Some(from_name), None) => {
            ("main".to_string(), from_name)
        }
    };

    let diffs = get_rift_diffs(&from_rift, &to_rift).await?;
    
    if diffs.is_empty() {
        println!("No differences found between {} and {}", from_rift, to_rift);
        return Ok(());
    }

    println!("\nDifferences between {} and {}:", from_rift, to_rift);
    println!("{:-<50}", "");
    for diff in diffs {
        println!("File: {}", diff.path.display());
        println!("Changes: {} lines modified", diff.change_count);
        println!("{:-<50}", "");
    }

    Ok(())
}

// Helper functions
fn is_valid_rift_name(name: &str) -> bool {
    let valid_chars = name.chars().all(|c| {
        c.is_alphanumeric() || c == '-' || c == '_'
    });
    valid_chars && !name.is_empty() && name.len() <= 64
}

/// Get list of rifts for current project
async fn get_rifts() -> Result<Vec<RiftInfo>> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to get rifts
    let client = reqwest::Client::new();
    let url = format!("{}/api/rifts", active_server.url);
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to get rifts: {}", error_text));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<Vec<RiftInfo>> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    let rifts = api_response.data.ok_or_else(|| anyhow!("No rift data received"))?;
    Ok(rifts)
}

/// Create a new rift
async fn create_rift(name: &str, description: Option<String>) -> Result<uuid::Uuid> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to create rift
    let client = reqwest::Client::new();
    let url = format!("{}/api/rifts", active_server.url);
    
    #[derive(serde::Serialize)]
    struct CreateRiftRequest {
        name: String,
        description: Option<String>,
    }
    
    let request = CreateRiftRequest {
        name: name.to_string(),
        description,
    };
    
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&request)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to create rift: {}", error_text));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<uuid::Uuid> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    let rift_id = api_response.data.ok_or_else(|| anyhow!("No rift ID received"))?;
    Ok(rift_id)
}

/// Get current rift information
async fn get_current_rift() -> Result<Option<RiftInfo>> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to get current rift
    let client = reqwest::Client::new();
    let url = format!("{}/api/rifts/current", active_server.url);
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to get current rift: {}", error_text));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<Option<RiftInfo>> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    let current_rift = api_response.data.ok_or_else(|| anyhow!("No rift data received"))?;
    Ok(current_rift)
}

/// Switch to a different rift
async fn switch_to_rift(rift_name: &str) -> Result<()> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to switch rift
    let client = reqwest::Client::new();
    let url = format!("{}/api/rifts/switch", active_server.url);
    
    #[derive(serde::Serialize)]
    struct SwitchRiftRequest {
        rift_name: String,
    }
    
    let request = SwitchRiftRequest {
        rift_name: rift_name.to_string(),
    };
    
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&request)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to switch rift: {}", error_text));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<String> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    // Update local project metadata with new rift
    update_local_rift_metadata(rift_name)?;
    
    Ok(())
}

/// Get differences between two rifts
async fn get_rift_diffs(from_rift: &str, to_rift: &str) -> Result<Vec<RiftDiff>> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to get rift diffs
    let client = reqwest::Client::new();
    let url = format!("{}/api/rifts/diff", active_server.url);
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .query(&[("from", from_rift), ("to", to_rift)])
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Failed to get rift diffs: {}", error_text));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<Vec<RiftDiff>> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    let diffs = api_response.data.ok_or_else(|| anyhow!("No diff data received"))?;
    Ok(diffs)
}

/// Helper function to get current project metadata
fn get_current_project_metadata() -> Result<ProjectMetadata> {
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
    Ok(project_metadata)
}

/// Helper function to update local rift metadata
fn update_local_rift_metadata(rift_name: &str) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let mothership_dir = current_dir.join(".mothership");
    let project_file = mothership_dir.join("project.json");

    if !project_file.exists() {
        return Err(anyhow!("No project metadata found"));
    }

    let project_content = std::fs::read_to_string(&project_file)?;
    let mut project_metadata: ProjectMetadata = serde_json::from_str(&project_content)?;
    
    // Update the current rift in metadata
    project_metadata.current_rift = Some(rift_name.to_string());
    
    // Write back to file
    let updated_content = serde_json::to_string_pretty(&project_metadata)?;
    std::fs::write(&project_file, updated_content)?;
    
    Ok(())
}

/// Project metadata structure
#[derive(serde::Serialize, serde::Deserialize)]
struct ProjectMetadata {
    project_id: String,
    project_name: String,
    created_at: String,
    mothership_url: String,
    rift_id: Option<String>,
    current_rift: Option<String>,
}

async fn handle_daemon_status() -> Result<()> {
    let _project_metadata = get_current_project_metadata()?;
    // ... existing code ...
    Ok(())
}

async fn handle_daemon_stop() -> Result<()> {
    let _project_metadata = get_current_project_metadata()?;
    // ... existing code ...
    Ok(())
}

async fn handle_daemon_restart() -> Result<()> {
    let _project_metadata = get_current_project_metadata()?;
    // ... existing code ...
    Ok(())
}

async fn handle_sync_internal() -> Result<()> {
    let _project_metadata = get_current_project_metadata()?;
    // ... existing code ...
    Ok(())
}

/// Check if the user requested verbose help (-h -v or -hv)
fn check_verbose_help(args: &[String]) -> bool {
    // Check for -h -v (separate flags)
    if args.contains(&"-h".to_string()) && args.contains(&"-v".to_string()) {
        return true;
    }
    
    // Check for -hv or -vh (combined flags)
    if args.iter().any(|arg| arg == "-hv" || arg == "-vh") {
        return true;
    }
    
    // Check for --help --verbose or --verbose --help
    if args.contains(&"--help".to_string()) && args.contains(&"--verbose".to_string()) {
        return true;
    }
    
    false
}

/// Print verbose help with color-coded command tree
fn print_verbose_help() {
    println!("{}", "┌─────────────────────────────────────────────────────────┐".bright_cyan());
    println!("{}", "│                    🚀 MOTHERSHIP CLI                    │".bright_cyan().bold());
    println!("{}", "│              Frictionless Version Control               │".bright_cyan());
    println!("{}", "└─────────────────────────────────────────────────────────┘".bright_cyan());
    println!();
    
    println!("{}", "USAGE:".bright_yellow().bold());
    println!("    {} {}", "mothership".green().bold(), "[COMMAND] [OPTIONS]".white());
    println!();
    
    println!("{}", "DESCRIPTION:".bright_yellow().bold());
    println!("    Mothership provides frictionless version control with real-time collaboration,");
    println!("    automatic conflict resolution, and seamless project synchronization across teams.");
    println!();
    
    println!("{}", "CORE COMMANDS:".bright_yellow().bold());
    print_command_section("🔐", "auth", "Authentication & Setup", &[
        ("google", "Login with Google OAuth", None),
        ("github", "Login with GitHub OAuth", None),
    ]);
    
    print_command_section("🌌", "gateway", "Project Management", &[
        ("list", "List available projects", Some("--include-inactive")),
        ("create", "Create a new project", Some("<name> --dir <path>")),
        ("disconnect", "Stop tracking a project", Some("[project]")),
    ]);
    
    print_command_section("🚀", "beam", "Project Development", &[]);
    println!("    {} {}", "mothership beam".green().bold(), "<project> [OPTIONS]".white());
    println!("    {} {}", "   --rift".bright_blue(), "<name>                Specify rift to join".dimmed());
    println!("    {} {}", "   --local-dir".bright_blue(), "<path>           Local directory for project".dimmed());
    println!();
    
    print_command_section("📊", "status", "Project Status", &[]);
    println!("    {} {}", "mothership status".green().bold(), "                        Check sync status".dimmed());
    println!();
    
    print_command_section("📸", "checkpoint", "Version Control", &[]);
    println!("    {} {}", "mothership checkpoint".green().bold(), "<message>        Create a checkpoint".dimmed());
    println!();
    
    print_command_section("📦", "sync", "Synchronization", &[]);
    println!("    {} {}", "mothership sync".green().bold(), "                         Sync with remote".dimmed());
    println!();
    
    print_command_section("📜", "history", "Project History", &[]);
    println!("    {} {}", "mothership history".green().bold(), "[OPTIONS]            View checkpoints".dimmed());
    println!("    {} {}", "   --limit".bright_blue(), "<num>               Limit results (default: 20)".dimmed());
    println!();
    
    print_command_section("🔄", "restore", "Time Travel", &[]);
    println!("    {} {}", "mothership restore".green().bold(), "<checkpoint-id>      Restore to checkpoint".dimmed());
    println!("    {} {}", "   --force".bright_blue(), "                       Skip confirmation".dimmed());
    println!();
    
    print_command_section("🗑️", "delete", "Project Cleanup", &[]);
    println!("    {} {}", "mothership delete".green().bold(), "<project> [--force]   Delete a project".dimmed());
    println!();
    
    print_command_section("🔗", "connect", "Server Management", &[]);
    println!("    {} {}", "mothership connect".green().bold(), "<url>               Connect to server".dimmed());
    println!();
    
    print_command_section("📡", "server", "Server Operations", &[
        ("status", "Check connection status", None),
        ("disconnect", "Disconnect from server", None),
        ("list", "List configured servers", None),
    ]);
    
    print_command_section("🤖", "daemon", "Background Service", &[
        ("status", "Show daemon status", None),
        ("stop", "Stop background daemon", None),
        ("restart", "Restart background daemon", None),
    ]);
    
    print_command_section("🌊", "rift", "Collaborative Spaces", &[
        ("list", "List project rifts", Some("--detailed")),
        ("new", "Create a new rift", Some("<name> --description <desc>")),
        ("switch", "Switch to a rift", Some("<name>")),
        ("status", "Show current rift", None),
        ("diff", "Compare rifts", Some("[from] [to]")),
    ]);
    
    print_command_section("🚀", "init", "Quick Init", &[]);
    println!("    {} {}", "mothership init".green().bold(), "[name]                Initialize current directory".dimmed());
    println!();
    
    print_command_section("🔓", "logout", "Session Management", &[]);
    println!("    {} {}", "mothership logout".green().bold(), "                       Clear credentials".dimmed());
    println!();
    
    print_command_section("⬆️", "update", "CLI Maintenance", &[]);
    println!("    {} {}", "mothership update".green().bold(), "[OPTIONS]             Update CLI version".dimmed());
    println!();
    
    println!("{}", "GLOBAL OPTIONS:".bright_yellow().bold());
    println!("    {} {}", "-h, --help".bright_blue().bold(), "        Show help information".dimmed());
    println!("    {} {}", "-h -v, -hv".bright_blue().bold(), "      Show this verbose help tree".dimmed());
    println!("    {} {}", "-V, --version".bright_blue().bold(), "     Show version information".dimmed());
    println!();
    
    println!("{}", "EXAMPLES:".bright_yellow().bold());
    println!("    {} {}", "mothership auth google".green(), "                    # Authenticate with Google".dimmed());
    println!("    {} {}", "mothership connect https://my-server.com".green(), "  # Connect to a server".dimmed());
    println!("    {} {}", "mothership gateway list".green(), "                   # List available projects".dimmed());
    println!("    {} {}", "mothership beam my-project".green(), "                # Join a project".dimmed());
    println!("    {} {}", "mothership rift new feature-branch".green(), "        # Create a new rift".dimmed());
    println!("    {} {}", "mothership checkpoint \"Added new feature\"".green(), "   # Save progress".dimmed());
    println!("    {} {}", "mothership daemon status".green(), "                  # Check background sync".dimmed());
    println!();
    
    println!("{}", "WORKFLOW:".bright_yellow().bold());
    println!("    {} {}", "1.".bright_cyan().bold(), "mothership auth google          # Authenticate");
    println!("    {} {}", "2.".bright_cyan().bold(), "mothership connect <server>     # Connect to server");
    println!("    {} {}", "3.".bright_cyan().bold(), "mothership gateway list         # Browse projects");
    println!("    {} {}", "4.".bright_cyan().bold(), "mothership beam <project>       # Join project");
    println!("    {} {}", "5.".bright_cyan().bold(), "mothership rift new <branch>    # Create workspace");
    println!("    {} {}", "6.".bright_cyan().bold(), "Edit files...                   # Work on your code");
    println!("    {} {}", "7.".bright_cyan().bold(), "mothership checkpoint <msg>     # Save progress");
    println!();
    
    println!("{}", "For more information, visit: https://mothership.dev/docs".bright_blue().underline());
}

/// Helper function to print a command section
fn print_command_section(icon: &str, command: &str, description: &str, subcommands: &[(&str, &str, Option<&str>)]) {
    println!("{} {} {} {}", icon, command.green().bold(), "-".dimmed(), description.white());
    
    if subcommands.is_empty() {
        return;
    }
    
    for (subcmd, desc, args) in subcommands {
        let full_command = format!("mothership {} {}", command, subcmd);
        let args_text = args.map(|a| format!(" {}", a)).unwrap_or_default();
        println!("    {} {}{}", full_command.green().bold(), args_text.white(), format!("  # {}", desc).dimmed());
    }
    println!();
}

/// Get machine ID for authentication
pub fn get_machine_id() -> anyhow::Result<String> {
    Ok(uuid::Uuid::new_v4().to_string())
}

/// Get machine name for authentication
pub fn get_machine_name() -> anyhow::Result<String> {
    let hostname = hostname::get()?;
    Ok(format!("{}-mothership-cli", hostname.to_string_lossy()))
} 