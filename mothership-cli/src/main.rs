use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use mothership_common::ClientConfig;

mod config;
mod auth;
mod gateway;
mod beam;
mod sync;

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
    /// Beam into a project for development
    Beam {
        /// Project name or ID
        project: String,
        /// Optional rift name/path
        #[arg(short, long)]
        rift: Option<String>,
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
        Commands::Beam { project, rift } => {
            // Validate authentication before beam operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("🚀 Beaming into {}...", project).cyan().bold());
            beam::handle_beam(&config_manager, project, rift, false).await?;
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
    
    if let Some(token) = &config.auth_token {
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