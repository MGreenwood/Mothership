use anyhow::Result;
use chrono::Utc;
use mothership_common::{Checkpoint, CheckpointId, FileChange, ChangeType, RiftId, UserId};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Content-Addressable Storage + Checkpoint Management
pub struct StorageEngine {
    /// Base directory for all storage
    storage_root: PathBuf,
    /// In-memory checkpoint index for fast lookups
    checkpoint_index: RwLock<HashMap<CheckpointId, Checkpoint>>,
    /// In-memory rift state (current working files)
    live_state: RwLock<HashMap<RiftId, HashMap<PathBuf, String>>>,
}

impl StorageEngine {
    pub async fn new(storage_root: PathBuf) -> Result<Self> {
        // Create directory structure
        fs::create_dir_all(&storage_root).await?;
        fs::create_dir_all(storage_root.join("content")).await?;  // CAS storage
        fs::create_dir_all(storage_root.join("checkpoints")).await?;  // Checkpoint metadata
        fs::create_dir_all(storage_root.join("live")).await?;  // Working state
        
        Ok(Self {
            storage_root,
            checkpoint_index: RwLock::new(HashMap::new()),
            live_state: RwLock::new(HashMap::new()),
        })
    }

    /// Store file content using content-addressable storage
    /// Returns the content hash
    pub async fn store_content(&self, content: &str) -> Result<String> {
        // Calculate SHA-256 hash
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        
        let content_path = self.storage_root.join("content").join(&hash);
        
        // Only write if file doesn't exist (deduplication)
        if !content_path.exists() {
            fs::write(&content_path, content).await?;
        }
        
        Ok(hash)
    }

    /// Retrieve file content by hash
    pub async fn get_content(&self, hash: &str) -> Result<Option<String>> {
        let content_path = self.storage_root.join("content").join(hash);
        
        if content_path.exists() {
            let content = fs::read_to_string(&content_path).await?;
            Ok(Some(content))
        } else {
            Ok(None)
        }
    }

    /// Update live working state for a rift
    pub async fn update_live_state(&self, rift_id: RiftId, path: PathBuf, content: String) -> Result<()> {
        let mut live_state = self.live_state.write().await;
        
        let rift_files = live_state.entry(rift_id).or_insert_with(HashMap::new);
        rift_files.insert(path, content);
        
        Ok(())
    }

    /// Get current live state for a rift
    pub async fn get_live_state(&self, rift_id: RiftId) -> Result<HashMap<PathBuf, String>> {
        let live_state = self.live_state.read().await;
        Ok(live_state.get(&rift_id).cloned().unwrap_or_default())
    }

    /// PERFORMANCE FIX: Get content for a specific file in a rift
    pub async fn get_file_content(&self, rift_id: RiftId, path: &PathBuf) -> Result<String> {
        let live_state = self.live_state.read().await;
        if let Some(rift_files) = live_state.get(&rift_id) {
            if let Some(content) = rift_files.get(path) {
                return Ok(content.clone());
            }
        }
        
        // File not found in live state
        Err(anyhow::anyhow!("File not found in rift {}: {}", rift_id, path.display()))
    }

    /// Create a new checkpoint from current live state
    pub async fn create_checkpoint(
        &self,
        rift_id: RiftId,
        author: UserId,
        message: Option<String>,
        auto_generated: bool,
    ) -> Result<Checkpoint> {
        let checkpoint_id = Uuid::new_v4();
        let timestamp = Utc::now();
        
        // Get current live state
        let live_files = self.get_live_state(rift_id).await?;
        
        // TODO: For now, treat all files as new/modified
        // In production, this would diff against parent checkpoint
        let mut changes = Vec::new();
        
        for (path, content) in live_files {
            let content_hash = self.store_content(&content).await?;
            let size = content.len() as u64;
            
            changes.push(FileChange {
                path: path.clone(),
                change_type: ChangeType::Modified, // Simplified for now
                content_hash,
                diff: None, // TODO: Generate diff
                size,
            });
        }
        
        let checkpoint = Checkpoint {
            id: checkpoint_id,
            rift_id,
            author,
            timestamp,
            changes,
            parent: None, // TODO: Link to parent checkpoint
            message,
            auto_generated,
        };
        
        // Store checkpoint metadata
        self.store_checkpoint(&checkpoint).await?;
        
        // Update in-memory index
        {
            let mut index = self.checkpoint_index.write().await;
            index.insert(checkpoint_id, checkpoint.clone());
        }
        
        Ok(checkpoint)
    }

