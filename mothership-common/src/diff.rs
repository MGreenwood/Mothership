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

    /// Returns a `LineDiff` when it is smaller than the new content and rebuilds it exactly,
    /// otherwise `FullContent`.
    pub fn generate_line_diff(&self, original: &str, new: &str) -> FileDiff {
        // Small files are cheaper to send whole than to describe as a diff
        if new.len() < 1024 {
            return FileDiff::FullContent(new.to_string());
        }

        let original_lines = split_lines_keep_endings(original);
        let new_lines = split_lines_keep_endings(new);
        let operations = self.compute_diff_operations(&original_lines, &new_lines);

        let inserted_bytes: usize = operations
            .iter()
            .map(|op| match op {
                DiffOperation::Insert { lines } | DiffOperation::Replace { insert_lines: lines, .. } => {
                    lines.iter().map(|l| l.len()).sum()
                }
                _ => 0,
            })
            .sum();
        if inserted_bytes >= new.len() {
            return FileDiff::FullContent(new.to_string());
        }

        let diff = FileDiff::LineDiff {
            operations,
            original_lines: original_lines.len() as u32,
            new_lines: new_lines.len() as u32,
        };

        // Only use the diff if it reproduces the new content exactly
        match self.apply_diff(original, &diff) {
            Ok(rebuilt) if rebuilt == new => diff,
            _ => FileDiff::FullContent(new.to_string()),
        }
    }

    /// Keeps the common prefix and suffix and replaces the changed middle as one block.
    fn compute_diff_operations(&self, original: &[&str], new: &[&str]) -> Vec<DiffOperation> {
        let prefix = original.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
        let suffix = original[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();

        let orig_mid = &original[prefix..original.len() - suffix];
        let new_mid = &new[prefix..new.len() - suffix];
        let owned = |lines: &[&str]| lines.iter().map(|l| l.to_string()).collect::<Vec<String>>();

        let mut operations = Vec::new();
        if prefix > 0 {
            operations.push(DiffOperation::Keep { count: prefix as u32 });
        }
        match (orig_mid.is_empty(), new_mid.is_empty()) {
            (true, true) => {}
            (true, false) => operations.push(DiffOperation::Insert { lines: owned(new_mid) }),
            (false, true) => operations.push(DiffOperation::Delete { count: orig_mid.len() as u32 }),
            (false, false) => operations.push(DiffOperation::Replace {
                delete_count: orig_mid.len() as u32,
                insert_lines: owned(new_mid),
            }),
        }
        if suffix > 0 {
            operations.push(DiffOperation::Keep { count: suffix as u32 });
        }
        operations
    }

    /// Apply a diff to the original content to get the new content
    pub fn apply_diff(&self, original: &str, diff: &FileDiff) -> Result<String> {
        match diff {
            FileDiff::FullContent(content) => Ok(content.clone()),
            FileDiff::LineDiff { operations, original_lines, new_lines: _ } => {
                let lines = split_lines_keep_endings(original);
                if lines.len() != *original_lines as usize {
                    return Err(anyhow::anyhow!(
                        "Diff base mismatch: diff expects {} lines, base has {}",
                        original_lines,
                        lines.len()
                    ));
                }

                let mut result = Vec::new();
                let mut current_line = 0;

                for op in operations {
                    match op {
                        DiffOperation::Keep { count } => {
                            let end = take_lines(&lines, current_line, *count)?;
                            result.extend(lines[current_line..end].iter().map(|l| l.to_string()));
                            current_line = end;
                        }
                        DiffOperation::Delete { count } => {
                            current_line = take_lines(&lines, current_line, *count)?;
                        }
                        DiffOperation::Insert { lines: new_lines } => {
                            result.extend(new_lines.iter().cloned());
                        }
                        DiffOperation::Replace { delete_count, insert_lines } => {
                            current_line = take_lines(&lines, current_line, *delete_count)?;
                            result.extend(insert_lines.iter().cloned());
                        }
                    }
                }

                if current_line != lines.len() {
                    return Err(anyhow::anyhow!(
                        "Diff did not consume base: stopped at line {} of {}",
                        current_line,
                        lines.len()
                    ));
                }

                Ok(result.concat())
            }
            FileDiff::BinaryDiff { .. } => {
                Err(anyhow::anyhow!("Binary diff application not yet implemented"))
            }
            FileDiff::Deleted => Ok(String::new()),
        }
    }
}

