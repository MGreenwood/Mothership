//! Shared beam logic: find local copies of projects, join a rift, download its files,
//! and register the folder with the daemon. Used by the GUI; the CLI keeps its own copy.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::client::MothershipClient;
use crate::protocol::SyncMessage;

pub const DAEMON_URL: &str = "http://localhost:7525";

const METADATA_DIR: &str = ".mothership";
const METADATA_FILE: &str = "project.json";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Project folders are found up to two levels below a search root (`root/project` or `root/group/project`).
const INDEX_MAX_DEPTH: usize = 3;
const PRUNED_DIRS: &[&str] = &["node_modules", "target", ".git", "AppData"];

/// Same on-disk format the CLI writes to `.mothership/project.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub project_id: String,
    pub project_name: String,
    pub created_at: String,
    pub mothership_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rift_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BeamOutcome {
    pub local_path: PathBuf,
    pub rift_id: Uuid,
    pub downloaded: usize,
    /// Server paths rejected as unsafe (absolute, `..`, or inside `.mothership`).
    pub skipped_unsafe: Vec<String>,
}

/// Folders searched for local project copies: `~/mothership` and `~`.
pub fn default_search_roots() -> Vec<PathBuf> {
    match dirs::home_dir() {
        Some(home) => vec![home.join("mothership"), home],
        None => Vec::new(),
    }
}

/// Maps project id to the local folder that holds its `.mothership/project.json`.
/// The first root that contains a project wins.
pub fn index_local_projects(roots: &[PathBuf]) -> HashMap<Uuid, PathBuf> {
    let mut index = HashMap::new();
    for root in roots {
        let walker = walkdir::WalkDir::new(root)
            .max_depth(INDEX_MAX_DEPTH)
            .into_iter()
            .filter_entry(|e| !is_pruned(e));
        for entry in walker.filter_map(|e| e.ok()) {
            if entry.file_name() != METADATA_FILE {
                continue;
            }
            let Some(project_dir) = entry
                .path()
                .parent()
                .filter(|p| p.file_name().map_or(false, |n| n == METADATA_DIR))
                .and_then(|p| p.parent())
            else {
                continue;
            };
            let Some(meta) = read_project_metadata(project_dir) else {
                continue;
            };
            if let Ok(id) = Uuid::parse_str(&meta.project_id) {
                index.entry(id).or_insert_with(|| project_dir.to_path_buf());
            }
        }
    }
    index
}

fn is_pruned(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .map_or(false, |name| PRUNED_DIRS.iter().any(|p| p.eq_ignore_ascii_case(name)))
}

