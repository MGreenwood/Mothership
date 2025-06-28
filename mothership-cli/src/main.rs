use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use mothership_common::ClientConfig;

mod config;
mod auth;
mod gateway;
mod beam;
mod sync;
mod connections;

use config::ConfigManager;

#[derive(Parser)]
#[command(name = "mothership")]
#[command(about = "Mothership - Frictionless Version Control")]
#[command(version = "0.1.0")]
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
    /// Deploy a new project in current directory
    Deploy {
        /// Project name (optional, defaults to directory name)
        name: Option<String>,
    },
    /// Beam into a project for development
    Beam {
        /// Project name or ID
        project: String,
        /// Optional rift name/path
        #[arg(short, long)]
        rift: Option<String>,
        /// Base directory where project folder will be created (required for non-local projects)
        #[arg(long)]
        local_dir: Option<std::path::PathBuf>,
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
    /// Disconnect from a project (stop background tracking)
    ProjectDisconnect {
        /// Project name to disconnect from (optional, defaults to current project)
        project: Option<String>,
    },
    /// Daemon management operations
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Logout (clear stored credentials)
    Logout,
}

#[derive(Subcommand)]
enum AuthMethod {
    /// Login with Google OAuth
    Google,
    /// Login with GitHub OAuth
    Github,
    /// Legacy device flow (for debugging)
    Device,
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

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();
    
    let cli = Cli::parse();
    let config_manager = ConfigManager::new()?;

    match cli.command {
        Commands::Auth { method } => {
            println!("{}", "🔐 Starting Mothership authentication...".cyan().bold());
            auth::handle_auth(&config_manager, method).await?;
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
            }
        }
        Commands::Deploy { name } => {
            // Validate authentication before deploy operations
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

            println!("{}", format!("🚀 Deploying {}...", project_name).cyan().bold());
            
            // Create the gateway/project
            if let Ok(_project) = gateway::handle_gateway_create(&config_manager, project_name.clone(), current_dir).await {
                // Automatically beam into the newly created project
                println!("\n{}", "🎯 Automatically beaming into your new project...".cyan().bold());
                if let Err(e) = beam::handle_beam(&config_manager, project_name, None, None, false).await {
                    print_api_error(&format!("Failed to beam into project: {}", e));
                    print_info("You can manually beam into your project later.");
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
            // Validate authentication before sync operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "📦 Syncing with remote Mothership...".cyan().bold());
            sync::handle_sync(&config_manager).await?;
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
        Commands::ProjectDisconnect { project } => {
            println!("{}", "🔌 Disconnecting from project...".cyan().bold());
            beam::handle_disconnect(&config_manager, project).await?;
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
    }

    Ok(())
}

/// Validate authentication by checking both local credentials and server connectivity
async fn validate_authentication(config_manager: &ConfigManager) -> Result<()> {
    // First check if we have local credentials
    if !config_manager.is_authenticated()? {
        return Err(anyhow::anyhow!("Not authenticated locally. Please run 'mothership auth' first."));
    }

    // Then validate with server
    let config = config_manager.load_config()?;
    let client = get_http_client(&config);
    
    // Try a simple auth check endpoint
    let auth_check_url = format!("{}/auth/check", config.mothership_url);
    let response = client.get(&auth_check_url).send().await;

    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                Ok(())
            } else if resp.status() == 401 {
                Err(anyhow::anyhow!("Authentication token expired or invalid. Please run 'mothership auth' again."))
            } else if resp.status() == 404 {
                Err(anyhow::anyhow!("User not found on server. Please run 'mothership auth' again."))
            } else {
                Err(anyhow::anyhow!("Authentication validation failed: HTTP {}", resp.status()))
            }
        }
        Err(_) => {
            Err(anyhow::anyhow!("Cannot connect to Mothership server at {}. Is the server running?", config.mothership_url))
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