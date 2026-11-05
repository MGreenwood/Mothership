use mothership_common::ClientConfig;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{CompleteEnv, engine::ArgValueCandidates};
use clap_complete::env::{Bash, Elvish, EnvCompleter, Fish, Shells, Zsh};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde::{Serialize, Deserialize};
use colored::Colorize;

mod auth;
mod beam;
mod completion;
mod completion_shell;
mod config;
mod connections;
mod gateway;
#[cfg(test)]
mod profiling;
mod project_index;
mod sync;
mod update;
mod wormhole;

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
    /// Gateway operations (list, release)
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
    /// Initialize a new project in current directory
    Init {
        /// Project name (optional, defaults to directory name)
        name: Option<String>,
        /// Create a private gateway (only visible to its members)
        #[arg(short = 'p', long)]
        private: bool,
    },
    /// Beam into a project for development
    Beam {
        /// Project name or ID to beam into (optional if in project directory)
        #[arg(default_value = "", add = ArgValueCandidates::new(completion::beam_project_candidates))]
        project: String,
        
        /// Local directory to use (required for new projects)
        #[arg(short = 'l', long = "local-dir")]
        local_dir: Option<PathBuf>,
    },
    /// Status of current Mothership environment
    Status,
    /// Create a checkpoint, or manage existing ones
    Checkpoint {
        #[command(subcommand)]
        action: Option<CheckpointAction>,
        /// Checkpoint message (defaults to "<rift> <date time>")
        #[arg(short = 'm', long = "message")]
        name: Option<String>,
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
    /// Wormhole operations (live merge previews across rifts)
    Wormhole {
        #[command(subcommand)]
        action: wormhole::WormholeAction,
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
    /// Release a gateway (stop background tracking)
    Release {
        /// Gateway name to release (optional, defaults to current gateway)
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

        /// Make the rift private (only members can see or switch to it)
        #[arg(short = 'p', long)]
        private: bool,
    },
    /// Switch to a different rift
    Switch {
        /// Name or ID of the rift to switch to
        name: String,
    },
    /// Add or remove a member of the current rift (owner only)
    Users {
        /// Add the user to the rift
        #[arg(short = 'a', long, conflicts_with = "remove")]
        add: bool,
        /// Remove the user from the rift
        #[arg(short = 'r', long, conflicts_with = "add")]
        remove: bool,
        /// Username to add or remove
        username: String,
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
    /// List and delete local backup copies saved before overwrites
    Purge,
    /// Send and read chat messages in a rift
    Chat {
        #[command(subcommand)]
        action: ChatAction,
    },
}

#[derive(Subcommand)]
enum CheckpointAction {
    /// Delete a checkpoint from your rift
    Delete {
        /// Checkpoint ID to delete
        checkpoint_id: String,
        /// Force deletion without confirmation
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ChatAction {
    /// Send a message to everyone in a rift
    Send {
        /// Rift name or ID
        #[arg(short, long)]
        rift: String,

        /// Message text
        #[arg(required = true)]
        message: Vec<String>,
    },
    /// Show recent chat messages in a rift
    History {
        /// Rift name or ID
        #[arg(short, long)]
        rift: String,

        /// Number of messages to show
        #[arg(short, long, default_value_t = 20)]
        limit: i64,
    },
    /// Open an interactive chat prompt for a rift (Ctrl+Z then Enter to leave)
    Enter {
        /// Rift name or ID (defaults to the rift you're currently in)
        #[arg(short, long)]
        rift: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    id: Uuid,
    rift_id: Uuid,
    author_id: Uuid,
    author_username: String,
    content: String,
    timestamp: DateTime<Utc>,
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
    #[serde(default)]
    project_id: Option<Uuid>,
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
    // Answers shell completion requests and exits; otherwise a no-op
    let shells: &[&dyn EnvCompleter] = &[
        &Bash,
        &Elvish,
        &Fish,
        &completion_shell::PowerShellCompleter,
        &Zsh,
    ];
    CompleteEnv::with_factory(Cli::command)
        .shells(Shells(shells))
        .complete();

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
            println!("{}", "Starting Mothership authentication...".cyan().bold());
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
                    println!("{}", "Accessing your development gateway...".cyan().bold());
                    gateway::handle_gateway(&config_manager, include_inactive).await?;
                }
                GatewayAction::Release { project } => {
                    println!("{}", "Releasing gateway...".cyan().bold());
                    beam::handle_disconnect(&config_manager, project).await?;
                }
            }
        }
        Commands::Init { name, private } => {
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

            println!("{}", format!("Initializing {}...", project_name).cyan().bold());
            
            // Create the gateway/project (CRITICAL FIX: Properly handle errors)
            match gateway::handle_gateway_create(&config_manager, project_name.clone(), current_dir.clone(), private).await {
                Ok(_project) => {
                    // Automatically beam into the newly created project
                    println!("\n{}", "Automatically beaming into your new project...".cyan().bold());
                    if let Err(e) = beam::handle_beam(&config_manager, project_name, Some(current_dir), false).await {
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
        Commands::Beam { project, local_dir } => {
            // Validate authentication before beam operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("Beaming into {}...", project).cyan().bold());
            beam::handle_beam(&config_manager, project, local_dir, false).await?;
        }
        Commands::Status => {
            // Validate authentication before status operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "Checking sync status...".cyan().bold());
            sync::handle_status(&config_manager).await?;
        }
        Commands::Checkpoint { action, name } => {
            // Validate authentication before checkpoint operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            match action {
                Some(CheckpointAction::Delete { checkpoint_id, force }) => {
                    sync::handle_delete_checkpoint(&config_manager, checkpoint_id, force).await?;
                }
                None => {
                    let checkpoint_name = match name {
                        Some(name) => name,
                        None => default_checkpoint_name().await,
                    };
                    println!("{}", "Creating checkpoint...".cyan().bold());
                    sync::handle_checkpoint(&config_manager, Some(checkpoint_name)).await?;
                }
            }
        }
        Commands::Sync => {
            println!("{}", "Syncing with remote Mothership...".cyan().bold());
            handle_sync_internal().await?;
        }
        Commands::History { limit } => {
            // Validate authentication before history operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", "Loading project history...".cyan().bold());
            sync::handle_history(&config_manager, limit).await?;
        }
        Commands::Restore { checkpoint_id, force } => {
            // Validate authentication before restore operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!("Restoring to checkpoint {}...", checkpoint_id).cyan().bold());
            sync::handle_restore(&config_manager, checkpoint_id, force).await?;
        }
        Commands::Delete { project_name, force } => {
            // Validate authentication before delete operations
            if let Err(e) = validate_authentication(&config_manager).await {
                print_auth_error(&e.to_string());
                return Ok(());
            }

            println!("{}", format!(" Deleting project {}...", project_name).cyan().bold());
            gateway::handle_delete(&config_manager, project_name, force).await?;
        }
        Commands::Connect { server_url } => {
            println!("{}", format!("Connecting to {}...", server_url).cyan().bold());
            connections::handle_connect(&config_manager, server_url).await?;
        }
        Commands::Server { action } => {
            match action {
                ServerAction::Status => {
                    println!("{}", "Checking server connection status...".cyan().bold());
                    connections::handle_server_status(&config_manager).await?;
                }
                ServerAction::Disconnect => {
                    println!("{}", "Disconnecting from server...".cyan().bold());
                    connections::handle_server_disconnect(&config_manager).await?;
                }
                ServerAction::List => {
                    println!("{}", "Listing configured servers...".cyan().bold());
                    connections::handle_server_list(&config_manager).await?;
                }
            }
        }
        Commands::Daemon { action } => {
            match action {
                DaemonAction::Status => {
                    println!("{}", "Checking daemon status...".cyan().bold());
                    beam::handle_daemon_status().await?;
                }
                DaemonAction::Stop => {
                    println!("{}", " Stopping daemon...".cyan().bold());
                    beam::handle_daemon_stop().await?;
                }
                DaemonAction::Restart => {
                    println!("{}", "Restarting daemon...".cyan().bold());
                    beam::handle_daemon_restart().await?;
                }
            }
        }
        Commands::Logout => {
            println!("{}", "Logging out...".cyan().bold());
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
                RiftAction::New { name, description, private } => {
                    handle_create_rift_command(name, description, private).await?;
                }
                RiftAction::Switch { name } => {
                    handle_switch_rift_command(name).await?;
                }
                RiftAction::Users { add, remove, username } => {
                    handle_rift_users_command(add, remove, username).await?;
                }
                RiftAction::Status => {
                    handle_rift_status_command().await?;
                }
                RiftAction::Diff { from, to } => {
                    handle_rift_diff_command(from, to).await?;
                }
                RiftAction::Purge => {
                    handle_rift_purge_command()?;
                }
                RiftAction::Chat { action } => match action {
                    ChatAction::Send { rift, message } => {
                        handle_chat_send_command(&rift, &message.join(" ")).await?;
                    }
                    ChatAction::History { rift, limit } => {
                        handle_chat_history_command(&rift, limit).await?;
                    }
                    ChatAction::Enter { rift } => {
                        handle_chat_enter_command(rift.as_deref()).await?;
                    }
                },
            }
        }
        Commands::Wormhole { action } => {
            wormhole::run(action).await?;
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

static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Process-wide shared HTTP client for requests that set headers per call.
pub(crate) fn http_client() -> &'static reqwest::Client {
    HTTP.get_or_init(reqwest::Client::new)
}

/// Helper function to get HTTP client with optional auth, caching the built client
/// when the configuration (auth token) is unchanged.
fn get_http_client(config: &ClientConfig) -> reqwest::Client {
    static CACHED_CLIENT: std::sync::OnceLock<Mutex<Option<(Option<String>, reqwest::Client)>>> =
        std::sync::OnceLock::new();

    // First try to get token from new OAuth credentials format
    let token = if let Some(oauth_token) = get_oauth_token() {
        Some(oauth_token)
    } else {
        // Fallback to old config format
        config.auth_token.clone()
    };

    let cache = CACHED_CLIENT.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((cached_token, cached_client)) = guard.as_ref() {
            if *cached_token == token {
                return cached_client.clone();
            }
        }
    }

    let mut headers = reqwest::header::HeaderMap::new();
    
    if let Some(ref token) = token {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("Bearer invalid")),
        );
    }

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_else(|_| http_client().clone());

    if let Ok(mut guard) = cache.lock() {
        *guard = Some((token, client.clone()));
    }

    client
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
    eprintln!("{} {}", "Authentication Error:".red().bold(), error);
    eprintln!("{}", "");
    eprintln!("{} {}", "To fix this:".yellow().bold(), "Run 'mothership auth' to authenticate");
    eprintln!("{} {}", "   ".dimmed(), "This will open your browser and guide you through the login process");
}

/// Pretty print API errors
fn print_api_error(error: &str) {
    eprintln!("{} {}", "Error:".red().bold(), error);
}

/// Pretty print success messages
fn print_success(message: &str) {
    println!("{} {}", "".green().bold(), message);
}

/// Pretty print info messages
fn print_info(message: &str) {
    println!("{} {}", "".blue().bold(), message);
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
                println!("This is a conflict resolution rift");
            }
            println!("{:-<50}", "");
        }
    } else {
        println!("\nAvailable Rifts:");
        for rift in rifts {
            let conflict_marker = if rift.is_conflict_rift { "" } else { "" };
            let short_id = &rift.id.to_string()[..8];
            println!("- {} ({}){}", rift.name, short_id, conflict_marker);
        }
    }

    Ok(())
}

