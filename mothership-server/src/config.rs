use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

/// Server configuration loaded from server.config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// General server settings
    pub server: ServerSettings,
    
    /// Feature toggles
    pub features: FeatureSettings,
    
    /// Authentication and access control
    pub auth: AuthSettings,
    
    /// Real-time collaboration settings
    pub collaboration: CollaborationSettings,
    
    /// CLI distribution settings
    pub cli_distribution: CliDistributionSettings,
    
    /// File streaming settings
    pub streaming: StreamingSettings,

    /// Upload size and count limits
    #[serde(default)]
    pub limits: UploadLimits,
}

/// Limits applied to files uploaded to the server. Missing keys use the defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadLimits {
    /// Largest single file accepted, in bytes
    pub max_file_bytes: u64,

    /// Largest number of files accepted in one upload request
    pub max_files_per_request: usize,

    /// Largest accepted request body, in bytes (checked before parsing)
    pub max_request_bytes: usize,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 5 * 1024 * 1024,
            max_files_per_request: 100,
            max_request_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// Server bind address (default: "0.0.0.0")
    pub host: String,
    
    /// Server port (default: 7523)
    pub port: u16,
    
    /// Web UI port (if different from main port, runs separate web server)
    pub web_port: Option<u16>,
    
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    
    /// Request timeout in seconds
    pub request_timeout: u64,
    
    /// Enable detailed logging
    pub debug_logging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSettings {
    /// Enable/disable real-time chat in rifts
    pub chat_enabled: bool,
    
    /// Enable/disable file sharing via uploads
    pub file_uploads_enabled: bool,
    
    /// Enable/disable project creation by users
    pub project_creation_enabled: bool,
    
    /// Enable/disable CLI distribution endpoints
    pub cli_distribution_enabled: bool,
    
    /// Enable/disable OAuth authentication
    pub oauth_enabled: bool,
    
    /// Enable/disable WebSocket real-time sync
    pub websocket_sync_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Enable user whitelist (if true, only whitelisted users can access)
    pub whitelist_enabled: bool,
    
    /// Path to whitelist file (relative to server.config)
    pub whitelist_path: String,
    
    /// Require authentication for all endpoints
    pub require_auth: bool,
    
    /// JWT token expiration time in days
    pub token_expiration_days: i64,
    
    /// Maximum failed login attempts before temporary ban
    pub max_login_attempts: u32,
    
    /// Temporary ban duration in minutes
    pub ban_duration_minutes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationSettings {
    /// Maximum number of users per rift
    pub max_users_per_rift: usize,
    
    /// Maximum message length for chat
    pub max_chat_message_length: usize,
    
    /// Enable message history storage
    pub store_chat_history: bool,
    
    /// Maximum number of chat messages to store per rift
    pub max_chat_history: usize,
    
    /// Enable presence indicators (who's online)
    pub presence_enabled: bool,
    
    /// Presence update interval in seconds
    pub presence_update_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliDistributionSettings {
    /// Path to CLI binaries directory
    pub binaries_path: String,
    
    /// Whether to require authentication for downloads
    pub require_auth_for_downloads: bool,
    
    /// Maximum downloads per hour per IP
    pub max_downloads_per_hour: u64,
    
    /// Whether to track download statistics
    pub track_downloads: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingSettings {
    /// Whether file streaming is enabled
    pub enabled: bool,
    
    /// File size threshold for streaming (in bytes)
    pub threshold: u64,
    
    /// Chunk size for streaming (in bytes)
    pub chunk_size: usize,
    
    /// Maximum concurrent streams
    pub max_concurrent_streams: usize,
    
    /// Stream timeout in seconds
    pub timeout_seconds: u64,
    
    /// Whether to enable compression
    pub enable_compression: bool,
    
    /// Compression level (0-9)
    pub compression_level: u32,
}

/// Per-user options from a whitelist line, e.g. `ghostid... admin nickname=mgreen`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Grant {
    pub admin: bool,
    pub nickname: Option<String>,
}

/// User whitelist loaded from whitelist file
#[derive(Debug, Clone)]
pub struct UserWhitelist {
    /// Set of allowed usernames
    pub usernames: HashSet<String>,
    
    /// Set of allowed email addresses
    pub emails: HashSet<String>,
    
    /// Set of allowed email domains (e.g., "company.com")
    pub domains: HashSet<String>,

    /// Options keyed by the username or email on the line
    pub grants: HashMap<String, Grant>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                host: "0.0.0.0".to_string(),
                port: 7523,
                web_port: None, // None = same port as API
                max_connections: 1000,
                request_timeout: 30,
                debug_logging: false,
            },
            features: FeatureSettings {
                chat_enabled: true,
                file_uploads_enabled: true,
                project_creation_enabled: true,
                cli_distribution_enabled: true,
                oauth_enabled: true,
                websocket_sync_enabled: true,
            },
            auth: AuthSettings {
                whitelist_enabled: false,
                whitelist_path: "whitelist.config".to_string(),
                require_auth: true,
                token_expiration_days: 30,
                max_login_attempts: 5,
                ban_duration_minutes: 15,
            },
            collaboration: CollaborationSettings {
                max_users_per_rift: 50,
                max_chat_message_length: 1000,
                store_chat_history: true,
                max_chat_history: 1000,
                presence_enabled: true,
                presence_update_interval: 30,
            },
            cli_distribution: CliDistributionSettings {
                binaries_path: "cli-binaries".to_string(),
                require_auth_for_downloads: true,
                max_downloads_per_hour: 100,
                track_downloads: true,
            },
            streaming: StreamingSettings {
                enabled: true,
                threshold: 10 * 1024 * 1024, // 10MB
                chunk_size: 64 * 1024, // 64KB
                max_concurrent_streams: 10,
                timeout_seconds: 300, // 5 minutes
                enable_compression: true,
                compression_level: 6,
            },
            limits: UploadLimits::default(),
        }
    }
}

