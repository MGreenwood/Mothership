//! Directory listing for the file explorer. Lists one level at a time so the
//! tree can load folders lazily when they are expanded.

use std::path::Path;

use mothership_common::daemon;
use serde::Serialize;

/// Folders that are never useful to browse in the editor.
const HIDDEN_NAMES: &[&str] = &[".git", "node_modules", "target"];

/// True when the daemon is tracking a project whose folder contains `path`, so edits in it sync.
/// An unreachable daemon counts as not beamed.
#[tauri::command]
pub async fn explorer_is_beamed(path: String) -> bool {
    let Ok(tracked) = daemon::tracked_projects().await else {
        return false;
    };
    tracked
        .iter()
        .any(|p| is_within(Path::new(&path), Path::new(&p.project_path)))
}

/// Compares canonical paths when both exist, so casing and relative segments don't matter.
fn is_within(child: &Path, parent: &Path) -> bool {
    let child = std::fs::canonicalize(child).unwrap_or_else(|_| child.to_path_buf());
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    child.starts_with(parent)
}

#[derive(Serialize)]
pub struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
}

#[tauri::command]
pub fn explorer_list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);
    let read = std::fs::read_dir(dir).map_err(|e| format!("{path}: {e}"))?;

    let mut entries = Vec::new();
    for item in read {
        let item = item.map_err(|e| format!("{path}: {e}"))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if HIDDEN_NAMES.contains(&name.as_str()) {
            continue;
        }
        let meta = match item.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        entries.push(DirEntry {
            name,
            path: item.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            size: if meta.is_file() { meta.len() } else { 0 },
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_dirs_first_and_skips_hidden_folders() {
        let dir = std::env::temp_dir().join(format!("mothership-explorer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("A.md"), "a").unwrap();

        let entries = explorer_list_dir(dir.to_string_lossy().into_owned()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "A.md", "b.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_is_within_its_beamed_project() {
        let root = std::env::temp_dir().join(format!("mothership-within-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let sibling = root.join("project-other");
        std::fs::create_dir_all(&sibling).unwrap();

        assert!(is_within(&project, &project));
        assert!(is_within(&project.join("src"), &project));
        assert!(!is_within(&sibling, &project));
        assert!(!is_within(&root, &project));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_folder_is_an_error() {
        assert!(explorer_list_dir(r"C:\definitely\not\here\mothership".into()).is_err());
    }
}
