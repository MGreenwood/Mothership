use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::collections::HashMap;


use crate::{CheckpointId, FileChange, ProjectId, RiftId, UserId};

/// WebSocket messages for real-time synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SyncMessage {
    // Client -> Server
    /// Client announces they're joining a rift for collaboration
    JoinRift {
        rift_id: RiftId,
        last_checkpoint: Option<CheckpointId>,
    },
    
    /// Client announces they're leaving a rift
    LeaveRift {
        rift_id: RiftId,
    },
    
    /// Client reports a file change
    FileChanged {
        rift_id: RiftId,
        path: PathBuf,
        content: String,
        timestamp: DateTime<Utc>,
    },
    
    /// Client reports multiple file changes (batch)
    FilesChanged {
        rift_id: RiftId,
        changes: Vec<FileChange>,
        timestamp: DateTime<Utc>,
    },
    
    /// Client requests checkpoint creation
    CreateCheckpoint {
        rift_id: RiftId,
        message: Option<String>,
    },
    
    /// Client requests full sync of a rift
    RequestSync {
        rift_id: RiftId,
        from_checkpoint: Option<CheckpointId>,
    },

    // Server -> Client
    /// Server broadcasts rift updates to all connected clients
    RiftUpdate {
        rift_id: RiftId,
        changes: Vec<FileChange>,
        author: UserId,
        timestamp: DateTime<Utc>,
    },
    
    /// Server notifies about checkpoint creation
    CheckpointCreated {
        rift_id: RiftId,
        checkpoint_id: CheckpointId,
        author: UserId,
        timestamp: DateTime<Utc>,
        message: Option<String>,
    },
    
    /// Server sends full sync data
    SyncData {
        rift_id: RiftId,
        checkpoint_id: CheckpointId,
        files: Vec<SyncFile>,
    },
    
    /// Server notifies about collaborator joining
    CollaboratorJoined {
        rift_id: RiftId,
        user_id: UserId,
        username: String,
    },
    
    /// Server notifies about collaborator leaving
    CollaboratorLeft {
        rift_id: RiftId,
        user_id: UserId,
    },
    
    /// Server reports conflict that needs resolution
    ConflictDetected {
        rift_id: RiftId,
        conflict: Conflict,
        suggestions: Vec<Resolution>,
    },

    // Bidirectional
    /// Heartbeat to keep connection alive
    Heartbeat,
    
    /// Generic error message
    Error {
        message: String,
        error_code: Option<String>,
    },
    
    /// Authentication challenge
    AuthChallenge {
        challenge: String,
    },
    
    /// Authentication response
    AuthResponse {
        token: String,
    },

    /// Server notifies about Rift joined
    RiftJoined {
        rift_id: RiftId,
        current_files: HashMap<PathBuf, String>,
        participants: Vec<String>,
        last_checkpoint: Option<CheckpointId>,
    },
}

/// File data for synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFile {
    pub path: PathBuf,
    pub content: String,
    pub hash: String,
    pub size: u64,
    pub modified_at: DateTime<Utc>,
}

/// Conflict information when multiple users edit the same file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub file_path: PathBuf,
    pub base_content: String,        // Common base version
    pub local_content: String,       // Local user's version
    pub remote_content: String,      // Remote user's version
    pub local_author: UserId,
    pub remote_author: UserId,
    pub timestamp: DateTime<Utc>,
}

/// Suggested resolution for conflicts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    pub strategy: ResolutionStrategy,
    pub confidence: f32,            // 0.0 to 1.0
    pub description: String,
    pub result_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResolutionStrategy {
    TakeLocal,                      // Use local version
    TakeRemote,                     // Use remote version
    Merge,                          // Automatic merge
    ManualMerge,                    // Requires user intervention
    SideBySide,                     // Present both versions
}

/// HTTP API messages (REST endpoints)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: None,
        }
    }
    
    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            message: None,
        }
    }
    
    pub fn message(message: String) -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            message: Some(message),
        }
    }
}

/// Gateway listing request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayRequest {
    pub include_inactive: bool,
}

/// Beam (project join) request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeamRequest {
    pub project_id: ProjectId,
    pub rift_name: Option<String>,   // If None, creates user's default rift
    pub force_sync: bool,            // Force full sync even if up to date
}

/// Beam response with project and rift information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeamResponse {
    pub project_id: ProjectId,
    pub rift_id: RiftId,
    pub websocket_url: String,       // WebSocket endpoint for real-time sync
    pub initial_sync_required: bool,
    pub checkpoint_count: u32,
} 