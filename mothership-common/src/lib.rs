use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub mod auth;
pub mod beam;
pub mod client;
pub mod crdt;
pub mod daemon;
pub mod diff;
pub mod merge;
pub mod protocol;
pub mod streaming;
pub mod transaction;

pub use auth::*;
pub use crdt::*;
pub use diff::*;
pub use protocol::*;
pub use streaming::*;
pub use transaction::{Transaction, TransactionStatus};

// Re-export commonly used types
pub use protocol::{SyncMessage, ConflictRiftInfo, FileDiff, DiffOperation, FileDiffChange};

// Core ID types
pub type UserId = Uuid;
pub type ProjectId = Uuid;
pub type RiftId = Uuid;
pub type CheckpointId = Uuid;

/// SHA-256 hex digest used as the sync base hash. Client and server must agree on it.
pub fn content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

#[cfg(test)]
mod content_hash_tests {
    use super::content_hash;

    #[test]
    fn same_content_gives_same_hash() {
        assert_eq!(content_hash("hello\n"), content_hash("hello\n"));
    }

    #[test]
    fn trailing_newline_changes_hash() {
        assert_ne!(content_hash("hello"), content_hash("hello\n"));
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = content_hash("abc");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Known SHA-256 of "abc"
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
pub enum UserRole {
    #[sqlx(rename = "super_admin")]
    SuperAdmin,  // Can create admins, manage all projects
    #[sqlx(rename = "admin")]
    Admin,       // Can create projects, manage users
    #[sqlx(rename = "user")]
    User,        // Regular user
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub description: String,
    pub members: Vec<UserId>,
    pub created_at: DateTime<Utc>,
    pub settings: ProjectSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub auto_checkpoint_interval: u64, // seconds
    pub max_checkpoint_history: u32,
    pub allowed_file_types: Vec<String>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            auto_checkpoint_interval: 10, // 10 seconds
            max_checkpoint_history: 1000,
            allowed_file_types: vec![
                "*.rs".to_string(),
                "*.js".to_string(),
                "*.ts".to_string(),
                "*.py".to_string(),
                "*.go".to_string(),
                "*.java".to_string(),
                "*.cpp".to_string(),
                "*.c".to_string(),
                "*.h".to_string(),
                "*.md".to_string(),
                "*.txt".to_string(),
                "*.json".to_string(),
                "*.yaml".to_string(),
                "*.yml".to_string(),
                "*.toml".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rift {
    pub id: RiftId,
    pub project_id: ProjectId,
    pub name: String,
    pub parent_rift: Option<RiftId>,
    pub collaborators: Vec<UserId>,
    pub created_at: DateTime<Utc>,
    pub last_checkpoint: Option<CheckpointId>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub rift_id: RiftId,
    pub author: UserId,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<FileChange>,
    pub parent: Option<CheckpointId>,
    pub message: Option<String>, // Optional user annotation
    pub auto_generated: bool,    // True for automatic checkpoints, false for manual
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub change_type: ChangeType,
    pub content_hash: String,
    pub diff: Option<String>, // Unified diff format
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Moved { from: PathBuf },
}

// Gateway response for project discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayProject {
    pub project: Project,
    pub active_rifts: Vec<RiftSummary>,
    pub your_rifts: Vec<RiftSummary>,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiftSummary {
    pub id: RiftId,
    pub name: String,
    pub collaborators: Vec<String>, // usernames for display
    pub last_checkpoint: Option<DateTime<Utc>>,
    pub change_count: u32,
}

// Configuration for local client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub mothership_url: String,
    pub auth_token: Option<String>,
    pub local_workspace: PathBuf,
    pub user_id: Option<UserId>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        // Get port from environment or use default
        let port = std::env::var("MOTHERSHIP_PORT")
            .unwrap_or_else(|_| "7523".to_string());
        
        let mothership_url = format!("http://localhost:{}", port);
        
        Self {
            mothership_url,
            auth_token: None,
            local_workspace: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("mothership"),
            user_id: None,
        }
    }
} 