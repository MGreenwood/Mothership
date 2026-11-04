use anyhow::Result;
use mothership_common::ignore::IgnoreRules;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::collections::VecDeque;
use tokio::sync::mpsc as async_mpsc;
use tracing::{error, info, warn, debug};
use uuid::Uuid;

/// Files up to this size are read and sent in one message; larger files are streamed in chunks
pub(crate) const MAX_DIRECT_SYNC_SIZE: u64 = 1_048_576; // 1MB in bytes

/// Minimum debounce interval between file events (100ms)
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(100);

/// How long to wait before trusting an empty modified file (truncate-then-write race)
const EMPTY_WRITE_SETTLE: Duration = Duration::from_millis(250);

/// Retries when a file is still held open by its writer (Windows sharing violation, os error 32)
const READ_LOCK_RETRIES: u32 = 10;
const READ_LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

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

/// Debouncing state for file events (deduplicates identical content bursts from OS watchers)
struct FileDebouncer {
    last_content: HashMap<PathBuf, (String, Instant)>,
    // Flood detection: track recent distinct-content change times per file
    recent_events: HashMap<PathBuf, VecDeque<Instant>>,
}

impl FileDebouncer {
    fn new() -> Self {
        Self {
            last_content: HashMap::new(),
            recent_events: HashMap::new(),
        }
    }

    /// Returns true when this event has new content or enough time has passed since the same content was sent.
    fn should_process_content(&mut self, path: &PathBuf, content_hash: &str) -> bool {
        let now = Instant::now();
        if let Some((last_hash, last_time)) = self.last_content.get(path) {
            if last_hash == content_hash && now.duration_since(*last_time) < Duration::from_millis(500) {
                debug!("Skipping duplicate watcher event with identical content for {}", path.display());
                return false;
            }
        }
        self.last_content.insert(path.clone(), (content_hash.to_string(), now));
        true
    }

    /// Flood detection: returns true only if distinct content is changing abnormally fast (> 50 distinct saves in 5s)
    fn is_flooding(&mut self, path: &PathBuf) -> bool {
        const WINDOW: Duration = Duration::from_secs(5);
        const MAX_EVENTS: usize = 50;
        let now = Instant::now();
        let events = self.recent_events.entry(path.clone()).or_insert_with(VecDeque::new);
        events.push_back(now);
        while let Some(&front) = events.front() {
            if now.duration_since(front) > WINDOW {
                events.pop_front();
            } else {
                break;
            }
        }
        events.len() > MAX_EVENTS
    }