async fn handle_create_rift_command(name: String, description: Option<String>, private: bool) -> Result<()> {
    // Validate rift name
    if !is_valid_rift_name(&name) {
        anyhow::bail!("Invalid rift name. Use only letters, numbers, dashes, and underscores.");
    }

    let _rift_id = create_rift(&name, description, private).await?;
    println!("Created new {}rift: {}", if private { "private " } else { "" }, name);
    println!("Switch to it with: mothership rift switch \"{}\"", name);

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

    if !switch_to_rift(&rift).await? {
        return Ok(());
    }
    println!("Switched to rift: {}", rift);
    
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
                println!("\nThis is a conflict resolution rift");
                println!("Use 'mothership rift-diff --to main' to see differences from main rift");
            }

            if let Some(requests) = fetch_rift_access_requests(&rift.name).await? {
                if requests.is_empty() {
                    println!("\nNo pending access requests");
                } else {
                    println!("\nPending access requests:");
                    for request in requests {
                        println!(
                            "  {} (requested {})",
                            request.username,
                            request.requested_at.format("%Y-%m-%d %H:%M:%S")
                        );
                    }
                }
            }
        }
        None => println!("Not currently in any rift"),
    }

    Ok(())
}

async fn handle_rift_users_command(add: bool, remove: bool, username: String) -> Result<()> {
    if add == remove {
        anyhow::bail!("Specify exactly one of -a/--add or -r/--remove");
    }

    let rift = get_current_rift()
        .await?
        .ok_or_else(|| anyhow!("Not currently in any rift"))?;
    let project_id = local_project_id().ok_or_else(|| anyhow!("Not in a project directory"))?;
    let (server_url, auth_token) = active_server_and_token()?;

    #[derive(serde::Serialize)]
    struct RiftMemberRequest<'a> {
        rift_name: &'a str,
        project_id: Uuid,
        username: &'a str,
        remove: bool,
    }

    let response = http_client()
        .post(format!("{}/api/rifts/members", server_url))
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&RiftMemberRequest {
            rift_name: &rift.name,
            project_id,
            username: &username,
            remove,
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to update rift members: {}", response_error_message(response).await));
    }

    let api_response: ApiResponse<String> = response.json().await?;
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    println!("{}", api_response.data.unwrap_or_else(|| "Rift members updated".to_string()));

    Ok(())
}

