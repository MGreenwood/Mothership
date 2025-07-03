//! PERFORMANCE FIX: Efficient diff generation and application
//! 
//! This module eliminates the broadcast content spam by implementing:
//! - Line-based diff generation (like git)
//! - Efficient diff application 
//! - Smart diff size optimization
//! - Compression integration

use crate::protocol::{FileDiff, DiffOperation, FileDiffChange};
use anyhow::Result;

/// PERFORMANCE FIX: Diff engine for minimal network usage
pub struct DiffEngine;

impl DiffEngine {
    pub fn new() -> Self {
        DiffEngine
    }

    pub fn generate_line_diff(&self, original: &str, new: &str) -> FileDiff {
        let original_lines: Vec<String> = original.lines().map(|s| s.to_string()).collect();
        let new_lines: Vec<String> = new.lines().map(|s| s.to_string()).collect();
        
        // Quick check: if new content is small or diff would be large, send full content
        if new.len() < 1024 || self.should_use_full_content(&original_lines, &new_lines) {
            return FileDiff::FullContent(new.to_string());
        }
        
        let operations = self.compute_diff_operations(&original_lines, &new_lines);
        
        FileDiff::LineDiff {
            operations,
            original_lines: original_lines.len() as u32,
            new_lines: new_lines.len() as u32,
        }
    }

    fn compute_diff_operations(&self, original: &[String], new: &[String]) -> Vec<DiffOperation> {
        let mut operations = Vec::new();
        let mut orig_pos = 0;
        let mut new_pos = 0;

        // Find common prefix
        let mut common_start = 0;
        while common_start < original.len() && common_start < new.len() 
            && original[common_start] == new[common_start] {
            common_start += 1;
        }

        if common_start > 0 {
            operations.push(DiffOperation::Keep { count: common_start as u32 });
            orig_pos = common_start;
            new_pos = common_start;
        }

        while orig_pos < original.len() || new_pos < new.len() {
            let mut best_match = None;
            let mut best_score = 0;

            // Look ahead for best matching sequence
            for orig_skip in 0..=original.len() - orig_pos {
                for new_skip in 0..=new.len() - new_pos {
                    if orig_skip == 0 && new_skip == 0 {
                        continue;
                    }

                    let mut score = 0;
                    for i in 0..orig_skip.min(new_skip) {
                        if original[orig_pos + i] == new[new_pos + i] {
                            score += 1;
                        }
                    }

                    if score > best_score {
                        best_score = score;
                        best_match = Some((orig_skip, new_skip));
                    }
                }
            }

            match best_match {
                Some((orig_skip, new_skip)) => {
                    if orig_skip == new_skip && best_score == orig_skip {
                        // Keep matching lines
                        operations.push(DiffOperation::Keep { count: orig_skip as u32 });
                    } else {
                        // Replace section with differences
                        if orig_skip > 0 {
                            operations.push(DiffOperation::Delete { count: orig_skip as u32 });
                        }
                        if new_skip > 0 {
                            operations.push(DiffOperation::Insert { 
                                lines: new[new_pos..new_pos + new_skip].iter().map(|s| s.to_string()).collect() 
                            });
                        }
                    }
                    orig_pos += orig_skip;
                    new_pos += new_skip;
                }
                None => {
                    // Delete remaining original lines
                    if orig_pos < original.len() {
                        operations.push(DiffOperation::Delete { count: (original.len() - orig_pos) as u32 });
                        orig_pos = original.len();
                    }
                    // Insert remaining new lines
                    if new_pos < new.len() {
                        operations.push(DiffOperation::Insert {
                            lines: new[new_pos..].iter().map(|s| s.to_string()).collect()
                        });
                        new_pos = new.len();
                    }
                }
            }
        }

        operations
    }

    fn should_use_full_content(&self, original_lines: &[String], new_lines: &[String]) -> bool {
        // If too many changes, full content might be smaller
        let changes = self.count_line_changes(original_lines, new_lines);
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

    fn count_line_changes(&self, original: &[String], new: &[String]) -> usize {
        use std::collections::HashSet;
        let original_set: HashSet<&String> = original.iter().collect();
        let new_set: HashSet<&String> = new.iter().collect();
        
        // Lines added + lines removed
        let added = new_set.difference(&original_set).count();
        let removed = original_set.difference(&new_set).count();
        
        added + removed
    }

    /// Apply a diff to the original content to get the new content
    pub fn apply_diff(&self, original: &str, diff: &FileDiff) -> Result<String> {
        match diff {
            FileDiff::FullContent(content) => Ok(content.clone()),
            FileDiff::LineDiff { operations, original_lines: _, new_lines: _ } => {
                let mut result = Vec::new();
                let lines: Vec<&str> = original.lines().collect();
                let mut current_line = 0;

                for op in operations {
                    match op {
                        DiffOperation::Keep { count } => {
                            for _ in 0..*count {
                                if current_line < lines.len() {
                                    result.push(lines[current_line].to_string());
                                    current_line += 1;
                                }
                            }
                        }
                        DiffOperation::Delete { count } => {
                            current_line += *count as usize;
                        }
                        DiffOperation::Insert { lines: new_lines } => {
                            result.extend(new_lines.iter().cloned());
                        }
                        DiffOperation::Replace { delete_count, insert_lines } => {
                            current_line += *delete_count as usize;
                            result.extend(insert_lines.iter().cloned());
                        }
                    }
                }

                Ok(result.join("\n"))
            }
            FileDiff::BinaryDiff { .. } => {
                Err(anyhow::anyhow!("Binary diff application not yet implemented"))
            }
            FileDiff::Deleted => Ok(String::new()),
        }
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
        let engine = DiffEngine::new();
        let original = "line 1\nline 2\nline 3";
        let new = "line 1\nmodified line 2\nline 3";
        
        let diff = engine.generate_line_diff(original, new);
        let applied = engine.apply_diff(original, &diff).unwrap();
        
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