impl ServerConfig {
    /// Load configuration from file, creating default if it doesn't exist
    pub fn load_from_file<P: AsRef<Path>>(config_path: P) -> Result<Self> {
        let config_path = config_path.as_ref();
        
        if !config_path.exists() {
            info!("Server config not found, creating default: {}", config_path.display());
            let default_config = Self::default();
            default_config.save_to_file(config_path)?;
            return Ok(default_config);
        }
        
        info!("Loading server config from: {}", config_path.display());
        let config_content = fs::read_to_string(config_path)
            .map_err(|e| anyhow!("Failed to read config file: {}", e))?;
        
        // Support both TOML and simple key=value format
        let config = if config_content.trim_start().starts_with('[') {
            // TOML format
            toml::from_str(&config_content)
                .map_err(|e| anyhow!("Failed to parse TOML config: {}", e))?
        } else {
            // Simple key=value format (for backwards compatibility)
            Self::parse_simple_format(&config_content)?
        };
        
        info!("Server config loaded successfully");
        Ok(config)
    }
    
    /// Save configuration to file in TOML format
    pub fn save_to_file<P: AsRef<Path>>(&self, config_path: P) -> Result<()> {
        let config_toml = toml::to_string_pretty(self)
            .map_err(|e| anyhow!("Failed to serialize config: {}", e))?;
        
        fs::write(config_path.as_ref(), config_toml)
            .map_err(|e| anyhow!("Failed to write config file: {}", e))?;
        
        info!("Server config saved to: {}", config_path.as_ref().display());
        Ok(())
    }
    
    /// Parse simple key=value format for backwards compatibility
    fn parse_simple_format(content: &str) -> Result<Self> {
        let mut config = Self::default();
        
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() != 2 {
                warn!("Invalid config line: {}", line);
                continue;
            }
            
            let key = parts[0].trim();
            let value = parts[1].trim();
            
            match key {
                "chat_enabled" => config.features.chat_enabled = parse_bool(value)?,
                "whitelist_enabled" | "whitelist" => config.auth.whitelist_enabled = parse_bool(value)?,
                "whitelist_path" => config.auth.whitelist_path = value.to_string(),
                "port" => config.server.port = value.parse()?,
                "web_port" => config.server.web_port = Some(value.parse()?),
                "host" => config.server.host = value.to_string(),
                "debug_logging" => config.server.debug_logging = parse_bool(value)?,
                "oauth_enabled" => config.features.oauth_enabled = parse_bool(value)?,
                "cli_distribution_enabled" => config.features.cli_distribution_enabled = parse_bool(value)?,
                _ => warn!("Unknown config key: {}", key),
            }
        }
        
