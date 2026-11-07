use anyhow::Result;
use chrono::Utc;
use mothership_common::{select_paths, Checkpoint, CheckpointId, FileChange, ChangeType, RiftId, UserId};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::RwLock;
use uuid::Uuid;

/// A checkpoint request the rift cannot satisfy. Handlers report it as 400, not 500.
#[derive(Debug)]
pub struct InvalidCheckpoint(pub String);

impl std::fmt::Display for InvalidCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidCheckpoint {}

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

        // Load existing checkpoints so parent links and history work after a restart
        let mut index = HashMap::new();
        let mut entries = fs::read_dir(storage_root.join("checkpoints")).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path).await.map_err(anyhow::Error::from).and_then(|json| {
                serde_json::from_str::<Checkpoint>(&json).map_err(anyhow::Error::from)
            }) {
                Ok(checkpoint) => {
                    index.insert(checkpoint.id, checkpoint);
                }
                Err(e) => tracing::warn!("Skipping unreadable checkpoint {}: {}", path.display(), e),
            }
        }
        
        Ok(Self {
            storage_root,
            checkpoint_index: RwLock::new(index),
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

    /// Directory holding a rift's persisted working files
    fn live_rift_dir(&self, rift_id: RiftId) -> PathBuf {
        self.storage_root.join("live").join(rift_id.to_string())
    }

    /// Reject paths that could escape the rift directory (absolute, `..`, prefixes, `.mothership`)
    /// and normalize both `/` and `\` separators across platforms.
    fn safe_relative_path(path: &Path) -> Result<PathBuf> {
        mothership_common::beam::safe_relative_path(path)
            .ok_or_else(|| anyhow::anyhow!("Invalid file path: {}", path.display()))
    }

    /// Load a rift's working files from disk into memory if not already cached
    async fn ensure_live_loaded(&self, rift_id: RiftId) -> Result<()> {
        if self.live_state.read().await.contains_key(&rift_id) {
            return Ok(());
        }

        let root = self.live_rift_dir(rift_id);
        let mut files = HashMap::new();
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            if !dir.exists() {
                break;
            }
            let mut entries = fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let entry_path = entry.path();
                if entry.file_type().await?.is_dir() {
                    pending.push(entry_path);
                } else if let Ok(relative) = entry_path.strip_prefix(&root) {
                    let Ok(clean_rel) = Self::safe_relative_path(relative) else {
                        continue;
                    };
                    let key = PathBuf::from(mothership_common::normalize_rel_path(&clean_rel));
                    match fs::read_to_string(&entry_path).await {
                        Ok(content) => {
                            files.insert(key, content);
                        }
                        Err(e) => tracing::warn!("Skipping unreadable live file {}: {}", entry_path.display(), e),
                    }
                }
            }
        }

        self.live_state.write().await.entry(rift_id).or_insert(files);
        Ok(())
    }

    /// Update live working state for a rift, persisting it to disk
    pub async fn update_live_state(&self, rift_id: RiftId, path: PathBuf, content: String) -> Result<()> {
        self.ensure_live_loaded(rift_id).await?;

        let relative = Self::safe_relative_path(&path)?;
        let file_path = self.live_rift_dir(rift_id).join(&relative);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content.as_bytes()).await?;

        let key = PathBuf::from(mothership_common::normalize_rel_path(&relative));
        let mut live_state = self.live_state.write().await;
        live_state.entry(rift_id).or_insert_with(HashMap::new).insert(key, content);
        Ok(())
    }

    /// Get current live state for a rift
    pub async fn get_live_state(&self, rift_id: RiftId) -> Result<HashMap<PathBuf, String>> {
        self.ensure_live_loaded(rift_id).await?;
        let live_state = self.live_state.read().await;
        Ok(live_state.get(&rift_id).cloned().unwrap_or_default())
    }

    /// PERFORMANCE FIX: Get content for a specific file in a rift
    pub async fn get_file_content(&self, rift_id: RiftId, path: &PathBuf) -> Result<String> {
        self.ensure_live_loaded(rift_id).await?;
        let relative = Self::safe_relative_path(path)?;
        let key = PathBuf::from(mothership_common::normalize_rel_path(&relative));
        let live_state = self.live_state.read().await;
        if let Some(rift_files) = live_state.get(&rift_id) {
            if let Some(content) = rift_files.get(&key).or_else(|| rift_files.get(path)) {
                return Ok(content.clone());
            }
        }
        
        // File not found in live state
        Err(anyhow::anyhow!("File not found in rift {}: {}", rift_id, path.display()))
    }

    /// Create a checkpoint from the rift's live state. Without `paths` it records the whole rift.
    /// With `paths` it records only the selected files, and the rest of the state comes from the
    /// rift's latest checkpoint, so a partial checkpoint needs an earlier one to build on.
    pub async fn create_checkpoint(
        &self,
        rift_id: RiftId,
        author: UserId,
        message: Option<String>,
        auto_generated: bool,
        paths: Option<&[String]>,
    ) -> Result<Checkpoint> {
        let checkpoint_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let parent = self.latest_checkpoint_id(rift_id).await;

        let live_files = self.get_live_state(rift_id).await?;
        let (files, partial) = match paths {
            Some(selectors) => {
                if parent.is_none() {
                    return Err(InvalidCheckpoint(
                        "this rift has no checkpoint yet; make a whole-rift checkpoint first".to_string(),
                    )
                    .into());
                }
                let selected = select_paths(&live_files, selectors)
                    .map_err(InvalidCheckpoint)?;
                (selected, true)
            }
            None => (live_files, false),
        };
        
        // TODO: For now, treat all files as new/modified
        // In production, this would diff against parent checkpoint
        let mut changes = Vec::with_capacity(files.len());
        
        for (path, content) in files {
            let content_hash = self.store_content(&content).await?;
            let size = content.len() as u64;
            
            changes.push(FileChange {
                path,
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
            parent,
            message,
            auto_generated,
            partial,
            verification: None,
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

    /// Attach a check result to a checkpoint and save it. Returns false when the checkpoint is unknown.
    pub async fn set_checkpoint_verification(
        &self,
        checkpoint_id: CheckpointId,
        verification: mothership_common::verify::VerificationResult,
    ) -> Result<bool> {
        let Some(mut checkpoint) = self.load_checkpoint(checkpoint_id).await? else {
            return Ok(false);
        };
        checkpoint.verification = Some(verification);
        self.store_checkpoint(&checkpoint).await?;
        self.checkpoint_index.write().await.insert(checkpoint_id, checkpoint);
        Ok(true)
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

    /// The newest checkpoint recorded for a rift, used as the parent of the next one
    async fn latest_checkpoint_id(&self, rift_id: RiftId) -> Option<CheckpointId> {
        let index = self.checkpoint_index.read().await;
        index
            .values()
            .filter(|cp| cp.rift_id == rift_id)
            .max_by_key(|cp| cp.timestamp)
            .map(|cp| cp.id)
    }

    /// Whether a partial checkpoint is built on this one. Deleting it would leave that one unrestorable.
    pub async fn has_partial_dependents(&self, checkpoint_id: CheckpointId) -> bool {
        let index = self.checkpoint_index.read().await;
        index
            .values()
            .any(|cp| cp.partial && cp.parent == Some(checkpoint_id))
    }

    /// The complete file set at a checkpoint. It starts from the nearest full checkpoint at or
    /// before it and applies each later partial checkpoint in order. Full checkpoints and older
    /// checkpoints hold the whole rift, so they need no earlier history.
    pub async fn get_checkpoint_files(&self, checkpoint_id: CheckpointId) -> Result<HashMap<PathBuf, String>> {
        let mut chain = Vec::new();
        let mut next = Some(checkpoint_id);
        while let Some(id) = next {
            let checkpoint = self
                .load_checkpoint(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("checkpoint history is incomplete: {} is missing", id))?;
            next = if checkpoint.partial { checkpoint.parent } else { None };
            chain.push(checkpoint);
        }

        if chain.last().is_some_and(|base| base.partial) {
            anyhow::bail!("checkpoint history is incomplete: no full checkpoint before {}", checkpoint_id);
        }

        let mut files = HashMap::new();
        for checkpoint in chain.iter().rev() {
            for change in &checkpoint.changes {
                match change.change_type {
                    ChangeType::Deleted => {
                        files.remove(&change.path);
                    }
                    _ => {
                        if let Some(content) = self.get_content(&change.content_hash).await? {
                            files.insert(change.path.clone(), content);
                        }
                    }
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

    /// Delete a checkpoint's metadata. Content blobs are shared by content hash, so they are kept.
    pub async fn delete_checkpoint(&self, checkpoint_id: CheckpointId) -> Result<bool> {
        {
            let mut index = self.checkpoint_index.write().await;
            index.remove(&checkpoint_id);
        }

        let checkpoint_path = self.storage_root
            .join("checkpoints")
            .join(format!("{}.json", checkpoint_id));

        match fs::remove_file(&checkpoint_path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
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

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("mothership-checkpoint-{}", Uuid::new_v4()))
    }

    async fn live(engine: &StorageEngine, rift: RiftId, files: &[(&str, &str)]) {
        for (path, content) in files {
            engine
                .update_live_state(rift, PathBuf::from(path), content.to_string())
                .await
                .unwrap();
        }
    }

    fn selectors(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn whole_rift_checkpoint_records_everything_and_is_not_partial() {
        let root = temp_root();
        let engine = StorageEngine::new(root.clone()).await.unwrap();
        let rift = Uuid::new_v4();
        live(&engine, rift, &[("README.md", "readme"), ("src/main.rs", "main")]).await;

        let cp = engine.create_checkpoint(rift, Uuid::new_v4(), None, false, None).await.unwrap();
        assert!(!cp.partial);
        assert_eq!(cp.changes.len(), 2);
        assert_eq!(engine.get_checkpoint_files(cp.id).await.unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn partial_checkpoint_keeps_unselected_files_from_its_parent() {
        let root = temp_root();
        let engine = StorageEngine::new(root.clone()).await.unwrap();
        let rift = Uuid::new_v4();
        live(&engine, rift, &[("README.md", "v1"), ("src/main.rs", "v1")]).await;
        let base = engine.create_checkpoint(rift, Uuid::new_v4(), None, false, None).await.unwrap();

        live(&engine, rift, &[("README.md", "v2"), ("src/main.rs", "v2")]).await;
        let partial = engine
            .create_checkpoint(rift, Uuid::new_v4(), None, false, Some(&selectors(&["src/"])))
            .await
            .unwrap();

        assert!(partial.partial);
        assert_eq!(partial.parent, Some(base.id));
        assert_eq!(partial.changes.len(), 1, "only the selected folder is recorded");

        // At the partial checkpoint, src/ is new and README.md is still the base version
        let files = engine.get_checkpoint_files(partial.id).await.unwrap();
        assert_eq!(files.get(&PathBuf::from("src/main.rs")).map(String::as_str), Some("v2"));
        assert_eq!(files.get(&PathBuf::from("README.md")).map(String::as_str), Some("v1"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn partial_checkpoint_needs_an_earlier_checkpoint() {
        let root = temp_root();
        let engine = StorageEngine::new(root.clone()).await.unwrap();
        let rift = Uuid::new_v4();
        live(&engine, rift, &[("README.md", "v1")]).await;

        let err = engine
            .create_checkpoint(rift, Uuid::new_v4(), None, false, Some(&selectors(&["README.md"])))
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<InvalidCheckpoint>().is_some());

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn unmatched_selector_is_rejected_and_nothing_is_recorded() {
        let root = temp_root();
        let engine = StorageEngine::new(root.clone()).await.unwrap();
        let rift = Uuid::new_v4();
        live(&engine, rift, &[("README.md", "v1")]).await;
        engine.create_checkpoint(rift, Uuid::new_v4(), None, false, None).await.unwrap();

        let err = engine
            .create_checkpoint(rift, Uuid::new_v4(), None, false, Some(&selectors(&["nope.rs"])))
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<InvalidCheckpoint>().is_some());
        assert!(err.to_string().contains("nope.rs"));
        assert_eq!(engine.list_checkpoints(rift).await.unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn history_survives_restart_so_parents_and_restores_still_work() {
        let root = temp_root();
        let rift = Uuid::new_v4();
        let (base_id, partial_id) = {
            let engine = StorageEngine::new(root.clone()).await.unwrap();
            live(&engine, rift, &[("README.md", "v1"), ("src/main.rs", "v1")]).await;
            let base = engine.create_checkpoint(rift, Uuid::new_v4(), None, false, None).await.unwrap();
            live(&engine, rift, &[("src/main.rs", "v2")]).await;
            let partial = engine
                .create_checkpoint(rift, Uuid::new_v4(), None, false, Some(&selectors(&["src/main.rs"])))
                .await
                .unwrap();
            (base.id, partial.id)
        };

        let restarted = StorageEngine::new(root.clone()).await.unwrap();
        let partial = restarted.load_checkpoint(partial_id).await.unwrap().unwrap();
        assert_eq!(partial.parent, Some(base_id));
        let files = restarted.get_checkpoint_files(partial_id).await.unwrap();
        assert_eq!(files.get(&PathBuf::from("src/main.rs")).map(String::as_str), Some("v2"));
        assert_eq!(files.get(&PathBuf::from("README.md")).map(String::as_str), Some("v1"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn checkpoint_with_a_partial_child_reports_dependents() {
        let root = temp_root();
        let engine = StorageEngine::new(root.clone()).await.unwrap();
        let rift = Uuid::new_v4();
        live(&engine, rift, &[("README.md", "v1"), ("src/main.rs", "v1")]).await;
        let base = engine.create_checkpoint(rift, Uuid::new_v4(), None, false, None).await.unwrap();
        assert!(!engine.has_partial_dependents(base.id).await);

        engine
            .create_checkpoint(rift, Uuid::new_v4(), None, false, Some(&selectors(&["src/"])))
            .await
            .unwrap();
        assert!(engine.has_partial_dependents(base.id).await);

        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[tokio::test]
    async fn live_state_survives_restart() {
        let root = std::env::temp_dir().join(format!("mothership-live-{}", Uuid::new_v4()));
        let rift_id = Uuid::new_v4();

        let engine = StorageEngine::new(root.clone()).await.unwrap();
        engine.update_live_state(rift_id, PathBuf::from("src/main.rs"), "fn main() {}".to_string()).await.unwrap();
        engine.update_live_state(rift_id, PathBuf::from("README.md"), "hello".to_string()).await.unwrap();
        drop(engine);

        let restarted = StorageEngine::new(root.clone()).await.unwrap();
        let files = restarted.get_live_state(rift_id).await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files.get(&PathBuf::from("src/main.rs")).map(String::as_str), Some("fn main() {}"));
        assert_eq!(restarted.get_file_content(rift_id, &PathBuf::from("README.md")).await.unwrap(), "hello");
        assert!(StorageEngine::safe_relative_path(Path::new("../escape.txt")).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