#[derive(serde::Deserialize)]
struct RiftAccessRequestEntry {
    username: String,
    requested_at: chrono::DateTime<chrono::Utc>,
}

/// Pending access requests for a rift. Returns `None` when the caller is not the rift owner.
async fn fetch_rift_access_requests(rift_name: &str) -> Result<Option<Vec<RiftAccessRequestEntry>>> {
    let Some(project_id) = local_project_id() else {
        return Ok(None);
    };
    let (server_url, auth_token) = active_server_and_token()?;
    let project_id = project_id.to_string();

    let response = http_client()
        .get(format!("{}/api/rifts/access-requests", server_url))
        .query(&[("rift_name", rift_name), ("project_id", project_id.as_str())])
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(anyhow!("Failed to list access requests: {}", response_error_message(response).await));
    }

    let api_response: ApiResponse<Vec<RiftAccessRequestEntry>> = response.json().await?;
    Ok(api_response.data)
}

/// Asks a yes/no question on stdin. Anything other than "y" or "yes" counts as no.
fn prompt_yes_no(question: &str) -> Result<bool> {
    use std::io::{BufRead, Write};
    print!("{} [y/N] ", question);
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Server URL and auth token for authenticated API calls
fn active_server_and_token() -> Result<(String, String)> {
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    Ok((active_server.url, auth_token))
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

/// Resolve a rift name or UUID to its ID, scoped to the current project when in one
async fn resolve_rift_id(name_or_id: &str) -> Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(name_or_id) {
        return Ok(id);
    }

    let project_id = local_project_id();
    let matches: Vec<RiftInfo> = get_rifts()
        .await?
        .into_iter()
        .filter(|r| r.name == name_or_id)
        .filter(|r| in_project(r, project_id))
        .collect();

    match matches.as_slice() {
        [] => Err(anyhow!("No rift named '{}' found", name_or_id)),
        [rift] => Ok(rift.id),
        _ => Err(anyhow!("Multiple rifts named '{}'; use the rift ID instead", name_or_id)),
    }
}

/// Build an authenticated request target for the active server
fn chat_api_context() -> Result<(String, String)> {
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    Ok((active_server.url, auth_token))
}

async fn handle_chat_send_command(rift: &str, content: &str) -> Result<()> {
    let rift_id = resolve_rift_id(rift).await?;
    let (server_url, auth_token) = chat_api_context()?;

    let response = http_client()
        .post(format!("{}/api/rifts/{}/chat", server_url, rift_id))
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&serde_json::json!({ "content": content }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to send message: HTTP {}", response.status()));
    }

    let api_response: ApiResponse<ChatMessage> = response.json().await?;
    if !api_response.success {
        return Err(anyhow!("Server error: {}", api_response.error.unwrap_or_else(|| "Unknown error".to_string())));
    }

    println!("Sent to rift {}", rift);
    Ok(())
}

async fn handle_chat_history_command(rift: &str, limit: i64) -> Result<()> {
    let rift_id = resolve_rift_id(rift).await?;
    let (server_url, auth_token) = chat_api_context()?;

    let response = http_client()
        .get(format!("{}/api/rifts/{}/chat", server_url, rift_id))
        .query(&[("limit", limit)])
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to load chat history: HTTP {}", response.status()));
    }

    let api_response: ApiResponse<Vec<ChatMessage>> = response.json().await?;
    if !api_response.success {
        return Err(anyhow!("Server error: {}", api_response.error.unwrap_or_else(|| "Unknown error".to_string())));
    }

    let messages = api_response.data.unwrap_or_default();
    if messages.is_empty() {
        println!("No chat messages in {}", rift);
        return Ok(());
    }

    for message in messages {
        println!(
            "[{}] {}: {}",
            message.timestamp.format("%Y-%m-%d %H:%M:%S"),
            message.author_username,
            message.content
        );
    }
    Ok(())
}

