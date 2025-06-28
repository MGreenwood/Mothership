//! PERFORMANCE FIX: Efficient diff generation and application
//! 
//! This module eliminates the broadcast content spam by implementing:
//! - Line-based diff generation (like git)
//! - Efficient diff application 
//! - Smart diff size optimization
//! - Compression integration

use crate::protocol::{FileDiff, DiffOperation, FileDiffChange};
use anyhow::Result;
use std::path::PathBuf;

/// PERFORMANCE FIX: Diff engine for minimal network usage
pub struct DiffEngine;

impl DiffEngine {
    /// Generate diff between two strings (line-based for code files)
    pub fn generate_line_diff(original: &str, new: &str) -> FileDiff {
        let original_lines: Vec<&str> = original.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();
        
        // Quick check: if new content is small or diff would be large, send full content
        if new.len() < 1024 || Self::should_use_full_content(&original_lines, &new_lines) {
            return FileDiff::FullContent(new.to_string());
        }
        
        let operations = Self::compute_diff_operations(&original_lines, &new_lines);
        
        FileDiff::LineDiff {
            operations,
            original_lines: original_lines.len() as u32,
            new_lines: new_lines.len() as u32,
        }
    }
    
    /// Apply diff to get new content
    pub fn apply_diff(original: &str, diff: &FileDiff) -> Result<String> {
        match diff {
            FileDiff::FullContent(content) => Ok(content.clone()),
            
            FileDiff::LineDiff { operations, .. } => {
                let original_lines: Vec<&str> = original.lines().collect();
                let mut result_lines = Vec::new();
                let mut original_pos = 0;
                
                for operation in operations {
                    match operation {
                        DiffOperation::Keep { count } => {
                            let end_pos = original_pos + *count as usize;
                            result_lines.extend_from_slice(&original_lines[original_pos..end_pos.min(original_lines.len())]);
                            original_pos = end_pos;
                        }
                        
                        DiffOperation::Delete { count } => {
                            original_pos += *count as usize;
                        }
                        
                        DiffOperation::Insert { lines } => {
                            result_lines.extend(lines.iter().map(|s| s.as_str()));
                        }
                        
                        DiffOperation::Replace { delete_count, insert_lines } => {
                            original_pos += *delete_count as usize;
                            result_lines.extend(insert_lines.iter().map(|s| s.as_str()));
                        }
                    }
                }
                
                Ok(result_lines.join("\n"))
            }
            
            FileDiff::Deleted => Ok(String::new()),
            
            FileDiff::BinaryDiff { .. } => {
                // TODO: Implement binary diff application
                Err(anyhow::anyhow!("Binary diff not yet implemented"))
            }
        }
    }
    
    /// Create a diff change event
    pub fn create_diff_change(path: PathBuf, original: &str, new: &str) -> FileDiffChange {
        let diff = Self::generate_line_diff(original, new);
        FileDiffChange {
            path,
            diff,
            file_size: new.len() as u64,
        }
    }
    
    /// Check if we should use full content instead of diff
    fn should_use_full_content(original_lines: &[&str], new_lines: &[&str]) -> bool {
        // If too many changes, full content might be smaller
        let changes = Self::count_line_changes(original_lines, new_lines);
        let original_len = original_lines.len();
        let new_len = new_lines.len();
        
        // Use full content if >70% of lines changed
        if original_len > 0 && changes as f32 / original_len as f32 > 0.7 {
            return true;
        }
        
        // Use full content if new file is very small
        if new_len < 10 {
            return true;
        }
        
        false
    }
    
    /// Simple diff algorithm (optimized for typical code editing patterns)
    fn compute_diff_operations(original: &[&str], new: &[&str]) -> Vec<DiffOperation> {
        let mut operations = Vec::new();
        let mut orig_pos = 0;
        let mut new_pos = 0;
        
        while orig_pos < original.len() || new_pos < new.len() {
            // Find common prefix
            let common_start = Self::find_common_prefix(&original[orig_pos..], &new[new_pos..]);
            
            if common_start > 0 {
                operations.push(DiffOperation::Keep { count: common_start as u32 });
                orig_pos += common_start;
                new_pos += common_start;
                continue;
            }
            
            // Find next matching section
            let (orig_skip, new_skip, _next_common) = Self::find_next_match(
                &original[orig_pos..], 
                &new[new_pos..]
            );
            
            if orig_skip > 0 && new_skip > 0 {
                // Replace operation
                operations.push(DiffOperation::Replace {
                    delete_count: orig_skip as u32,
                    insert_lines: new[new_pos..new_pos + new_skip].iter().map(|s| s.to_string()).collect(),
                });
            } else if orig_skip > 0 {
                // Delete operation
                operations.push(DiffOperation::Delete { count: orig_skip as u32 });
            } else if new_skip > 0 {
                // Insert operation
                operations.push(DiffOperation::Insert {
                    lines: new[new_pos..new_pos + new_skip].iter().map(|s| s.to_string()).collect(),
                });
            } else {
                // No more matches, handle remaining lines
                if orig_pos < original.len() {
                    operations.push(DiffOperation::Delete { 
                        count: (original.len() - orig_pos) as u32 
                    });
                }
                if new_pos < new.len() {
                    operations.push(DiffOperation::Insert {
                        lines: new[new_pos..].iter().map(|s| s.to_string()).collect(),
                    });
                }
                break;
            }
            
            orig_pos += orig_skip;
            new_pos += new_skip;
        }
        
        operations
    }
    
