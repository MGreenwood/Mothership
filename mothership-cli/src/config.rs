use anyhow::{anyhow, Result};
use mothership_common::ClientConfig;
use std::fs;
use std::path::PathBuf;

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?
            .join("mothership");
        
        // Create config directory if it doesn't exist
        fs::create_dir_all(&config_dir)?;
        
        let config_path = config_dir.join("config.json");
        
        Ok(Self { config_path })
    }

    /// Load configuration from disk
    pub fn load_config(&self) -> Result<ClientConfig> {
        if !self.config_path.exists() {
            // Return default config if file doesn't exist
            return Ok(ClientConfig::default());
        }

        let config_content = fs::read_to_string(&self.config_path)?;
        let config: ClientConfig = serde_json::from_str(&config_content)
            .map_err(|e| anyhow!("Failed to parse config file: {}", e))?;

        Ok(config)
    }

    /// Save configuration to disk
    pub fn save_config(&self, config: &ClientConfig) -> Result<()> {
        let config_json = serde_json::to_string_pretty(config)
            .map_err(|e| anyhow!("Failed to serialize config: {}", e))?;

        fs::write(&self.config_path, config_json)
            .map_err(|e| anyhow!("Failed to write config file: {}", e))?;

        Ok(())
    }

    /// Check if user is authenticated
    pub fn is_authenticated(&self) -> Result<bool> {
        let config = self.load_config()?;
        Ok(config.auth_token.is_some() && config.user_id.is_some())
    }

    /// Get the config file path for display
    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    /// Update just the auth token and user ID
    pub fn update_auth(&self, token: String, user_id: uuid::Uuid) -> Result<()> {
        let mut config = self.load_config()?;
        config.auth_token = Some(token);
        config.user_id = Some(user_id);
        self.save_config(&config)
    }

    /// Save authentication data (alias for OAuth compatibility)
    pub fn save_auth(&self, access_token: String, _refresh_token: String, _username: String, user_id: uuid::Uuid) -> Result<()> {
        // For now, we just store the access token and user ID
        // In a full implementation, we'd store the refresh token and username too
        self.update_auth(access_token, user_id)
    }

    /// Clear authentication
    pub fn clear_auth(&self) -> Result<()> {
        let mut config = self.load_config()?;
        config.auth_token = None;
        config.user_id = None;
        self.save_config(&config)
    }

    /// Get workspace directory for a project
    pub fn get_project_workspace(&self, project_name: &str) -> Result<PathBuf> {
        let config = self.load_config()?;
        let workspace = config.local_workspace.join(project_name);
        
        // Create workspace directory if it doesn't exist
        fs::create_dir_all(&workspace)?;
        
        Ok(workspace)
    }
} 