async fn fetch_chat_messages(server_url: &str, auth_token: &str, rift_id: Uuid, limit: i64) -> Result<Vec<ChatMessage>> {
    let response = http_client()
        .get(format!("{}/api/rifts/{}/chat", server_url, rift_id))
        .query(&[("limit", limit)])
        .header("Authorization", format!("Bearer {}", auth_token))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("HTTP {}", response.status()));
    }

    let api_response: ApiResponse<Vec<ChatMessage>> = response.json().await?;
    if !api_response.success {
        return Err(anyhow!("Server error: {}", api_response.error.unwrap_or_else(|| "Unknown error".to_string())));
    }
    Ok(api_response.data.unwrap_or_default())
}

async fn post_chat_message(server_url: &str, auth_token: &str, rift_id: Uuid, content: &str) -> Result<ChatMessage> {
    let response = http_client()
        .post(format!("{}/api/rifts/{}/chat", server_url, rift_id))
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&serde_json::json!({ "content": content }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("HTTP {}", response.status()));
    }

    let api_response: ApiResponse<ChatMessage> = response.json().await?;
    if !api_response.success {
        return Err(anyhow!("Server error: {}", api_response.error.unwrap_or_else(|| "Unknown error".to_string())));
    }
    api_response.data.ok_or_else(|| anyhow!("Server returned no message"))
}