        Ok(config)
    }
    
    /// Load user whitelist from file
    pub fn load_whitelist(&self) -> Result<Option<UserWhitelist>> {
        if !self.auth.whitelist_enabled {
            return Ok(None);
        }
        
        let whitelist_path = Path::new(&self.auth.whitelist_path);
        if !whitelist_path.exists() {
            warn!("Whitelist enabled but file not found: {}", whitelist_path.display());
            return Ok(None);
        }
        
        info!("Loading user whitelist from: {}", whitelist_path.display());
        let content = fs::read_to_string(whitelist_path)
            .map_err(|e| anyhow!("Failed to read whitelist file: {}", e))?;
        
        let mut usernames = HashSet::new();
        let mut emails = HashSet::new();
        let mut domains = HashSet::new();
        let mut grants = HashMap::new();
        
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            if line.starts_with('@') {
                // Domain (e.g., @company.com)
                domains.insert(line[1..].to_string());
                continue;
            }
            let mut parts = line.split_whitespace();
            let identity = parts.next().unwrap_or_default().to_string();
            let grant = parse_grant(parts);
            if identity.contains('@') {
                emails.insert(identity.clone());
            } else {
                usernames.insert(identity.clone());
            }
            if grant != Grant::default() {
                grants.insert(identity, grant);
            }
        }
        
        info!("Whitelist loaded: {} usernames, {} emails, {} domains", 
            usernames.len(), emails.len(), domains.len());
        
        Ok(Some(UserWhitelist {
            usernames,
            emails,
            domains,
            grants,
        }))
    }
}

impl UserWhitelist {
    /// Options for a user, matched by username first, then email. Defaults when the line has none.
    pub fn grant_for(&self, username: &str, email: &str) -> Grant {
        self.grants
            .get(username)
            .or_else(|| self.grants.get(email))
            .cloned()
            .unwrap_or_default()
    }

    /// Check if a user is allowed based on username and email
    pub fn is_user_allowed(&self, username: &str, email: &str) -> bool {
        // Check exact username match
        if self.usernames.contains(username) {
            return true;
        }
        
        // Check exact email match
        if self.emails.contains(email) {
            return true;
        }
        
        // Check email domain
        if let Some(domain) = email.split('@').nth(1) {
            if self.domains.contains(domain) {
                return true;
            }
        }
        
        false
    }
}

/// Parses the options after a whitelist identity. Unknown options are logged and ignored.
fn parse_grant<'a>(options: impl Iterator<Item = &'a str>) -> Grant {
    let mut grant = Grant::default();
    for option in options {
        if option == "admin" {
            grant.admin = true;
        } else if let Some(name) = option.strip_prefix("nickname=") {
            if !name.is_empty() {
                grant.nickname = Some(name.to_string());
            }
        } else {
            warn!("Unknown whitelist option: {}", option);
        }
    }
    grant
}

/// Parse boolean value from string
fn parse_bool(value: &str) -> Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" | "enabled" => Ok(true),
        "false" | "no" | "0" | "off" | "disabled" => Ok(false),
        _ => Err(anyhow!("Invalid boolean value: {}", value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_bool() {
        assert_eq!(parse_bool("true").unwrap(), true);
        assert_eq!(parse_bool("false").unwrap(), false);
        assert_eq!(parse_bool("yes").unwrap(), true);
        assert_eq!(parse_bool("no").unwrap(), false);
        assert_eq!(parse_bool("1").unwrap(), true);
        assert_eq!(parse_bool("0").unwrap(), false);
    }
    
    #[test]
    fn test_whitelist_user_allowed() {
        let mut whitelist = UserWhitelist {
            usernames: HashSet::new(),
            emails: HashSet::new(),
            domains: HashSet::new(),
            grants: HashMap::new(),
        };
        
        whitelist.usernames.insert("alice".to_string());
        whitelist.emails.insert("bob@example.com".to_string());
        whitelist.domains.insert("company.com".to_string());
        
        assert!(whitelist.is_user_allowed("alice", "alice@anywhere.com"));
        assert!(whitelist.is_user_allowed("bob", "bob@example.com"));
        assert!(whitelist.is_user_allowed("charlie", "charlie@company.com"));
        assert!(!whitelist.is_user_allowed("eve", "eve@malicious.com"));
    }
} 