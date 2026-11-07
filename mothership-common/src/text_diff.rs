//! Text diffs between two sets of files, such as a rift's live state or a checkpoint.
//! The server, CLI, and GUI all use these to show what changed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use crate::normalize_rel_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub status: DiffStatus,
    /// Lines in the right-hand side that the left-hand side does not have
    pub added: usize,
    /// Lines in the left-hand side that the right-hand side does not have
    pub removed: usize,
    /// Unified diff text. Only present when the caller asked for full diffs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified: Option<String>,
}

/// Diffs every file that differs between `from` (left) and `to` (right), sorted by path.
pub fn diff_files(
    from: &HashMap<PathBuf, String>,
    to: &HashMap<PathBuf, String>,
    include_text: bool,
) -> Vec<FileDiff> {
    let left = index_by_path(from);
    let right = index_by_path(to);
    let paths: BTreeSet<&String> = left.keys().chain(right.keys()).collect();

    paths
        .into_iter()
        .filter_map(|path| {
            file_diff(
                path,
                left.get(path).copied().map(String::as_str),
                right.get(path).copied().map(String::as_str),
                include_text,
            )
        })
        .collect()
}

/// Diff for one file. A side that is `None` means the file is absent there.
/// Returns `None` when both sides hold the same content.
pub fn file_diff(path: &str, from: Option<&str>, to: Option<&str>, include_text: bool) -> Option<FileDiff> {
    if from == to {
        return None;
    }

    let status = match (from, to) {
        (None, Some(_)) => DiffStatus::Added,
        (Some(_), None) => DiffStatus::Removed,
        _ => DiffStatus::Modified,
    };

    let diff = TextDiff::from_lines(from.unwrap_or(""), to.unwrap_or(""));
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    let unified = include_text.then(|| {
        diff.unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string()
    });

    Some(FileDiff {
        path: path.to_string(),
        status,
        added,
        removed,
        unified,
    })
}

fn index_by_path(files: &HashMap<PathBuf, String>) -> BTreeMap<String, &String> {
    files
        .iter()
        .map(|(path, content)| (normalize_rel_path(path), content))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        pairs.iter().map(|(p, c)| (PathBuf::from(p), c.to_string())).collect()
    }

    #[test]
    fn classifies_added_removed_and_modified_and_skips_unchanged() {
        let from = files(&[("keep.txt", "same"), ("gone.txt", "bye"), ("edit.txt", "old")]);
        let to = files(&[("keep.txt", "same"), ("new.txt", "hi"), ("edit.txt", "new")]);

        let diffs = diff_files(&from, &to, false);

        let summary: Vec<(&str, DiffStatus)> = diffs.iter().map(|d| (d.path.as_str(), d.status)).collect();
        assert_eq!(
            summary,
            vec![
                ("edit.txt", DiffStatus::Modified),
                ("gone.txt", DiffStatus::Removed),
                ("new.txt", DiffStatus::Added),
            ]
        );
        assert!(diffs.iter().all(|d| d.unified.is_none()));
    }

    #[test]
    fn counts_lines_added_and_removed() {
        let diff = file_diff("a.txt", Some("a\nb\nc\n"), Some("a\nB\nc\nd\n"), false).unwrap();
        assert_eq!((diff.added, diff.removed), (2, 1));
    }

    #[test]
    fn full_diffs_include_unified_text() {
        let diff = file_diff("a.txt", Some("a\nb\nc\n"), Some("a\nB\nc\n"), true).unwrap();
        let text = diff.unified.expect("unified text requested");
        assert!(text.contains("-b"), "{text}");
        assert!(text.contains("+B"), "{text}");
        assert!(text.contains("a/a.txt") && text.contains("b/a.txt"), "{text}");
    }

    #[test]
    fn identical_content_has_no_diff() {
        assert!(file_diff("a.txt", Some("same"), Some("same"), true).is_none());
    }
}