fn print_chat_line(message: &ChatMessage) {
    println!(
        "[{}] {}: {}",
        message.timestamp.format("%H:%M:%S"),
        message.author_username,
        message.content
    );
}

/// Interactive chat prompt: shows recent history, polls for new messages, and sends each line entered
async fn handle_chat_enter_command(rift: Option<&str>) -> Result<()> {
    let (rift_id, rift_label) = match rift {
        Some(name) => (resolve_rift_id(name).await?, name.to_string()),
        None => {
            let current = get_current_rift()
                .await?
                .ok_or_else(|| anyhow!("You're not in a rift. Pass one with -r <rift-name>."))?;
            (current.id, current.name)
        }
    };
    let (server_url, auth_token) = chat_api_context()?;

    let seen: Arc<Mutex<HashSet<Uuid>>> = Arc::new(Mutex::new(HashSet::new()));

    let history = fetch_chat_messages(&server_url, &auth_token, rift_id, 50).await?;
    {
        let mut seen = seen.lock().unwrap();
        for message in &history {
            seen.insert(message.id);
            print_chat_line(message);
        }
    }
    println!("Chatting in {}. Ctrl+Z then Enter to leave.", rift_label);

    let poll_seen = Arc::clone(&seen);
    let poll_url = server_url.clone();
    let poll_token = auth_token.clone();
    let poller = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let Ok(messages) = fetch_chat_messages(&poll_url, &poll_token, rift_id, 50).await else {
                continue;
            };
            let mut seen = poll_seen.lock().unwrap();
            for message in messages {
                if seen.insert(message.id) {
                    println!();
                    print_chat_line(&message);
                    print!(">>> ");
                    let _ = std::io::stdout().flush();
                }
            }
        }
    });

    loop {
        print!(">>> ");
        std::io::stdout().flush()?;

        let line = tokio::task::spawn_blocking(|| -> std::io::Result<(usize, String)> {
            let mut buffer = String::new();
            let bytes = std::io::stdin().read_line(&mut buffer)?;
            Ok((bytes, buffer))
        })
        .await??;

        let (bytes, input) = line;
        if bytes == 0 {
            break;
        }

        let content = input.trim();
        if content.is_empty() {
            continue;
        }

        match post_chat_message(&server_url, &auth_token, rift_id, content).await {
            Ok(message) => {
                seen.lock().unwrap().insert(message.id);
            }
            Err(error) => eprintln!("Failed to send: {}", error),
        }
    }

    poller.abort();
    println!();
    println!("Left chat.");
    Ok(())
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
    let client = http_client();
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
    let project_id = local_project_id();
    Ok(rifts.into_iter().filter(|r| in_project(r, project_id)).collect())
}

