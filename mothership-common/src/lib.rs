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
pub mod ignore;
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

/// Normalizes a relative path to forward slashes (`/`) so Windows and Unix
/// clients, daemons, and servers always agree on path keys.
pub fn normalize_rel_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

/// Prefix the server gives rifts created to hold a stale-base edit
pub const CONFLICT_RIFT_PREFIX: &str = "conflict-";

/// Whether a rift name was generated for a stale-base conflict
pub fn is_conflict_rift_name(name: &str) -> bool {
    name.starts_with(CONFLICT_RIFT_PREFIX)
}

/// Save a local file's current bytes under `.mothership/local-backup/` before server content
/// overwrites it. Returns the backup path, or None when the file is missing or already matches.
pub fn preserve_local_version(project_path: &std::path::Path, rel_path: &std::path::Path, server_content: &[u8]) -> std::io::Result<Option<std::path::PathBuf>> {
    let local = match std::fs::read(project_path.join(rel_path)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if local == server_content {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = project_path.join(".mothership").join("local-backup").join(stamp.to_string()).join(rel_path);
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&backup, local)?;
    Ok(Some(backup))
}

#[cfg(test)]
mod conflict_and_backup_tests {
    use super::{is_conflict_rift_name, preserve_local_version, CONFLICT_RIFT_PREFIX};
    use std::path::Path;

    #[test]
    fn conflict_rift_names_are_detected() {
        assert!(is_conflict_rift_name(&format!("{}alice-src/main.rs", CONFLICT_RIFT_PREFIX)));
        assert!(!is_conflict_rift_name("main"));
        assert!(!is_conflict_rift_name("feature-conflict-x"));
    }

    #[test]
    fn overwrite_of_differing_file_keeps_local_copy() {
        let dir = std::env::temp_dir().join(format!("mothership-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "local edit").unwrap();

        let backup = preserve_local_version(&dir, Path::new("src/lib.rs"), b"server version").unwrap()
            .expect("differing local file should be backed up");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "local edit");
        assert!(backup.starts_with(dir.join(".mothership").join("local-backup")));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identical_or_missing_file_is_not_backed_up() {
        let dir = std::env::temp_dir().join(format!("mothership-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "same").unwrap();

        assert!(preserve_local_version(&dir, Path::new("a.txt"), b"same").unwrap().is_none());
        assert!(preserve_local_version(&dir, Path::new("missing.txt"), b"new").unwrap().is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }
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