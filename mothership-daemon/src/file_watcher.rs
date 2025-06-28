use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tokio::sync::mpsc as async_mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

/// File change event sent to the daemon
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub project_id: Uuid,
    pub file_path: PathBuf,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Background file watcher for a single project
pub struct FileWatcher {
    project_path: PathBuf,
    project_id: Uuid,
    _watcher: RecommendedWatcher, // Keep alive to maintain watching
}

impl FileWatcher {
    /// Create a new file watcher for a project
    pub async fn new(
        project_path: PathBuf,
        project_id: Uuid,
        change_sender: async_mpsc::UnboundedSender<FileChangeEvent>,
    ) -> Result<Self> {
        info!("🔍 Setting up file watcher for project {} at {}", 
            project_id, project_path.display());
        
        // Validate project path
        if !project_path.exists() {
            return Err(anyhow::anyhow!("Project path does not exist: {}", project_path.display()));
        }
        
        // Create sync channel for file system events
        let (fs_tx, fs_rx) = mpsc::channel();
        
        // Create the file system watcher
        let mut watcher = RecommendedWatcher::new(fs_tx, Config::default())?;
        watcher.watch(&project_path, RecursiveMode::Recursive)?;
        
        // Spawn background task to handle file system events
        let project_path_clone = project_path.clone();
        tokio::task::spawn_blocking(move || {
            for res in fs_rx {
                match res {
                    Ok(event) => {
                        if let Err(e) = handle_file_event(
                            &event, 
                            &project_path_clone, 
                            project_id, 
                            &change_sender
                        ) {
                            error!("Error handling file event in project {}: {}", project_id, e);
                        }
                    }
                    Err(e) => error!("File watcher error for project {}: {}", project_id, e),
                }
            }
            info!("🔍 File watcher stopped for project {}", project_id);
        });
        
        info!("✅ File watcher started for project {} at {}", 
            project_id, project_path.display());
        
        Ok(Self {
            project_path,
            project_id,
            _watcher: watcher,
        })
    }
}

/// Handle a file system event and send change events to daemon
fn handle_file_event(
    event: &Event,
    project_path: &Path,
    project_id: Uuid,
    change_sender: &async_mpsc::UnboundedSender<FileChangeEvent>,
) -> Result<()> {
    // Filter out events we don't care about
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            // Process the event
        }
        _ => {
            return Ok(()); // Ignore other event types
        }
    }
    
    for path in &event.paths {
        // Skip hidden files and directories
        if path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        
        // Skip directories
        if path.is_dir() {
            continue;
        }
        
        // Skip common build/cache directories and temporary files
        let path_str = path.to_string_lossy();
        if should_ignore_file(&path_str) {
            continue;
        }
        
        // Calculate relative path
        let relative_path = match path.strip_prefix(project_path) {
            Ok(rel_path) => rel_path.to_path_buf(),
            Err(_) => {
                warn!("Path {} is outside project directory {}", 
                    path.display(), project_path.display());
                continue;
            }
        };
        
        // Validate relative path isn't corrupted
        let relative_path_str = relative_path.to_string_lossy();
        if relative_path_str.len() > 1000 {
            error!("Detected corrupted path: {} (original: {})", 
                relative_path_str, path.display());
            continue;
        }
        
        // Read file content
        match std::fs::read_to_string(path) {
            Ok(content) => {
                info!("📝 File changed in project {}: {}", project_id, relative_path.display());
                
                let change_event = FileChangeEvent {
                    project_id,
                    file_path: relative_path,
                    content,
                    timestamp: chrono::Utc::now(),
                };
                
                if let Err(e) = change_sender.send(change_event) {
                    error!("Failed to send file change event: {}", e);
                }
            }
            Err(e) => {
                // Skip binary files or files we can't read
                info!("Skipping unreadable file {}: {}", path.display(), e);
            }
        }
    }
    
    Ok(())
}

/// Check if a file should be ignored during file watching
fn should_ignore_file(path_str: &str) -> bool {
    // Common patterns to ignore
    let ignore_patterns = [
        "target/", "node_modules/", ".git/", "dist/", "build/", 
        ".mothership/", ".vscode/", ".idea/", "__pycache__/",
        ".lock", "~", ".tmp", ".temp", ".log", ".cache",
        ".DS_Store", "Thumbs.db", "desktop.ini",
    ];
    
    for pattern in &ignore_patterns {
        if pattern.ends_with('/') {
            // Directory pattern
            if path_str.contains(pattern) {
                return true;
            }
        } else {
            // File extension or suffix pattern
            if path_str.ends_with(pattern) || path_str.contains(pattern) {
                return true;
            }
        }
    }
    
    false
} 