/// Create a new rift
async fn create_rift(name: &str, description: Option<String>, is_private: bool) -> Result<uuid::Uuid> {
    // Check if we're in a project directory
    let project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to create rift
    let client = http_client();
    let url = format!("{}/api/rifts", active_server.url);
    println!("DEBUG: Creating rift at {}", url);
    
    #[derive(serde::Serialize)]
    struct CreateRiftRequest {
        name: String,
        description: Option<String>,
        project_id: Option<uuid::Uuid>,
        fork_from: Option<uuid::Uuid>,
        is_private: bool,
    }
    
    let request = CreateRiftRequest {
        name: name.to_string(),
        description,
        project_id: local_project_id(),
        fork_from: project_metadata.rift_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
        is_private,
    };
    
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&request)
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(anyhow!("Failed to create rift: {}", response_error_message(response).await));
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
/// Project ID from the local .mothership metadata, if we're in a project directory
fn local_project_id() -> Option<Uuid> {
    get_current_project_metadata()
        .ok()
        .and_then(|metadata| Uuid::parse_str(&metadata.project_id).ok())
}

/// Whether a rift belongs to the given project (rifts without project info are kept)
fn in_project(rift: &RiftInfo, project_id: Option<Uuid>) -> bool {
    match (project_id, rift.project_id) {
        (Some(project), Some(rift_project)) => project == rift_project,
        _ => true,
    }
}

/// Default checkpoint name: "<rift> <YYYY-MM-DD HH:MM>" in local time
async fn default_checkpoint_name() -> String {
    let rift = get_current_rift()
        .await
        .ok()
        .flatten()
        .map(|r| r.name)
        .unwrap_or_else(|| "main".to_string());
    format!("{} {}", rift, chrono::Local::now().format("%Y-%m-%d %H:%M"))
}

/// The rift this project directory is currently on, from local metadata
async fn get_current_rift() -> Result<Option<RiftInfo>> {
    let metadata = get_current_project_metadata()?;
    let project_id = local_project_id();

    let rifts = get_rifts().await?;
    if let Some(rift_id_str) = &metadata.rift_id {
        if let Ok(rift_id) = Uuid::parse_str(rift_id_str) {
            if let Some(found) = rifts.iter().find(|r| r.id == rift_id) {
                return Ok(Some(RiftInfo {
                    id: found.id,
                    project_id: found.project_id,
                    name: found.name.clone(),
                    description: found.description.clone(),
                    created_at: found.created_at,
                    author: found.author.clone(),
                    file_count: found.file_count,
                    is_conflict_rift: found.is_conflict_rift,
                }));
            }
        }
    }
    if let Some(name) = &metadata.current_rift {
        return Ok(rifts
            .into_iter()
            .find(|r| &r.name == name && in_project(r, project_id)));
    }
    Ok(None)
}

/// Switch to a different rift. Returns `false` if the caller is not a member and chose not to request access.
async fn switch_to_rift(rift_name: &str) -> Result<bool> {
    // Check if we're in a project directory
    let _project_metadata = get_current_project_metadata()?;
    
    // Get active server connection
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    
    // Get auth token
    let auth_token = get_oauth_token()
        .ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    
    // Make API call to switch rift
    let client = http_client();
    let url = format!("{}/api/rifts/switch", active_server.url);
    
    #[derive(serde::Serialize)]
    struct SwitchRiftRequest {
        rift_name: String,
        project_id: Option<uuid::Uuid>,
    }
    
    let request = SwitchRiftRequest {
        rift_name: rift_name.to_string(),
        project_id: local_project_id(),
    };
    
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&request)
        .send()
        .await?;
    
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let message = response_error_message(response).await;
        println!("{}", message);
        if !prompt_yes_no(&format!("Request access to rift '{}'?", rift_name))? {
            return Ok(false);
        }
        let project_id = local_project_id().ok_or_else(|| anyhow!("Not in a project directory"))?;

        #[derive(serde::Serialize)]
        struct RiftAccessRequestBody<'a> {
            rift_name: &'a str,
            project_id: Uuid,
        }

        let response = client
            .post(format!("{}/api/rifts/access-requests", active_server.url))
            .header("Authorization", format!("Bearer {}", auth_token))
            .json(&RiftAccessRequestBody { rift_name, project_id })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(anyhow!("Failed to request access: {}", response_error_message(response).await));
        }
        let api_response: ApiResponse<String> = response.json().await?;
        println!("{}", api_response.data.unwrap_or_else(|| "Access request sent".to_string()));
        return Ok(false);
    }

    if !response.status().is_success() {
        return Err(anyhow!("Failed to switch rift: {}", response_error_message(response).await));
    }
    
    // Parse ApiResponse format
    let api_response: ApiResponse<String> = response.json().await?;
    
    if !api_response.success {
        let error_msg = api_response.error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow!("Server error: {}", error_msg));
    }
    
    // Update local project metadata with the new rift (name for display, id for sync)
    let rift_id = get_rifts()
        .await?
        .into_iter()
        .find(|r| r.name == rift_name && in_project(r, local_project_id()))
        .map(|r| r.id);
    update_local_rift_metadata(rift_name, rift_id)?;

    // Ensure the daemon is running and reconnects so local files are updated to the new rift
    if let Some(project_id) = local_project_id() {
        let _ = beam::ensure_daemon_running().await;
        if let Err(e) = mothership_common::daemon::reconnect_project(project_id).await {
            println!("Daemon reconnect warning: {e}");
        }
    }
    
    Ok(true)
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
    let client = http_client();
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

