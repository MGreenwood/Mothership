use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub mod anchors;
pub mod auth;
pub mod beam;
pub mod client;
pub mod crdt;
pub mod daemon;
pub mod diff;
pub mod ignore;
pub mod merge;
pub mod protocol;
pub mod secrets;
pub mod streaming;
pub mod text_diff;
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

/// Normalizes a user-typed path selector for matching: forward slashes, no `.` segments, and a
/// trailing `/` for folders. Absolute paths, drive letters, and `..` are rejected.
pub fn normalize_selector(selector: &str) -> Result<String, String> {
    let unified = selector.trim().replace('\\', "/");
    let is_folder = unified.ends_with('/');
    let parts: Vec<&str> = unified
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    if unified.starts_with('/') || unified.contains(':') || parts.contains(&"..") {
        return Err(format!("'{}' must be a path inside the project", selector));
    }
    if parts.is_empty() {
        return Err("empty path selector".to_string());
    }

    let mut normalized = parts.join("/");
    if is_folder {
        normalized.push('/');
    }
    Ok(normalized)
}

/// Picks the files named by `selectors`. A folder selector (ending in `/`) takes every file under
/// it; a file selector takes that one file. Fails if any selector matches nothing.
pub fn select_paths(
    files: &std::collections::HashMap<PathBuf, String>,
    selectors: &[String],
) -> Result<std::collections::HashMap<PathBuf, String>, String> {
    let mut picked = std::collections::HashMap::new();
    let mut unmatched = Vec::new();

    for selector in selectors {
        let selector = normalize_selector(selector)?;
        let mut matched = false;
        for (path, content) in files {
            let key = normalize_rel_path(path);
            let hit = if selector.ends_with('/') {
                key.starts_with(selector.as_str())
            } else {
                key == selector
            };
            if hit {
                picked.insert(path.clone(), content.clone());
                matched = true;
            }
        }
        if !matched {
            unmatched.push(selector);
        }
    }

    if !unmatched.is_empty() {
        return Err(format!("nothing in the rift matches: {}", unmatched.join(", ")));
    }
    Ok(picked)
}
/// Prefix the server gives rifts created to hold a stale-base edit
pub const CONFLICT_RIFT_PREFIX: &str = "conflict-";

#[cfg(test)]
mod selector_tests {
    use super::{normalize_selector, select_paths};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn files() -> HashMap<PathBuf, String> {
        ["README.md", "src/main.rs", "src/util/mod.rs", "srcs/other.rs"]
            .iter()
            .map(|p| (PathBuf::from(p), p.to_string()))
            .collect()
    }

    #[test]
    fn normalizes_separators_dots_and_folder_marker() {
        assert_eq!(normalize_selector(r"src\util\").unwrap(), "src/util/");
        assert_eq!(normalize_selector("./README.md").unwrap(), "README.md");
        assert_eq!(normalize_selector("  src/  ").unwrap(), "src/");
    }

    #[test]
    fn rejects_paths_outside_the_project() {
        assert!(normalize_selector("../secret").is_err());
        assert!(normalize_selector("/etc/passwd").is_err());
        assert!(normalize_selector(r"C:\Windows").is_err());
        assert!(normalize_selector(".").is_err());
    }

    #[test]
    fn folder_selector_takes_only_files_under_it() {
        let picked = select_paths(&files(), &["src/".to_string()]).unwrap();
        let mut names: Vec<String> = picked.keys().map(|p| p.display().to_string().replace('\\', "/")).collect();
        names.sort();
        assert_eq!(names, vec!["src/main.rs", "src/util/mod.rs"]);
    }

    #[test]
    fn file_selector_takes_exactly_that_file() {
        let picked = select_paths(&files(), &["README.md".to_string()]).unwrap();
        assert_eq!(picked.len(), 1);
        assert!(picked.contains_key(&PathBuf::from("README.md")));
    }

    #[test]
    fn unmatched_selector_is_an_error_naming_it() {
        let err = select_paths(&files(), &["missing.rs".to_string()]).unwrap_err();
        assert!(err.contains("missing.rs"), "{err}");
    }
}

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
    /// True when `changes` holds only the selected files. The rest of the rift's state comes from
    /// the parent checkpoint. False (the default, and what older checkpoints load as) means
    /// `changes` is the whole rift.
    #[serde(default)]
    pub partial: bool,
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