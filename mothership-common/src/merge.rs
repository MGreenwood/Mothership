//! Line-based N-way merge used by wormholes.
//!
//! Each rift's edits are diffed against a shared base. Edits whose base line
//! ranges overlap form a cluster. A cluster changed by one rift, or identically
//! by several, merges cleanly. Anything else becomes a conflict that needs a
//! resolution. Pure functions, no I/O.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

/// Upper bound on LCS table cells. Larger changed regions are treated as one block
/// so memory stays bounded.
const MAX_LCS_CELLS: usize = 16_000_000;
/// Hex characters of the fingerprint used as a conflict id. Short enough to type.
const CONFLICT_ID_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    /// Half-open range of base lines this edit replaces
    base_start: usize,
    base_end: usize,
    lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conflict {
    pub id: String,
    pub base_start: usize,
    pub base_end: usize,
    /// Lines each rift proposes for this region, indexed like the input rifts
    pub sides: Vec<Vec<String>>,
    /// Hash of the base region and every side. A resolution only applies while this matches.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Segment {
    Clean(Vec<String>),
    Conflict(Conflict),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileMerge {
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Choice {
    /// Take the lines proposed by the rift at this index
    Side(usize),
    Custom(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resolution {
    pub fingerprint: String,
    pub choice: Choice,
}

impl FileMerge {
    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict> + '_ {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Conflict(conflict) => Some(conflict),
            Segment::Clean(_) => None,
        })
    }

    /// Conflicts with no resolution, or whose resolution was made against older content
    pub fn unresolved(&self, resolutions: &HashMap<String, Resolution>) -> Vec<&Conflict> {
        self.conflicts()
            .filter(|conflict| {
                resolutions
                    .get(&conflict.id)
                    .map_or(true, |resolution| resolution.fingerprint != conflict.fingerprint)
            })
            .collect()
    }

    /// Final lines once every conflict has a current resolution, otherwise None
    pub fn render(&self, resolutions: &HashMap<String, Resolution>) -> Option<Vec<String>> {
        if !self.unresolved(resolutions).is_empty() {
            return None;
        }
        let mut out = Vec::new();
        for segment in &self.segments {
            match segment {
                Segment::Clean(lines) => out.extend(lines.iter().cloned()),
                Segment::Conflict(conflict) => match &resolutions.get(&conflict.id)?.choice {
                    Choice::Side(index) => out.extend(conflict.sides.get(*index)?.iter().cloned()),
                    Choice::Custom(lines) => out.extend(lines.iter().cloned()),
                },
            }
        }
        Some(out)
    }
}

/// Merge one file across several rifts. `None` means the file is absent,
/// which is different from an empty file.
pub fn merge_file(base: Option<&str>, rifts: &[Option<&str>]) -> FileMerge {
    let base_lines = lines_of(base);
    let rift_lines: Vec<Vec<&str>> = rifts.iter().map(|content| lines_of(*content)).collect();

    let mut tagged: Vec<(usize, Hunk)> = Vec::new();
    for (index, lines) in rift_lines.iter().enumerate() {
        for hunk in diff_lines(&base_lines, lines) {
            tagged.push((index, hunk));
        }
    }
    tagged.sort_by_key(|(_, hunk)| (hunk.base_start, hunk.base_end));

    let mut clusters: Vec<Vec<(usize, Hunk)>> = Vec::new();
    let mut span = (0usize, 0usize);
    for item in tagged {
        let range = (item.1.base_start, item.1.base_end);
        if !clusters.is_empty() && ranges_overlap(span, range) {
            span.1 = span.1.max(range.1);
            clusters.last_mut().expect("cluster list is non-empty").push(item);
        } else {
            span = range;
            clusters.push(vec![item]);
        }
    }

    let mut segments = Vec::new();
    let mut clean: Vec<String> = Vec::new();
    let mut pos = 0usize;
    for cluster in clusters {
        let start = cluster.iter().map(|(_, hunk)| hunk.base_start).min().unwrap_or(0);
        let end = cluster.iter().map(|(_, hunk)| hunk.base_end).max().unwrap_or(0);
        clean.extend(base_lines[pos..start].iter().map(|line| line.to_string()));

        let per_rift: Vec<Vec<String>> = (0..rifts.len())
            .map(|index| {
                let hunks: Vec<&Hunk> = cluster
                    .iter()
                    .filter(|(owner, _)| *owner == index)
                    .map(|(_, hunk)| hunk)
                    .collect();
                region_text(&base_lines, start, end, &hunks)
            })
            .collect();

        let touching: BTreeSet<usize> = cluster.iter().map(|(owner, _)| *owner).collect();
        let proposals: Vec<&Vec<String>> = touching.iter().map(|index| &per_rift[*index]).collect();
        if proposals.windows(2).all(|pair| pair[0] == pair[1]) {
            clean.extend(proposals[0].iter().cloned());
        } else {
            if !clean.is_empty() {
                segments.push(Segment::Clean(std::mem::take(&mut clean)));
            }
            segments.push(Segment::Conflict(build_conflict(start, end, &base_lines, per_rift)));
        }
        pos = end;
    }
    clean.extend(base_lines[pos..].iter().map(|line| line.to_string()));
    if !clean.is_empty() {
        segments.push(Segment::Clean(clean));
    }

    FileMerge { segments }
}

/// Merge every file across the rifts in a wormhole against a shared base
pub fn merge_rifts(
    base: &HashMap<PathBuf, String>,
    rifts: &[&HashMap<PathBuf, String>],
) -> HashMap<PathBuf, FileMerge> {
    let paths: BTreeSet<&PathBuf> = base.keys().chain(rifts.iter().flat_map(|rift| rift.keys())).collect();
    paths
        .into_iter()
        .map(|path| {
            let base_content = base.get(path).map(String::as_str);
            let contents: Vec<Option<&str>> = rifts.iter().map(|rift| rift.get(path).map(String::as_str)).collect();
            (path.clone(), merge_file(base_content, &contents))
        })
        .collect()
}

/// Base for a new wormhole: only files whose content is identical in every rift.
/// Files that already differ are left out, so their first merge treats each side's
/// lines as new and reports a conflict.
pub fn snapshot_base(rifts: &[&HashMap<PathBuf, String>]) -> HashMap<PathBuf, String> {
    let Some((first, rest)) = rifts.split_first() else {
        return HashMap::new();
    };
    first
        .iter()
        .filter(|(path, content)| rest.iter().all(|rift| rift.get(*path) == Some(*content)))
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect()
}

/// Base for a new wormhole, falling back to a common ancestor rift (such as `main`)
/// for paths where the selected rifts have already diverged before the wormhole was created.
pub fn snapshot_base_with_fallback(
    rifts: &[&HashMap<PathBuf, String>],
    fallback_base: Option<&HashMap<PathBuf, String>>,
) -> HashMap<PathBuf, String> {
    let mut base = snapshot_base(rifts);
    if let Some(fallback) = fallback_base {
        for (path, content) in fallback {
            base.entry(path.clone()).or_insert_with(|| content.clone());
        }
    }
    base
}

fn lines_of(content: Option<&str>) -> Vec<&str> {
    match content {
        None | Some("") => Vec::new(),
        Some(text) => text.split('\n').collect(),
    }
}

/// Matches Git: ranges conflict if they share a base line or touch at an edge.
/// Two insertions at the same point conflict. Edits separated by at least one
/// untouched base line do not.
fn ranges_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

fn diff_lines(base: &[&str], side: &[&str]) -> Vec<Hunk> {
    let prefix = base.iter().zip(side).take_while(|(a, b)| a == b).count();
    let max_suffix = base.len().min(side.len()) - prefix;
    let suffix = base[prefix..]
        .iter()
        .rev()
        .zip(side[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(max_suffix);

    let mut hunks = diff_middle(&base[prefix..base.len() - suffix], &side[prefix..side.len() - suffix]);
    for hunk in &mut hunks {
        hunk.base_start += prefix;
        hunk.base_end += prefix;
    }
    hunks
}

/// LCS-based diff of the region between the common prefix and suffix
fn diff_middle(base: &[&str], side: &[&str]) -> Vec<Hunk> {
    let (n, m) = (base.len(), side.len());
    if n == 0 && m == 0 {
        return Vec::new();
    }
    if (n + 1).saturating_mul(m + 1) > MAX_LCS_CELLS {
        return vec![Hunk {
            base_start: 0,
            base_end: n,
            lines: side.iter().map(|line| line.to_string()).collect(),
        }];
    }

    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if base[i] == side[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }

    let mut hunks = Vec::new();
    let mut open: Option<Hunk> = None;
    let (mut i, mut j) = (0usize, 0usize);
    while i < n || j < m {
        if i < n && j < m && base[i] == side[j] {
            if let Some(hunk) = open.take() {
                hunks.push(hunk);
            }
            i += 1;
            j += 1;
            continue;
        }
        let delete_base = i < n && (j == m || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]);
        let hunk = open.get_or_insert_with(|| Hunk { base_start: i, base_end: i, lines: Vec::new() });
        if delete_base {
            hunk.base_end = i + 1;
            i += 1;
        } else {
            hunk.lines.push(side[j].to_string());
            j += 1;
        }
    }
    if let Some(hunk) = open.take() {
        hunks.push(hunk);
    }
    hunks
}

/// Text of one rift over base lines `start..end`, with its hunks applied
fn region_text(base: &[&str], start: usize, end: usize, hunks: &[&Hunk]) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = start;
    for hunk in hunks {
        out.extend(base[pos..hunk.base_start].iter().map(|line| line.to_string()));
        out.extend(hunk.lines.iter().cloned());
        pos = hunk.base_end;
    }
    out.extend(base[pos..end].iter().map(|line| line.to_string()));
    out
}

fn build_conflict(start: usize, end: usize, base: &[&str], sides: Vec<Vec<String>>) -> Conflict {
    let mut hasher = Sha256::new();
    for line in &base[start..end] {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    for side in &sides {
        hasher.update(b"\x1e");
        for line in side {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
    }
    let fingerprint = format!("{:x}", hasher.finalize());
    Conflict {
        // Derived from content, not position, so the id survives edits elsewhere in the file
        id: fingerprint[..CONFLICT_ID_LEN].to_string(),
        base_start: start,
        base_end: end,
        sides,
        fingerprint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge(base: &str, rifts: &[&str]) -> FileMerge {
        let contents: Vec<Option<&str>> = rifts.iter().map(|content| Some(*content)).collect();
        merge_file(Some(base), &contents)
    }

    fn clean_text(merged: &FileMerge) -> String {
        let lines = merged.render(&HashMap::new()).expect("merge should be clean");
        lines.join("\n")
    }

    fn conflict_count(merged: &FileMerge) -> usize {
        merged.conflicts().count()
    }

    const BASE: &str = "a\nb\nc\nd\ne";

    #[test]
    fn identical_rifts_are_clean() {
        let merged = merge(BASE, &[BASE, BASE]);
        assert_eq!(conflict_count(&merged), 0);
        assert_eq!(clean_text(&merged), BASE);
    }

    #[test]
    fn single_rift_edit_is_taken() {
        let merged = merge(BASE, &["A\nb\nc\nd\ne", BASE]);
        assert_eq!(clean_text(&merged), "A\nb\nc\nd\ne");
    }

    #[test]
    fn edits_in_different_places_combine() {
        let merged = merge(BASE, &["A\nb\nc\nd\ne", "a\nb\nc\nd\nE"]);
        assert_eq!(conflict_count(&merged), 0);
        assert_eq!(clean_text(&merged), "A\nb\nc\nd\nE");
    }

    #[test]
    fn same_line_different_edits_conflict() {
        let merged = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nY\nd\ne"]);
        assert_eq!(conflict_count(&merged), 1);
        let conflict = merged.conflicts().next().unwrap();
        assert_eq!(conflict.sides, vec![vec!["X".to_string()], vec!["Y".to_string()]]);
    }

    #[test]
    fn identical_edits_do_not_conflict() {
        let merged = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nX\nd\ne"]);
        assert_eq!(conflict_count(&merged), 0);
        assert_eq!(clean_text(&merged), "a\nb\nX\nd\ne");
    }

    #[test]
    fn five_lines_inserted_mid_file_do_not_conflict_with_edit_elsewhere() {
        let inserted = "a\nb\nN1\nN2\nN3\nN4\nN5\nc\nd\ne";
        let merged = merge(BASE, &[inserted, "a\nb\nc\nd\nE"]);
        assert_eq!(conflict_count(&merged), 0);
        assert_eq!(clean_text(&merged), "a\nb\nN1\nN2\nN3\nN4\nN5\nc\nd\nE");
    }

    #[test]
    fn overlapping_function_edits_conflict() {
        let base = "1\n2\n3\n4\n5\n6";
        let merged = merge(base, &["1\nx\ny\n4\n5\n6", "1\n2\nz\n4\n5\n6"]);
        assert_eq!(conflict_count(&merged), 1);
    }

    #[test]
    fn adjacent_line_edits_conflict_like_git() {
        let base = "1\n2\n3\n4";
        let merged = merge(base, &["1\nX\n3\n4", "1\n2\nY\n4"]);
        assert_eq!(conflict_count(&merged), 1);
    }

    #[test]
    fn edits_separated_by_one_untouched_line_do_not_conflict() {
        let base = "1\n2\n3\n4\n5";
        let merged = merge(base, &["X\n2\n3\n4\n5", "1\n2\n3\nY\n5"]);
        assert_eq!(conflict_count(&merged), 0);
        assert_eq!(clean_text(&merged), "X\n2\n3\nY\n5");
    }

    #[test]
    fn two_inserts_at_same_spot_conflict() {
        let base = "a\nb\nc";
        let merged = merge(base, &["a\nb\np\nc", "a\nb\nq\nc"]);
        assert_eq!(conflict_count(&merged), 1);
    }

    #[test]
    fn two_sides_writing_into_empty_base_conflict() {
        let merged = merge("", &["line1\nside-a\nline3", "line1\nline2\nline3"]);
        assert_eq!(conflict_count(&merged), 1);
        let conflict = merged.conflicts().next().unwrap();
        assert_eq!((conflict.base_start, conflict.base_end), (0, 0));
    }

    #[test]
    fn resolution_applies_while_fingerprint_matches() {
        let merged = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nY\nd\ne"]);
        let conflict = merged.conflicts().next().unwrap().clone();
        let mut resolutions = HashMap::new();
        resolutions.insert(
            conflict.id.clone(),
            Resolution { fingerprint: conflict.fingerprint.clone(), choice: Choice::Side(1) },
        );
        assert_eq!(merged.render(&resolutions).unwrap().join("\n"), "a\nb\nY\nd\ne");
    }

    #[test]
    fn resolution_resets_when_conflict_content_changes() {
        let first = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nY\nd\ne"]);
        let conflict = first.conflicts().next().unwrap().clone();
        let mut resolutions = HashMap::new();
        resolutions.insert(
            conflict.id.clone(),
            Resolution { fingerprint: conflict.fingerprint.clone(), choice: Choice::Side(0) },
        );

        let changed = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nZ\nd\ne"]);
        assert_eq!(changed.unresolved(&resolutions).len(), 1);
        assert!(changed.render(&resolutions).is_none());
    }

    #[test]
    fn unrelated_change_keeps_resolution() {
        let first = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nY\nd\ne"]);
        let conflict = first.conflicts().next().unwrap().clone();
        let mut resolutions = HashMap::new();
        resolutions.insert(
            conflict.id.clone(),
            Resolution { fingerprint: conflict.fingerprint.clone(), choice: Choice::Custom(vec!["Q".into()]) },
        );

        let later = merge(BASE, &["a\nb\nX\nd\ne", "a\nb\nY\nd\ne"]);
        assert!(later.unresolved(&resolutions).is_empty());
    }

    #[test]
    fn two_separate_conflicts_resolve_independently() {
        let base = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10";
        let merged = merge(base, &["1\nA2\n3\n4\n5\n6\n7\nA8\n9\n10", "1\nB2\n3\n4\n5\n6\n7\nB8\n9\n10"]);
        assert_eq!(conflict_count(&merged), 2);

        let conflicts: Vec<_> = merged.conflicts().cloned().collect();
        assert_ne!(conflicts[0].id, conflicts[1].id);

        let mut resolutions = HashMap::new();
        resolutions.insert(
            conflicts[0].id.clone(),
            Resolution { fingerprint: conflicts[0].fingerprint.clone(), choice: Choice::Side(0) },
        );
        assert_eq!(merged.unresolved(&resolutions).len(), 1);
        assert!(merged.render(&resolutions).is_none());

        resolutions.insert(
            conflicts[1].id.clone(),
            Resolution { fingerprint: conflicts[1].fingerprint.clone(), choice: Choice::Custom(vec!["C8".into()]) },
        );
        assert!(merged.unresolved(&resolutions).is_empty());
        assert_eq!(
            merged.render(&resolutions).unwrap().join("\n"),
            "1\nA2\n3\n4\n5\n6\n7\nC8\n9\n10"
        );
    }

    #[test]
    fn deleting_all_lines_yields_no_lines() {
        let merged = merge_file(Some("a\nb"), &[None, Some("a\nb")]);
        assert_eq!(conflict_count(&merged), 0);
        assert!(merged.render(&HashMap::new()).unwrap().is_empty());
    }

    #[test]
    fn three_rifts_conflict_only_between_those_that_disagree() {
        let base = "a\nb\nc";
        let merged = merge(base, &["a\nX\nc", "a\nb\nc", "a\nY\nc"]);
        assert_eq!(conflict_count(&merged), 1);
        let conflict = merged.conflicts().next().unwrap();
        assert_eq!(conflict.sides.len(), 3);
        assert_eq!(conflict.sides[1], vec!["b".to_string()]);
    }

    #[test]
    fn snapshot_base_excludes_files_that_already_differ() {
        let mut first = HashMap::new();
        first.insert(PathBuf::from("same.txt"), "x".to_string());
        first.insert(PathBuf::from("diff.txt"), "one".to_string());
        let mut second = HashMap::new();
        second.insert(PathBuf::from("same.txt"), "x".to_string());
        second.insert(PathBuf::from("diff.txt"), "two".to_string());

        let base = snapshot_base(&[&first, &second]);
        assert_eq!(base.get(&PathBuf::from("same.txt")), Some(&"x".to_string()));
        assert!(!base.contains_key(&PathBuf::from("diff.txt")));

        let merged = merge_rifts(&base, &[&first, &second]);
        assert_eq!(merged[&PathBuf::from("diff.txt")].conflicts().count(), 1);
        assert_eq!(merged[&PathBuf::from("same.txt")].conflicts().count(), 0);
    }

    #[test]
    fn toggling_side_off_falls_back_to_base_while_preserving_other_sides() {
        let mut base = HashMap::new();
        base.insert(PathBuf::from("app.rs"), "line1\nline2\nline3".to_string());

        let mut alice = base.clone();
        alice.insert(PathBuf::from("app.rs"), "alice1\nline2\nline3".to_string());
        alice.insert(PathBuf::from("alice_mod.rs"), "pub fn alice() {}".to_string());

        let mut bob = base.clone();
        bob.insert(PathBuf::from("app.rs"), "bob1\nline2\nline3".to_string());
        bob.insert(PathBuf::from("bob_mod.rs"), "pub fn bob() {}".to_string());

        // Both enabled: app.rs has a conflict on line 1.
        let both_on = merge_rifts(&base, &[&alice, &bob]);
        assert_eq!(both_on[&PathBuf::from("app.rs")].conflicts().count(), 1);

        // Toggle Bob off (replace Bob's side with &base): conflict disappears and Alice's module remains active.
        let bob_off = merge_rifts(&base, &[&alice, &base]);
        assert_eq!(bob_off[&PathBuf::from("app.rs")].conflicts().count(), 0);
        assert_eq!(
            bob_off[&PathBuf::from("app.rs")]
                .render(&HashMap::new())
                .unwrap()
                .join("\n"),
            "alice1\nline2\nline3"
        );
        assert_eq!(
            bob_off[&PathBuf::from("alice_mod.rs")]
                .render(&HashMap::new())
                .unwrap()
                .join("\n"),
            "pub fn alice() {}"
        );
        assert!(!bob_off.contains_key(&PathBuf::from("bob_mod.rs")));
    }
}