    /// Find common prefix between two slices
    fn find_common_prefix(a: &[&str], b: &[&str]) -> usize {
        let mut count = 0;
        let max_len = a.len().min(b.len());
        
        while count < max_len && a[count] == b[count] {
            count += 1;
        }
        
        count
    }
    
    /// Find next matching section in the sequences
    fn find_next_match(original: &[&str], new: &[&str]) -> (usize, usize, usize) {
        // Simple strategy: look for first common line in reasonable distance
        for orig_offset in 0..original.len().min(10) {
            for new_offset in 0..new.len().min(10) {
                if orig_offset < original.len() && new_offset < new.len() {
                    if original[orig_offset] == new[new_offset] {
                        let common_len = Self::find_common_prefix(
                            &original[orig_offset..],
                            &new[new_offset..]
                        );
                        if common_len > 0 {
                            return (orig_offset, new_offset, common_len);
                        }
                    }
                }
            }
        }
        
        // No match found, consume all remaining
        (original.len(), new.len(), 0)
    }
    
    /// Count number of changed lines (for optimization decisions)
    fn count_line_changes(original: &[&str], new: &[&str]) -> usize {
        use std::collections::HashSet;
        
        let original_set: HashSet<&str> = original.iter().cloned().collect();
        let new_set: HashSet<&str> = new.iter().cloned().collect();
        
        // Lines added + lines removed
        let added = new_set.difference(&original_set).count();
        let removed = original_set.difference(&new_set).count();
        
        added + removed
    }
}

/// PERFORMANCE FIX: Compression utilities
pub struct CompressionEngine;

impl CompressionEngine {
    /// Compress data using fast LZ4-style compression
    pub fn compress(data: &[u8]) -> Result<Vec<u8>> {
        // For now, use simple gzip compression
        // TODO: Switch to LZ4 for better speed/ratio balance
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }
    
    /// Decompress data
    pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>> {
        use flate2::read::GzDecoder;
        use std::io::Read;
        
        let mut decoder = GzDecoder::new(compressed);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }
    
    /// Check if compression would be beneficial
    pub fn should_compress(data: &[u8]) -> bool {
        // Only compress if data is larger than 1KB
        data.len() > 1024
    }
}

/// PERFORMANCE FIX: Batch operations for reducing message overhead
pub struct BatchEngine;

impl BatchEngine {
    /// Combine multiple diff changes into a batch
    pub fn create_batch(changes: Vec<FileDiffChange>, should_compress: bool) -> Result<Vec<u8>> {
        let serialized = serde_json::to_vec(&changes)?;
        
        if should_compress && CompressionEngine::should_compress(&serialized) {
            CompressionEngine::compress(&serialized)
        } else {
            Ok(serialized)
        }
    }
    
    /// Extract diff changes from batch
    pub fn extract_batch(data: &[u8], is_compressed: bool) -> Result<Vec<FileDiffChange>> {
        let decompressed = if is_compressed {
            CompressionEngine::decompress(data)?
        } else {
            data.to_vec()
        };
        
        Ok(serde_json::from_slice(&decompressed)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_line_diff() {
        let original = "line 1\nline 2\nline 3";
        let new = "line 1\nmodified line 2\nline 3";
        
        let diff = DiffEngine::generate_line_diff(original, new);
        let applied = DiffEngine::apply_diff(original, &diff).unwrap();
        
        assert_eq!(applied, new);
    }
    
    #[test]
    fn test_compression() {
        let data = b"This is a test string that should compress well when repeated. ".repeat(100);
        let compressed = CompressionEngine::compress(&data).unwrap();
        let decompressed = CompressionEngine::decompress(&compressed).unwrap();
        
        assert_eq!(data, decompressed);
        assert!(compressed.len() < data.len());
    }
} 