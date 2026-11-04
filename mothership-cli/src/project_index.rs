use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use walkdir::DirEntry;

#[derive(Deserialize)]
struct IndexedProjectMetadata {
    project_name: String,
}

/// Default search roots in priority order: cwd, `%USERPROFILE%\mothership`, `%USERPROFILE%`.
pub fn default_search_roots() -> Vec<PathBuf> {
    let user_profile = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()));
    vec![
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        user_profile.join("mothership"),
        user_profile,
    ]
}

fn should_visit_entry(entry: &DirEntry, user_profile: Option<&Path>) -> bool {
    if entry.depth() == 0 {
        return true;
    }

    if !entry.file_type().is_dir() {
        return false;
    }

    // Do not descend into subdirectories inside `.mothership`.
    if entry
        .path()
        .parent()
        .and_then(|p| p.file_name())
        == Some(OsStr::new(".mothership"))
    {
        return false;
    }

    let Some(name) = entry.file_name().to_str() else {
        return true;
    };

    if name == ".mothership" {
        return true;
    }

    if name.starts_with('.') {
        return false;
    }

    if matches!(name, "node_modules" | "target" | ".git") {
        return false;
    }

    if name.eq_ignore_ascii_case("AppData") {
        if let Some(profile) = user_profile {
            if entry.path().parent() == Some(profile) {
                return false;
            }
        }
        return false;
    }

    true
}

/// Walks each root up to depth 3, pruning ignored directories, and returns a map
/// of `project_name -> project_dir` with first-match-wins semantics across roots.
pub fn build_index(roots: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let user_profile_ref = user_profile.as_deref();

    for root in roots {
        if !root.exists() {
            continue;
        }

        let walker = walkdir::WalkDir::new(root)
            .max_depth(3)
            .into_iter()
            .filter_entry(|entry| should_visit_entry(entry, user_profile_ref));

        for entry in walker.filter_map(|e| e.ok()) {
            if entry.file_type().is_dir() && entry.file_name() == OsStr::new(".mothership") {
                let path = entry.path();
                let Some(project_dir) = path.parent() else {
                    continue;
                };
                let metadata_file = path.join("project.json");
                if let Ok(content) = std::fs::read_to_string(&metadata_file) {
                    if let Ok(metadata) = serde_json::from_str::<IndexedProjectMetadata>(&content) {
                        index
                            .entry(metadata.project_name)
                            .or_insert_with(|| project_dir.to_path_buf());
                    }
                }
            }
        }
    }

    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiling::TempTree;

    fn write_project_meta(dir: &Path, name: &str) {
        let meta_dir = dir.join(".mothership");
        std::fs::create_dir_all(&meta_dir).unwrap();
        let meta = serde_json::json!({
            "project_id": uuid::Uuid::new_v4().to_string(),
            "project_name": name,
            "created_at": "2026-01-01T00:00:00Z",
            "mothership_url": "http://localhost",
        });
        std::fs::write(meta_dir.join("project.json"), meta.to_string()).unwrap();
    }

    #[test]
    fn build_index_prunes_ignored_directories() {
        let tree = TempTree::new("index-prune");
        let root = &tree.0;

        let valid_dir = root.join("alpha");
        write_project_meta(&valid_dir, "alpha");

        // Projects nested inside pruned directories must be skipped.
        write_project_meta(&root.join("node_modules").join("hidden-nm"), "hidden-nm");
        write_project_meta(&root.join("target").join("hidden-target"), "hidden-target");
        write_project_meta(&root.join(".git").join("hidden-git"), "hidden-git");
        write_project_meta(&root.join(".cache").join("hidden-dot"), "hidden-dot");
        write_project_meta(&root.join("AppData").join("hidden-appdata"), "hidden-appdata");

        let index = build_index(std::slice::from_ref(root));
        assert_eq!(index.get("alpha"), Some(&valid_dir));
        assert!(!index.contains_key("hidden-nm"));
        assert!(!index.contains_key("hidden-target"));
        assert!(!index.contains_key("hidden-git"));
        assert!(!index.contains_key("hidden-dot"));
        assert!(!index.contains_key("hidden-appdata"));
    }

    #[test]
    fn build_index_preserves_first_match_wins() {
        let first = TempTree::new("index-first");
        let second = TempTree::new("index-second");

        let first_proj = first.0.join("shared");
        let second_proj = second.0.join("shared");
        let second_only = second.0.join("unique");

        write_project_meta(&first_proj, "shared");
        write_project_meta(&second_proj, "shared");
        write_project_meta(&second_only, "unique");

        let roots = vec![first.0.clone(), second.0.clone()];
        let index = build_index(&roots);

        assert_eq!(index.get("shared"), Some(&first_proj));
        assert_eq!(index.get("unique"), Some(&second_proj.parent().unwrap().join("unique")));
    }
}
