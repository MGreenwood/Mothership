use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as async_mpsc;
use tracing::{error, info, warn, debug};
use uuid::Uuid;

/// Maximum file size to process (1MB limit)
const MAX_FILE_SIZE: u64 = 1_048_576; // 1MB in bytes

/// Minimum debounce interval between file events (100ms)
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(100);

/// File change event sent to the daemon
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub project_id: Uuid,
    pub file_path: PathBuf,
    pub content: String,  // CRITICAL: Restored for sync functionality
    pub file_size: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub change_type: FileChangeType,
}

#[derive(Debug, Clone)]
pub enum FileChangeType {
    Created,
    Modified,
    Deleted,
}

/// Debouncing state for file events
struct FileDebouncer {
    last_event_time: HashMap<PathBuf, Instant>,
}

impl FileDebouncer {
    fn new() -> Self {
        Self {
            last_event_time: HashMap::new(),
        }
    }

    /// Check if enough time has passed since the last event for this file
    fn should_process_event(&mut self, path: &PathBuf) -> bool {
        let now = Instant::now();
        
        if let Some(&last_time) = self.last_event_time.get(path) {
            if now.duration_since(last_time) < DEBOUNCE_INTERVAL {
                debug!("⏳ Debouncing file event for {}", path.display());
                return false;
            }
        }
        
        self.last_event_time.insert(path.clone(), now);
        true
    }

    /// Clean up old entries to prevent memory leak
    fn cleanup_old_entries(&mut self) {
        let now = Instant::now();
        let cutoff = Duration::from_secs(300); // 5 minutes
        
        self.last_event_time.retain(|_, &mut last_time| {
            now.duration_since(last_time) < cutoff
        });
    }
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
        
        // CRITICAL FIX: Create a sync channel bridge for async/sync boundary
        let (sync_tx, sync_rx) = mpsc::channel::<FileChangeEvent>();
        
        // Spawn async task to bridge sync -> async channels
        let async_sender = change_sender.clone();
        tokio::spawn(async move {
            info!("🌉 Starting async/sync bridge for file watcher");
            // Convert sync receiver to iterator and process events
            while let Ok(event) = sync_rx.recv() {
                debug!("🔄 Forwarding file change event through async bridge");
                if let Err(e) = async_sender.send(event) {
                    error!("Failed to forward file change event to daemon: {}", e);
                    break; // Channel closed, stop the bridge
                }
            }
            info!("🌉 Async/sync bridge stopped");
        });
        
        // Spawn background task to handle file system events
        let project_path_clone = project_path.clone();
        tokio::task::spawn_blocking(move || {
            info!("👀 File watcher blocking task started for project {}", project_id);
            let mut debouncer = FileDebouncer::new();
            let mut cleanup_counter = 0;
            
            for res in fs_rx {
                match res {
                    Ok(event) => {
                        debug!("🔔 Received file system event: {:?}", event.kind);
                        if let Err(e) = handle_file_event(
                            &event, 
                            &project_path_clone, 
                            project_id, 
                            &sync_tx,  // Use sync channel here!
                            &mut debouncer
                        ) {
                            error!("Error handling file event in project {}: {}", project_id, e);
                        }
                    }
                    Err(e) => error!("File watcher error for project {}: {}", project_id, e),
                }
                
                // Periodic cleanup of debouncer to prevent memory leaks
                cleanup_counter += 1;
                if cleanup_counter % 1000 == 0 {
                    debouncer.cleanup_old_entries();
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
    change_sender: &mpsc::Sender<FileChangeEvent>,  // Now using sync channel!
    debouncer: &mut FileDebouncer,
) -> Result<()> {
    // Determine change type and filter events
    let change_type = match event.kind {
        EventKind::Create(_) => FileChangeType::Created,
        EventKind::Modify(_) => FileChangeType::Modified,
        EventKind::Remove(_) => FileChangeType::Deleted,
        _ => {
            return Ok(()); // Ignore other event types
        }
    };
    
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
        
        // PERFORMANCE FIX: Apply debouncing
        if !debouncer.should_process_event(&relative_path) {
            continue;
        }
        
        // PERFORMANCE FIX: Check file size without reading content
        let file_size = match std::fs::metadata(path) {
            Ok(metadata) => metadata.len(),
            Err(e) => {
                debug!("Skipping file with unreadable metadata {}: {}", path.display(), e);
                continue;
            }
        };
        
        // PERFORMANCE FIX: Skip files larger than 1MB
        if file_size > MAX_FILE_SIZE {
            debug!("⚠️ Skipping large file {} ({} bytes > {} bytes limit)", 
                path.display(), file_size, MAX_FILE_SIZE);
            continue;
        }
        
        // Read file content for sync (CRITICAL: Restored for data safety)
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                error!("Failed to read file content for {}: {}", path.display(), e);
                continue;
            }
        };
        
        info!("📝 File changed in project {}: {} ({} bytes)", 
            project_id, relative_path.display(), file_size);
        
        let change_event = FileChangeEvent {
            project_id,
            file_path: relative_path,
            content,
            file_size,
            timestamp: chrono::Utc::now(),
            change_type: change_type.clone(),
        };
        
        if let Err(e) = change_sender.send(change_event) {
            error!("Failed to send file change event: {}", e);
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