    /// Store checkpoint metadata to disk
    async fn store_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        let checkpoint_path = self.storage_root
            .join("checkpoints")
            .join(format!("{}.json", checkpoint.id));
        
        let json = serde_json::to_string_pretty(checkpoint)?;
        fs::write(&checkpoint_path, json).await?;
        
        Ok(())
    }

    /// Load checkpoint from disk
    pub async fn load_checkpoint(&self, checkpoint_id: CheckpointId) -> Result<Option<Checkpoint>> {
        // Check memory first
        {
            let index = self.checkpoint_index.read().await;
            if let Some(checkpoint) = index.get(&checkpoint_id) {
                return Ok(Some(checkpoint.clone()));
            }
        }
        
        // Load from disk
        let checkpoint_path = self.storage_root
            .join("checkpoints")
            .join(format!("{}.json", checkpoint_id));
        
        if checkpoint_path.exists() {
            let json = fs::read_to_string(&checkpoint_path).await?;
            let checkpoint: Checkpoint = serde_json::from_str(&json)?;
            
            // Cache in memory
            {
                let mut index = self.checkpoint_index.write().await;
                index.insert(checkpoint_id, checkpoint.clone());
            }
            
            Ok(Some(checkpoint))
        } else {
            Ok(None)
        }
    }

    /// Get all files at a specific checkpoint
    pub async fn get_checkpoint_files(&self, checkpoint_id: CheckpointId) -> Result<HashMap<PathBuf, String>> {
        let mut files = HashMap::new();
        
        if let Some(checkpoint) = self.load_checkpoint(checkpoint_id).await? {
            for change in checkpoint.changes {
                if let Some(content) = self.get_content(&change.content_hash).await? {
                    files.insert(change.path, content);
                }
            }
        }
        
        Ok(files)
    }

    /// List checkpoints for a rift
    pub async fn list_checkpoints(&self, rift_id: RiftId) -> Result<Vec<Checkpoint>> {
        let index = self.checkpoint_index.read().await;
        let checkpoints: Vec<Checkpoint> = index
            .values()
            .filter(|cp| cp.rift_id == rift_id)
            .cloned()
            .collect();
        
        Ok(checkpoints)
    }

    /// Calculate storage statistics
    pub async fn get_stats(&self) -> Result<StorageStats> {
        let content_dir = self.storage_root.join("content");
        let checkpoint_dir = self.storage_root.join("checkpoints");
        
        let content_files = self.count_files(&content_dir).await?;
        let checkpoint_files = self.count_files(&checkpoint_dir).await?;
        let total_size = self.calculate_dir_size(&self.storage_root).await?;
        
        Ok(StorageStats {
            content_files,
            checkpoint_files,
            total_size_bytes: total_size,
            live_rifts: self.live_state.read().await.len(),
        })
    }

    async fn count_files(&self, dir: &Path) -> Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }
        
        let mut count = 0;
        let mut entries = fs::read_dir(dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_file() {
                count += 1;
            }
        }
        
        Ok(count)
    }

    async fn calculate_dir_size(&self, dir: &Path) -> Result<u64> {
        if !dir.exists() {
            return Ok(0);
        }
        
        let mut total_size = 0;
        let mut entries = fs::read_dir(dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            if metadata.is_file() {
                total_size += metadata.len();
            } else if metadata.is_dir() {
                total_size += Box::pin(self.calculate_dir_size(&entry.path())).await?;
            }
        }
        
        Ok(total_size)
    }
}

#[derive(Debug, Clone)]
pub struct StorageStats {
    pub content_files: usize,
    pub checkpoint_files: usize,
    pub total_size_bytes: u64,
    pub live_rifts: usize,
}

impl StorageStats {
    pub fn total_size_mb(&self) -> f64 {
        self.total_size_bytes as f64 / (1024.0 * 1024.0)
    }
} 