    /// Clean up old entries to prevent memory leak
    fn cleanup_old_entries(&mut self) {
        let now = Instant::now();
        let cutoff = Duration::from_secs(300); // 5 minutes
        
        self.last_content.retain(|_, (_, last_time)| {
            now.duration_since(*last_time) < cutoff
        });

        // Also clean up recent_events
        self.recent_events.retain(|_, events| {
            while let Some(&front) = events.front() {
                if now.duration_since(front) > cutoff {
                    events.pop_front();
                } else {
                    break;
                }
            }
            !events.is_empty()
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
        info!("Setting up file watcher for project {} at {}", 
            project_id, project_path.display());
        
        // Validate project path
        if !project_path.exists() {
            return Err(anyhow::anyhow!("Project path does not exist: {}", project_path.display()));
        }
        
        // Create sync channel for file system events
        let (fs_tx, fs_rx) = mpsc::channel();
        
        // Load .mothershipignore once per watcher; restart the watch to pick up edits
        let ignore = IgnoreRules::load(&project_path)?;
        
        // Create the file system watcher
        let mut watcher = RecommendedWatcher::new(fs_tx, Config::default())?;
        watcher.watch(&project_path, RecursiveMode::Recursive)?;
        
        // CRITICAL FIX: Create a sync channel bridge for async/sync boundary
        let (sync_tx, sync_rx) = mpsc::channel::<FileChangeEvent>();
        
        // Spawn async task to bridge sync -> async channels
        let async_sender = change_sender.clone();
        tokio::spawn(async move {
            info!("Starting async/sync bridge for file watcher");
            // Convert sync receiver to iterator and process events
            while let Ok(event) = sync_rx.recv() {
                debug!("Forwarding file change event through async bridge");
                if let Err(e) = async_sender.send(event) {
                    error!("Failed to forward file change event to daemon: {}", e);
                    break; // Channel closed, stop the bridge
                }
            }
            info!("Async/sync bridge stopped");
        });
        
        // Spawn background task to handle file system events
        let project_path_clone = project_path.clone();
        tokio::task::spawn_blocking(move || {
            info!("File watcher blocking task started for project {}", project_id);
            let mut debouncer = FileDebouncer::new();
            let mut cleanup_counter = 0;
            
            for res in fs_rx {
                match res {
                    Ok(event) => {
                        debug!("Received file system event: {:?}", event.kind);
                        if let Err(e) = handle_file_event(
                            &event, 
                            &project_path_clone, 
                            project_id, 
                            &sync_tx,  // Use sync channel here!
                            &mut debouncer,
                            &ignore,
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
            info!("File watcher stopped for project {}", project_id);
        });
        
        info!("File watcher started for project {} at {}", 
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
    ignore: &IgnoreRules,
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
        info!("Raw watcher event {:?}: {}", event.kind, path.display());

        // Skip directories
        if path.is_dir() {
            continue;
        }
        
        // Skip anything the project's .mothershipignore excludes
        if ignore.is_ignored(path, false) {
            continue;
        }
        
        // Calculate normalized relative path (forward slashes so Windows/Unix match)
        let relative_path = match path.strip_prefix(project_path) {
            Ok(rel_path) => PathBuf::from(mothership_common::normalize_rel_path(rel_path)),
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

        // Wait/retry if the file is momentarily missing during an editor atomic rename-save
        let mut meta_attempts = 0;
        let metadata = loop {
            match std::fs::metadata(path) {
                Ok(m) => break Some(m),
                Err(e) if meta_attempts < 5 && (e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(32)) => {
                    meta_attempts += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    debug!("Skipping file with unreadable metadata {}: {}", path.display(), e);
                    break None;
                }
            }
        };
        let Some(metadata) = metadata else {
            continue;
        };
        if metadata.is_dir() {
            continue;
        }
        let file_size = metadata.len();
        
        // Check file size and determine sync strategy
        if file_size > MAX_DIRECT_SYNC_SIZE {
            if !debouncer.should_process_content(&relative_path, &format!("stream:{file_size}")) {
                continue;
            }
            debug!("File {} ({} bytes) exceeds direct sync limit of {} bytes, will use streaming",
                path.display(), file_size, MAX_DIRECT_SYNC_SIZE);
            
            // For now, create event without content - streaming will be handled in daemon
            let change_event = FileChangeEvent {
                project_id,
                file_path: relative_path.clone(),
                content: String::new(), // Empty content for streaming files
                file_size,
                timestamp: chrono::Utc::now(),
                change_type: convert_event_kind_to_change_type(event.kind),
            };
            
            // Send event to be handled by streaming logic
            if let Err(e) = change_sender.send(change_event) {
                error!("Failed to send file change event: {}", e);
            }
            continue;
        }
        
        // Editors and shells often truncate before writing; wait briefly so an
        // empty read isn't sent as the file's final state
        let file_size = if file_size == 0 && matches!(change_type, FileChangeType::Modified | FileChangeType::Created | FileChangeType::Deleted) {
            std::thread::sleep(EMPTY_WRITE_SETTLE);
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        } else {
            file_size
        };

        // Read file content for direct sync (files <= MAX_DIRECT_SYNC_SIZE; larger files stream above)
        let mut attempts = 0;
        let content = loop {
            match std::fs::read_to_string(path) {
                Ok(content) => break Some(content),
                Err(e) if (e.raw_os_error() == Some(32) || e.kind() == std::io::ErrorKind::NotFound) && attempts < READ_LOCK_RETRIES => {
                    attempts += 1;
                    std::thread::sleep(READ_LOCK_RETRY_DELAY);
                }
                Err(e) => {
                    error!("Failed to read file content for {}: {}", path.display(), e);
                    break None;
                }
            }
        };
        let Some(content) = content else {
            continue;
        };

        let content_hash = mothership_common::content_hash(&content);
        if !debouncer.should_process_content(&relative_path, &content_hash) {
            continue;
        }
        if debouncer.is_flooding(&relative_path) {
            warn!("Suppressing file event flood for {} (too many changes in short time)", relative_path.display());
            continue;
        }
        
        info!("File changed in project {}: {} ({} bytes)", 
            project_id, relative_path.display(), content.len());
        
        let change_event = FileChangeEvent {
            project_id,
            file_path: relative_path,
            content,
            file_size,
            timestamp: chrono::Utc::now(),
            change_type: FileChangeType::Modified,
        };
        
        if let Err(e) = change_sender.send(change_event) {
            error!("Failed to send file change event: {}", e);
        }
    }
    
    Ok(())
}

/// Convert notify EventKind to our FileChangeType
fn convert_event_kind_to_change_type(kind: EventKind) -> FileChangeType {
    match kind {
        EventKind::Create(_) => FileChangeType::Created,
        EventKind::Modify(_) => FileChangeType::Modified,
        EventKind::Remove(_) => FileChangeType::Deleted,
        _ => FileChangeType::Modified, // Default fallback
    }
} 