pub fn read_project_metadata(project_dir: &Path) -> Option<ProjectMetadata> {
    let raw = fs::read_to_string(project_dir.join(METADATA_DIR).join(METADATA_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_project_metadata(project_dir: &Path, meta: &ProjectMetadata) -> Result<()> {
    let dir = project_dir.join(METADATA_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create {}", dir.display()))?;
    fs::write(dir.join(METADATA_FILE), serde_json::to_string_pretty(meta)?)
        .context("Failed to write project.json")?;
    Ok(())
}

/// Accepts only plain relative paths made of normal components, outside `.mothership`.
pub fn safe_relative_path(raw: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for (i, component) in raw.components().enumerate() {
        match component {
            Component::Normal(part) => {
                if i == 0 && part == METADATA_DIR {
                    return None;
                }
                out.push(part);
            }
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Joins a rift on the server and writes the project into `local_path`.
/// Server files overwrite any existing local copy, matching the CLI.
pub async fn beam_project(
    client: &MothershipClient,
    mothership_url: &str,
    project_id: Uuid,
    project_name: &str,
    local_path: &Path,
    rift_name: Option<&str>,
) -> Result<BeamOutcome> {
    let resp = client.beam(project_id, rift_name).await?;

    fs::create_dir_all(local_path)
        .with_context(|| format!("Failed to create {}", local_path.display()))?;

    let mut outcome = BeamOutcome {
        local_path: local_path.to_path_buf(),
        rift_id: resp.rift_id,
        downloaded: 0,
        skipped_unsafe: Vec::new(),
    };

    if resp.initial_sync_required {
        let token = client
            .token()
            .ok_or_else(|| anyhow!("Not signed in. Sign in before beaming a project."))?;
        let files = download_rift_files(token, &resp.websocket_url, resp.rift_id).await?;

        for (raw_path, content) in files {
            let Some(rel) = safe_relative_path(&raw_path) else {
                outcome.skipped_unsafe.push(raw_path.display().to_string());
                continue;
            };
            let target = local_path.join(&rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            fs::write(&target, content)
                .with_context(|| format!("Failed to write {}", target.display()))?;
            outcome.downloaded += 1;
        }
    }

    write_project_metadata(
        local_path,
        &ProjectMetadata {
            project_id: project_id.to_string(),
            project_name: project_name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            mothership_url: mothership_url.to_string(),
            rift_id: Some(resp.rift_id.to_string()),
        },
    )?;

    Ok(outcome)
}

/// Opens the sync WebSocket, joins the rift, and returns every file the server sends back.
async fn download_rift_files(
    token: &str,
    websocket_url: &str,
    rift_id: Uuid,
) -> Result<Vec<(PathBuf, String)>> {
    let mut url = reqwest::Url::parse(websocket_url)
        .with_context(|| format!("Invalid sync URL: {websocket_url}"))?;
    url.query_pairs_mut().append_pair("token", token);

    tokio::time::timeout(DOWNLOAD_TIMEOUT, async {
        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .context("Failed to connect to sync server")?;
        let (mut tx, mut rx) = ws.split();

        let join = serde_json::to_string(&SyncMessage::JoinRift {
            rift_id,
            last_checkpoint: None,
        })?;
        tx.send(Message::Text(join))
            .await
            .context("Failed to send JoinRift")?;

        let files: Vec<(PathBuf, String)> = loop {
            match rx.next().await {
                Some(Ok(Message::Text(text))) => {
                    let Ok(msg) = serde_json::from_str::<SyncMessage>(&text) else {
                        continue;
                    };
                    match msg {
                        SyncMessage::SyncData { files, .. } => {
                            break files.into_iter().map(|f| (f.path, f.content)).collect();
                        }
                        SyncMessage::RiftJoined { current_files, .. } => {
                            break current_files.into_iter().collect();
                        }
                        SyncMessage::Error { message, .. } => bail!("Sync error: {message}"),
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    bail!("Sync connection closed before files arrived")
                }
                Some(Err(e)) => bail!("WebSocket error: {e}"),
                Some(Ok(_)) => {}
            }
        };

        let close = CloseFrame {
            code: CloseCode::Normal,
            reason: "Initial sync completed".into(),
        };
        let _ = tx.send(Message::Close(Some(close))).await;
        Ok(files)
    })
    .await
    .map_err(|_| anyhow!("Timed out downloading project files"))?
}

pub(crate) async fn daemon_healthy(http: &reqwest::Client) -> bool {
    matches!(
        http.get(format!("{DAEMON_URL}/health")).send().await,
        Ok(resp) if resp.status().is_success()
    )
}

/// Starts `mothership-daemon` if it is not already answering on port 7525.
/// The daemon is looked for next to the running executable, then in the workspace `target` folders.
/// It is detached so it keeps running after the GUI closes.
pub async fn ensure_daemon_running() -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    if daemon_healthy(&http).await {
        return Ok(());
    }

    let exe_name = if cfg!(windows) { "mothership-daemon.exe" } else { "mothership-daemon" };
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("Could not determine the GUI executable folder"))?;
    let candidates = [
        exe_dir.join(exe_name),
        exe_dir.join("..").join("debug").join(exe_name),
        exe_dir.join("..").join("release").join(exe_name),
    ];
    let daemon = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            anyhow!("Could not find {exe_name}. Build it with `cargo build -p mothership-daemon`.")
        })?;

    let mut cmd = std::process::Command::new(daemon);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .with_context(|| format!("Failed to start {}", daemon.display()))?;

    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if daemon_healthy(&http).await {
            return Ok(());
        }
    }
    bail!("Started {} but it did not answer on {DAEMON_URL} within 10 seconds", daemon.display())
}

/// Tells the local daemon to start watching the folder. Errors mean the daemon is unreachable or rejected it.
pub async fn register_with_daemon(
    project_id: Uuid,
    project_name: &str,
    project_path: &Path,
) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = serde_json::json!({
        "project_id": project_id,
        "project_name": project_name,
        "project_path": project_path,
    });
    let resp = http
        .post(format!("{DAEMON_URL}/projects/add"))
        .json(&body)
        .send()
        .await
        .context("Daemon is not reachable")?;
    if !resp.status().is_success() {
        bail!("Daemon rejected the project (HTTP {})", resp.status());
    }
    Ok(())
}

/// Marker written into a beamed wormhole folder, matching the CLI's `.mothership/wormhole.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeMarker {
    pub wormhole_id: String,
    pub wormhole_name: String,
    pub project_id: String,
}

#[derive(Debug, Clone)]
pub struct WormholeBeamOutcome {
    pub folder: PathBuf,
    pub written: usize,
    /// Merged paths rejected as unsafe (absolute, `..`, or inside `.mothership`).
    pub skipped_unsafe: Vec<String>,
}

/// Writes merged wormhole files into `folder`.
///
/// The folder must be new, or must already hold a beam of the same wormhole. Anything else is
/// refused so that a folder from another wormhole or project is never overwritten.
pub fn beam_wormhole_files(
    folder: &Path,
    marker: &WormholeMarker,
    files: &[(String, String)],
) -> Result<WormholeBeamOutcome> {
    let marker_path = folder.join(METADATA_DIR).join("wormhole.json");
    if folder.exists() {
        let existing: Option<WormholeMarker> = fs::read_to_string(&marker_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        match existing {
            Some(m) if m.wormhole_id == marker.wormhole_id => {}
            _ => bail!(
                "{} already exists and was not beamed from this wormhole; choose another folder",
                folder.display()
            ),
        }
    }

    let mut written = 0;
    let mut skipped_unsafe = Vec::new();
    for (path, content) in files {
        let Some(relative) = safe_relative_path(Path::new(path)) else {
            skipped_unsafe.push(path.clone());
            continue;
        };
        let target = folder.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(&target, content)
            .with_context(|| format!("Failed to write {}", target.display()))?;
        written += 1;
    }

    fs::create_dir_all(folder.join(METADATA_DIR))?;
    fs::write(&marker_path, serde_json::to_string_pretty(marker)?)
        .context("Failed to write wormhole marker")?;

    Ok(WormholeBeamOutcome {
        folder: folder.to_path_buf(),
        written,
        skipped_unsafe,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mothership-beam-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_path_accepts_nested_relative() {
        assert_eq!(
            safe_relative_path(Path::new("src/main.rs")),
            Some(PathBuf::from("src").join("main.rs"))
        );
    }

    #[test]
    fn safe_path_rejects_escape_and_metadata() {
        assert_eq!(safe_relative_path(Path::new("../evil.txt")), None);
        assert_eq!(safe_relative_path(Path::new("a/../../evil.txt")), None);
        assert_eq!(safe_relative_path(Path::new(".mothership/project.json")), None);
        assert_eq!(safe_relative_path(Path::new("")), None);
    }

    #[test]
    fn index_finds_project_and_skips_pruned_dirs() {
        let root = temp_dir("index");
        let id = Uuid::new_v4();
        let project = root.join("Demo");
        write_project_metadata(
            &project,
            &ProjectMetadata {
                project_id: id.to_string(),
                project_name: "Demo".into(),
                created_at: "now".into(),
                mothership_url: "http://localhost:7523".into(),
                rift_id: None,
            },
        )
        .unwrap();

        let hidden = root.join("node_modules").join("pkg");
        let hidden_id = Uuid::new_v4();
        write_project_metadata(
            &hidden,
            &ProjectMetadata {
                project_id: hidden_id.to_string(),
                project_name: "Hidden".into(),
                created_at: "now".into(),
                mothership_url: "http://localhost:7523".into(),
                rift_id: None,
            },
        )
        .unwrap();

        let index = index_local_projects(std::slice::from_ref(&root));
        assert_eq!(index.get(&id), Some(&project));
        assert!(!index.contains_key(&hidden_id));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn wormhole_beam_refuses_foreign_folder_and_allows_same_wormhole() {
        let dir = temp_dir("wormhole");
        let marker = WormholeMarker {
            wormhole_id: "wh-1".into(),
            wormhole_name: "wh".into(),
            project_id: "p".into(),
        };
        let files = vec![
            ("src/a.txt".to_string(), "one".to_string()),
            ("../escape.txt".to_string(), "bad".to_string()),
        ];

        // Beam into a folder that does not exist yet.
        let folder = dir.join("wh");
        let first = beam_wormhole_files(&folder, &marker, &files).unwrap();
        assert_eq!(first.written, 1);
        assert_eq!(first.skipped_unsafe, vec!["../escape.txt".to_string()]);
        assert_eq!(fs::read_to_string(folder.join("src").join("a.txt")).unwrap(), "one");

        // Re-beaming the same wormhole into its own folder is allowed.
        assert!(beam_wormhole_files(&folder, &marker, &files).is_ok());

        // A different wormhole must not write into this folder.
        let other = WormholeMarker {
            wormhole_id: "wh-2".into(),
            ..marker.clone()
        };
        assert!(beam_wormhole_files(&folder, &other, &files).is_err());

        // An existing folder with no marker is refused too.
        let unmarked = dir.join("unmarked");
        fs::create_dir_all(&unmarked).unwrap();
        assert!(beam_wormhole_files(&unmarked, &marker, &files).is_err());

        fs::remove_dir_all(&dir).unwrap();
    }
}
