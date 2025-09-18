//! Streaming infrastructure for large file transfers
//! 
//! This module provides the **protocol definitions and data structures** for streaming.
//! Actual async implementations are in daemon and server crates with tokio support.

use crate::protocol::{StreamInfo, StreamStatus};
use crate::UserId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Default streaming configuration constants
pub const DEFAULT_CHUNK_SIZE: u32 = 65536; // 64KB chunks
pub const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 10;
pub const DEFAULT_STREAM_TIMEOUT_SECONDS: u64 = 300; // 5 minutes
pub const DEFAULT_STREAMING_THRESHOLD: u64 = 1_048_576; // 1MB threshold
pub const DEFAULT_COMPRESSION_THRESHOLD: u64 = 1024; // 1KB compression threshold

/// Configuration for streaming operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Size of each chunk in bytes
    pub chunk_size: u32,
    
    /// Maximum number of concurrent streams per user
    pub max_concurrent_streams: u32,
    
    /// Files larger than this size will use streaming
    pub streaming_threshold: u64,
    
    /// Chunks larger than this will be compressed
    pub compression_threshold: u64,
    
    /// Stream timeout in seconds
    pub timeout_seconds: u64,
    
    /// Directory for temporary files during streaming
    pub temp_dir: PathBuf,
    
    /// Enable compression for chunks
    pub compression_enabled: bool,
    
    /// Enable integrity checking with hashes
    pub hash_verification: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
            streaming_threshold: DEFAULT_STREAMING_THRESHOLD,
            compression_threshold: DEFAULT_COMPRESSION_THRESHOLD,
            timeout_seconds: DEFAULT_STREAM_TIMEOUT_SECONDS,
            temp_dir: PathBuf::from("temp/streams"),
            compression_enabled: true,
            hash_verification: true,
        }
    }
}

/// Result of adding a chunk to a stream
#[derive(Debug, Clone, PartialEq)]
pub enum StreamChunkResult {
    ChunkReceived,
    StreamComplete,
}

/// Statistics about streaming operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStats {
    pub total_active_streams: usize,
    pub streams_by_status: HashMap<StreamStatus, u32>,
    pub config: StreamConfig,
}

/// Metadata for tracking stream progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamProgress {
    pub stream_id: Uuid,
    pub chunks_received: u32,
    pub total_chunks: u32,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub transfer_rate_bytes_per_sec: f64,
    pub eta_seconds: Option<u64>,
}

impl StreamProgress {
    pub fn completion_percentage(&self) -> f64 {
        if self.total_chunks == 0 {
            return 0.0;
        }
        (self.chunks_received as f64 / self.total_chunks as f64) * 100.0
    }
    
    pub fn is_complete(&self) -> bool {
        self.chunks_received >= self.total_chunks
    }
}

/// Calculate hash of chunk data for integrity verification
pub fn calculate_chunk_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Calculate hash of complete file data
pub fn calculate_file_hash(data: &[u8]) -> String {
    calculate_chunk_hash(data) // Same implementation for now
}

/// Check if chunk should be compressed based on size and config
pub fn should_compress_chunk(data: &[u8], config: &StreamConfig) -> bool {
    config.compression_enabled && data.len() as u64 >= config.compression_threshold
}

/// Compress chunk data if beneficial
pub fn compress_chunk_if_needed(data: &[u8], config: &StreamConfig) -> Result<(Vec<u8>, bool), Box<dyn std::error::Error>> {
    if should_compress_chunk(data, config) {
        // Use existing compression from diff.rs
        crate::diff::CompressionEngine::compress(data)
            .map(|compressed| (compressed, true))
            .map_err(|e| e.into())
    } else {
        Ok((data.to_vec(), false))
    }
}

/// Decompress chunk data if compressed
pub fn decompress_chunk_if_needed(data: &[u8], is_compressed: bool) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if is_compressed {
        crate::diff::CompressionEngine::decompress(data)
            .map_err(|e| e.into())
    } else {
        Ok(data.to_vec())
    }
}
