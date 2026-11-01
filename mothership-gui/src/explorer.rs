//! Directory listing for the file explorer. Lists one level at a time so the
//! tree can load folders lazily when they are expanded.

use std::path::Path;

use serde::Serialize;

/// Folders that are never useful to browse in the editor.
const HIDDEN_NAMES: &[&str] = &[".git", "node_modules", "target"];

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
    fn missing_folder_is_an_error() {
        assert!(explorer_list_dir(r"C:\definitely\not\here\mothership".into()).is_err());
    }
}
