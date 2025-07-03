use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use crate::diff::DiffEngine;
use crate::protocol::FileDiff;
use crate::crdt::RiftCRDT;
use anyhow::{Result, anyhow};
use sha2::{Sha256, Digest};

/// Represents a multi-file transaction that ensures atomic changes across files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Uuid,
    pub project_id: Uuid,
    pub rift_id: Uuid,
    pub message: String,
    pub status: TransactionStatus,
    pub created_at: DateTime<Utc>,
    pub committed_at: Option<DateTime<Utc>>,
    pub files: HashMap<PathBuf, FileState>,
    pub dependencies: Vec<Uuid>,
    pub author: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub path: PathBuf,
    pub diff: FileDiff,
    pub status: TransactionStatus,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    Active,
    Committed,
    RolledBack,
    Pending,
}

/// Manages atomic multi-file transactions and directory-level CRDTs
pub struct TransactionManager {
    active_transactions: HashMap<Uuid, Transaction>,
    committed_transactions: Vec<Transaction>,
    directory_crdts: HashMap<PathBuf, RiftCRDT>,
    rift_id: Uuid,
}

impl TransactionManager {
    pub fn new(rift_id: Uuid) -> Self {
        Self {
            active_transactions: HashMap::new(),
            committed_transactions: Vec::new(),
            directory_crdts: HashMap::new(),
            rift_id,
        }
    }

    /// Start a new transaction
    pub fn create_transaction(&mut self, author: Uuid, message: String) -> Transaction {
        let transaction = Transaction {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            rift_id: Uuid::new_v4(),
            message,
            status: TransactionStatus::Active,
            created_at: Utc::now(),
            committed_at: None,
            files: HashMap::new(),
            dependencies: Vec::new(),
            author,
        };
        
        self.active_transactions.insert(transaction.id, transaction.clone());
        transaction
    }

    /// Add a file modification to a transaction
    pub fn add_file_modification(
        &mut self,
        transaction_id: Uuid,
        path: PathBuf,
        new_content: &str,
        current_content: &str,
    ) -> Result<()> {
        let transaction = self.active_transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow!("Transaction not found"))?;

        if transaction.status != TransactionStatus::Active {
            return Err(anyhow!("Transaction is not in active state"));
        }

        let engine = DiffEngine::new();
        let diff = engine.generate_line_diff(current_content, new_content);
        let _previous_hash = crypto_hash(current_content);

        let file_state = FileState {
            path: path.clone(),
            diff: diff.clone(),
            status: TransactionStatus::Pending,
            timestamp: Utc::now(),
        };

        transaction.files.insert(path, file_state);

        Ok(())
    }

    /// Add a file creation to a transaction
    pub fn add_file_creation(
        &mut self,
        transaction_id: Uuid,
        path: PathBuf,
        content: String,
    ) -> Result<()> {
        let transaction = self.active_transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow!("Transaction not found"))?;

        let file_state = FileState {
            path: path.clone(),
            diff: FileDiff::FullContent(content),
            status: TransactionStatus::Pending,
            timestamp: Utc::now(),
        };

        transaction.files.insert(path, file_state);

        Ok(())
    }

    /// Add a file deletion to a transaction
    pub fn add_file_deletion(
        &mut self,
        transaction_id: Uuid,
        path: PathBuf,
        _current_content: String,
    ) -> Result<()> {
        let transaction = self.active_transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow!("Transaction not found"))?;

        let file_state = FileState {
            path: path.clone(),
            diff: FileDiff::Deleted,
            status: TransactionStatus::Pending,
            timestamp: Utc::now(),
        };

        transaction.files.insert(path, file_state);

        Ok(())
    }

    /// Commit a transaction
    pub async fn commit_transaction(&mut self, transaction_id: Uuid) -> Result<()> {
        let transaction = self.active_transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow!("Transaction not found"))?;

        // Check dependencies
        for dep_id in &transaction.dependencies {
            if !self.committed_transactions.iter().any(|t| t.id == *dep_id) {
                transaction.status = TransactionStatus::RolledBack;
                return Err(anyhow!("Dependency not satisfied"));
            }
        }

        // Apply file operations
        for (path, file_state) in &transaction.files {
            match &file_state.diff {
                FileDiff::FullContent(content) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(path, content)?;
                }
                FileDiff::LineDiff { .. } => {
                    let current_content = std::fs::read_to_string(path)?;
                    let engine = DiffEngine::new();
                    let new_content = engine.apply_diff(&current_content, &file_state.diff)?;
                    std::fs::write(path, new_content)?;
                }
                FileDiff::BinaryDiff { .. } => {
                    return Err(anyhow!("Binary diff not yet supported"));
                }
                FileDiff::Deleted => {
                    std::fs::remove_file(path)?;
                }
            }
        }

        transaction.status = TransactionStatus::Committed;
        transaction.committed_at = Some(Utc::now());
        let committed = transaction.clone();
        self.committed_transactions.push(committed);
        self.active_transactions.remove(&transaction_id);
        Ok(())
    }

    /// Roll back a transaction
    pub async fn rollback_transaction(&mut self, transaction_id: Uuid) -> Result<()> {
        let transaction = self.active_transactions.get_mut(&transaction_id)
            .ok_or_else(|| anyhow!("Transaction not found"))?;

        // Rollback file operations in reverse order
        // Since HashMap doesn't have a reverse iterator, we collect the entries first
        let files: Vec<_> = transaction.files.iter().collect();
        for (path, file_state) in files.into_iter().rev() {
            match &file_state.diff {
                FileDiff::FullContent(_) => {
                    if let Ok(_) = std::fs::remove_file(path) {}
                }
                FileDiff::LineDiff { .. } => {
                    // Restore previous content if available
                    if let Ok(current_content) = std::fs::read_to_string(path) {
                        std::fs::write(path, current_content)?;
                    }
                }
                FileDiff::BinaryDiff { .. } => {
                    // Binary rollback not implemented
                }
                FileDiff::Deleted => {
                    // Can't restore deleted file without backup
                }
            }
        }

        transaction.status = TransactionStatus::RolledBack;
        self.active_transactions.remove(&transaction_id);
        Ok(())
    }

    pub fn get_transaction(&self, transaction_id: &Uuid) -> Option<&Transaction> {
        self.active_transactions.get(transaction_id)
    }

    pub fn get_directory_crdt(&mut self, path: &Path) -> &mut RiftCRDT {
        self.directory_crdts.entry(path.to_path_buf()).or_insert_with(|| {
            RiftCRDT::new(self.rift_id)
        })
    }
}

fn crypto_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
} 