/// List every local backup file in the current project with size and modified time, then delete them all on confirmation
fn handle_rift_purge_command() -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let mothership_dir = current_dir.join(".mothership");
    if !mothership_dir.join("project.json").exists() {
        return Err(anyhow!("Not in a Mothership project directory."));
    }

    let backup_root = mothership_dir.join("local-backup");
    let mut entries: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = Vec::new();
    if backup_root.exists() {
        for entry in walkdir::WalkDir::new(&backup_root).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let meta = entry.metadata()?;
                let rel = entry.path().strip_prefix(&backup_root).unwrap_or(entry.path()).to_path_buf();
                entries.push((rel, meta.len(), meta.modified().ok()));
            }
        }
    }

    if entries.is_empty() {
        println!("{}", "No local backups in this project.".green());
        return Ok(());
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let total: u64 = entries.iter().map(|(_, size, _)| *size).sum();
    println!("{}", "Local backups:".cyan().bold());
    for (rel, size, modified) in &entries {
        let modified = modified
            .map(|t| DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("  {:>10} bytes  {}  {}", size, modified, rel.display());
    }
    println!("{} file(s), {} bytes total", entries.len(), total);

    print!("Purge all {} backup file(s)? [y/n]: ", entries.len());
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("{}", "Purge cancelled. Backups kept.".yellow());
        return Ok(());
    }

    std::fs::remove_dir_all(&backup_root)?;
    println!("{}", format!("Purged {} backup file(s).", entries.len()).green().bold());
    Ok(())
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

/// Read the error message from a failed server response, falling back to the raw body
async fn response_error_message(response: reqwest::Response) -> String {
    let body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
    serde_json::from_str::<ApiResponse<serde_json::Value>>(&body)
        .ok()
        .and_then(|parsed| parsed.error)
        .unwrap_or(body)
}

/// Helper function to update local rift metadata. `rift_id` is what the daemon syncs to.
fn update_local_rift_metadata(rift_name: &str, rift_id: Option<uuid::Uuid>) -> Result<()> {
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
    if let Some(id) = rift_id {
        project_metadata.rift_id = Some(id.to_string());
    }
    
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
    let project_metadata = get_current_project_metadata()?;
    let project_id = Uuid::parse_str(&project_metadata.project_id)?;
    beam::ensure_daemon_running().await?;
    mothership_common::daemon::reconnect_project(project_id).await?;
    print_success("Synchronized local project with active rift on server!");
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
    println!("{}", "│                    MOTHERSHIP CLI                    │".bright_cyan().bold());
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
    print_command_section("", "auth", "Authentication & Setup", &[
        ("google", "Login with Google OAuth", None),
        ("github", "Login with GitHub OAuth", None),
    ]);
    
    print_command_section("", "gateway", "Project Management", &[
        ("list", "List available projects", Some("--include-inactive")),
        ("release", "Stop tracking a gateway", Some("[project]")),
    ]);
    
    print_command_section("", "beam", "Project Development", &[]);
    println!("    {} {}", "mothership beam".green().bold(), "<gateway> [OPTIONS]".white());
    println!("    {} {}", "   -l, --local-dir".bright_blue(), "<path>     Local directory for project".dimmed());
    println!();
    
    print_command_section("", "status", "Project Status", &[]);
    println!("    {} {}", "mothership status".green().bold(), "                        Check sync status".dimmed());
    println!();
    
    print_command_section("", "checkpoint", "Version Control", &[]);
    println!("    {} {}", "mothership checkpoint".green().bold(), "[-m <message>]  Create a checkpoint".dimmed());
    println!("    {} {}", "   --message".bright_blue(), "<message>          Message (default: \"<rift> <date time>\")".dimmed());
    println!("    {} {}", "mothership checkpoint delete".green().bold(), "<checkpoint-id>".dimmed());
    println!("    {} {}", "   --force".bright_blue(), "                       Skip confirmation".dimmed());
    println!();
    
    print_command_section("", "sync", "Synchronization", &[]);
    println!("    {} {}", "mothership sync".green().bold(), "                         Sync with remote".dimmed());
    println!();
    
    print_command_section("", "history", "Project History", &[]);
    println!("    {} {}", "mothership history".green().bold(), "[OPTIONS]            View checkpoints".dimmed());
    println!("    {} {}", "   --limit".bright_blue(), "<num>               Limit results (default: 20)".dimmed());
    println!();
    
    print_command_section("", "restore", "Time Travel", &[]);
    println!("    {} {}", "mothership restore".green().bold(), "<checkpoint-id>      Restore to checkpoint".dimmed());
    println!("    {} {}", "   --force".bright_blue(), "                       Skip confirmation".dimmed());
    println!();
    
    print_command_section("", "delete", "Project Cleanup", &[]);
    println!("    {} {}", "mothership delete".green().bold(), "<project> [--force]   Delete a project".dimmed());
    println!();
    
    print_command_section("", "connect", "Server Management", &[]);
    println!("    {} {}", "mothership connect".green().bold(), "<url>               Connect to server".dimmed());
    println!();
    
    print_command_section("", "server", "Server Operations", &[
        ("status", "Check connection status", None),
        ("disconnect", "Disconnect from server", None),
        ("list", "List configured servers", None),
    ]);
    
    print_command_section("", "daemon", "Background Service", &[
        ("status", "Show daemon status", None),
        ("stop", "Stop background daemon", None),
        ("restart", "Restart background daemon", None),
    ]);
    
    print_command_section("", "rift", "Collaborative Spaces", &[
        ("list", "List project rifts", Some("--detailed")),
        ("new", "Create a new rift", Some("<name> --description <desc>")),
        ("switch", "Switch to a rift", Some("<name>")),
        ("status", "Show current rift", None),
        ("diff", "Compare rifts", Some("[from] [to]")),
    ]);
    
    print_command_section("", "wormhole", "Merge Previews", &[
        ("create", "Preview merging rifts", Some("<name> <rift> <rift> [rift...]")),
        ("list", "List project wormholes", None),
        ("show", "Show merged state and conflicts", Some("<wormhole>")),
        ("toggle", "Toggle a rift on/off in a wormhole", Some("<wormhole> <rift> [--enable|--disable]")),
        ("resolve", "Resolve a conflict", Some("<wormhole> <path> <conflict> --side <rift> | --text <t> | --file <f>")),
        ("converge", "Turn a wormhole into a rift", Some("<wormhole> [--name <rift>]")),
        ("dissolve", "Archive or delete a wormhole", Some("<wormhole> [-d|--delete]")),
        ("beam", "Copy merged files to a read-only folder", Some("<wormhole> [--dir <folder>]")),
        ("watch", "Report when a wormhole's rifts change", Some("<wormhole>")),
    ]);
    
    print_command_section("", "init", "Quick Init", &[]);
    println!("    {} {}", "mothership init".green().bold(), "[name]                Initialize current directory".dimmed());
    println!();
    
    print_command_section("", "logout", "Session Management", &[]);
    println!("    {} {}", "mothership logout".green().bold(), "                       Clear credentials".dimmed());
    println!();
    
    print_command_section("⬆", "update", "CLI Maintenance", &[]);
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
    println!("    {} {}", "mothership checkpoint -m \"Feature works\"".green(), "   # Save progress".dimmed());
    println!("    {} {}", "mothership daemon status".green(), "                  # Check background sync".dimmed());
    println!();
    
    println!("{}", "WORKFLOW:".bright_yellow().bold());
    println!("    {} {}", "1.".bright_cyan().bold(), "mothership auth google          # Authenticate");
    println!("    {} {}", "2.".bright_cyan().bold(), "mothership connect <server>     # Connect to server");
    println!("    {} {}", "3.".bright_cyan().bold(), "mothership gateway list         # Browse projects");
    println!("    {} {}", "4.".bright_cyan().bold(), "mothership beam <project>       # Join project");
    println!("    {} {}", "5.".bright_cyan().bold(), "mothership rift new <branch>    # Create workspace");
    println!("    {} {}", "6.".bright_cyan().bold(), "Edit files...                   # Work on your code");
    println!("    {} {}", "7.".bright_cyan().bold(), "mothership checkpoint [-m <message>] # Save progress");
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