/// Split into lines that keep their terminators (`\n`, or `\r\n`), so content
/// concatenates back to the exact original bytes.
fn split_lines_keep_endings(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Advance `count` lines from `start`, failing if that runs past the base.
fn take_lines(lines: &[&str], start: usize, count: u32) -> Result<usize> {
    let end = start + count as usize;
    if end > lines.len() {
        return Err(anyhow::anyhow!(
            "Diff operation runs past end of base ({} lines, needs {})",
            lines.len(),
            end
        ));
    }
    Ok(end)
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
    fn test_large_line_diff_round_trip() {
        let engine = DiffEngine::new();
        let original: String = (0..200).map(|i| format!("let value_{} = {};", i, i)).collect::<Vec<_>>().join("\n");
        let new = original.replacen("value_100 = 100", "value_100 = 999", 1);

        let diff = engine.generate_line_diff(&original, &new);
        assert!(matches!(diff, FileDiff::LineDiff { .. }), "expected LineDiff path");
        assert_eq!(engine.apply_diff(&original, &diff).unwrap(), new);
    }

    #[test]
    fn test_apply_rejects_base_mismatch() {
        let engine = DiffEngine::new();
        let original = "a\nb\nc";
        let diff = FileDiff::LineDiff {
            operations: vec![DiffOperation::Keep { count: 3 }],
            original_lines: 3,
            new_lines: 3,
        };

        assert!(engine.apply_diff("a\nb", &diff).is_err());
    }

    #[test]
    fn test_apply_rejects_out_of_range_operation() {
        let engine = DiffEngine::new();
        let diff = FileDiff::LineDiff {
            operations: vec![DiffOperation::Keep { count: 5 }],
            original_lines: 3,
            new_lines: 5,
        };

        assert!(engine.apply_diff("a\nb\nc", &diff).is_err());
    }

    #[test]
    fn test_apply_rejects_incomplete_diff() {
        let engine = DiffEngine::new();
        let diff = FileDiff::LineDiff {
            operations: vec![DiffOperation::Keep { count: 1 }],
            original_lines: 3,
            new_lines: 1,
        };

        assert!(engine.apply_diff("a\nb\nc", &diff).is_err());
    }

    #[test]
    fn test_round_trip_preserves_trailing_newline() {
        let engine = DiffEngine::new();
        let original: String = (0..200).map(|i| format!("let value_{} = {};\n", i, i)).collect();
        let new = original.replacen("value_100 = 100", "value_100 = 999", 1);

        let diff = engine.generate_line_diff(&original, &new);
        assert!(matches!(diff, FileDiff::LineDiff { .. }), "expected LineDiff path");
        assert_eq!(engine.apply_diff(&original, &diff).unwrap(), new);
    }

    #[test]
    fn test_round_trip_preserves_crlf() {
        let engine = DiffEngine::new();
        let original: String = (0..200).map(|i| format!("let value_{} = {};\r\n", i, i)).collect();
        let new = original.replacen("value_100 = 100", "value_100 = 999", 1);

        let diff = engine.generate_line_diff(&original, &new);
        assert!(matches!(diff, FileDiff::LineDiff { .. }), "expected LineDiff path");
        assert_eq!(engine.apply_diff(&original, &diff).unwrap(), new);
    }

    #[test]
    fn test_single_line_edit_is_small_on_crlf_without_trailing_newline() {
        let engine = DiffEngine::new();
        let original: String = (0..200).map(|i| format!("let value_{} = {};", i, i)).collect::<Vec<_>>().join("\r\n");
        let new = original.replacen("value_100 = 100", "value_100 = 999", 1);

        let diff = engine.generate_line_diff(&original, &new);
        match &diff {
            FileDiff::LineDiff { operations, .. } => {
                let sent: usize = operations
                    .iter()
                    .map(|op| match op {
                        DiffOperation::Insert { lines } | DiffOperation::Replace { insert_lines: lines, .. } => {
                            lines.iter().map(|l| l.len()).sum()
                        }
                        _ => 0,
                    })
                    .sum();
                assert!(sent < 100, "expected only the changed line to be sent, sent {} bytes", sent);
            }
            other => panic!("expected LineDiff, got {:?}", other),
        }
        assert_eq!(engine.apply_diff(&original, &diff).unwrap(), new);
    }

    #[test]
    fn test_insert_and_delete_at_edges_round_trip() {
        let engine = DiffEngine::new();
        let base: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        let prepended = format!("header\n{}", base);
        let trimmed = base.replacen("line 0\n", "", 1);

        for new in [prepended, trimmed] {
            let diff = engine.generate_line_diff(&base, &new);
            assert_eq!(engine.apply_diff(&base, &diff).unwrap(), new);
        }
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