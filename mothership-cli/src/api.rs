use anyhow::Result;
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftDiff {
    pub path: PathBuf,
    pub change_count: usize,
}

pub async fn get_rifts() -> Result<Vec<RiftInfo>> {
    let config = get_config()?;
    let url = format!("{}/api/rifts", config.server_url);
    
    let response = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .send()
        .await?
        .error_for_status()?;
    
    let rifts = response.json().await?;
    Ok(rifts)
}

pub async fn create_rift(name: &str, description: Option<String>) -> Result<Uuid> {
    let config = get_config()?;
    let url = format!("{}/api/rifts", config.server_url);
    
    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .json(&serde_json::json!({
            "name": name,
            "description": description,
        }))
        .send()
        .await?
        .error_for_status()?;
    
    let rift_id: Uuid = response.json().await?;
    Ok(rift_id)
}

pub async fn switch_to_rift(rift_name: &str) -> Result<()> {
    let config = get_config()?;
    let url = format!("{}/api/rifts/switch", config.server_url);
    
    reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .json(&serde_json::json!({
            "rift_name": rift_name,
        }))
        .send()
        .await?
        .error_for_status()?;
    
    // Update local rift state
    let mut local_config = read_local_config()?;
    local_config.current_rift = Some(rift_name.to_string());
    write_local_config(&local_config)?;
    
    Ok(())
}

pub async fn get_current_rift() -> Result<Option<RiftInfo>> {
    let local_config = read_local_config()?;
    
    if let Some(rift_name) = local_config.current_rift {
        let config = get_config()?;
        let url = format!("{}/api/rifts/current", config.server_url);
        
        let response = reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("Bearer {}", config.auth_token))
            .send()
            .await?
            .error_for_status()?;
        
        let rift: Option<RiftInfo> = response.json().await?;
        Ok(rift)
    } else {
        Ok(None)
    }
}

pub async fn get_rift_diffs(from: &str, to: &str) -> Result<Vec<RiftDiff>> {
    let config = get_config()?;
    let url = format!("{}/api/rifts/diff", config.server_url);
    
    let response = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .query(&[("from", from), ("to", to)])
        .send()
        .await?
        .error_for_status()?;
    
    let diffs = response.json().await?;
    Ok(diffs)
}

#[derive(Debug, Serialize, Deserialize)]
struct LocalConfig {
    current_rift: Option<String>,
    // ... other local config fields ...
}

fn read_local_config() -> Result<LocalConfig> {
    let path = std::env::current_dir()?.join(".mothership/config.json");
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(LocalConfig {
            current_rift: None,
        })
    }
}

fn write_local_config(config: &LocalConfig) -> Result<()> {
    let path = std::env::current_dir()?.join(".mothership/config.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(config)?)?;
    Ok(())
} 