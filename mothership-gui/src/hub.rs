use mothership_common::beam::{
    beam_project, beam_wormhole_files, default_search_roots, ensure_daemon_running,
    index_local_projects, read_project_metadata, register_with_daemon, restore_files_into,
    WormholeMarker,
};
use mothership_common::client::{
    clear_oauth_token, load_oauth_token, save_oauth_token, scan_text_files, start_browser_login,
    CheckpointData, ConvergeResult, MothershipClient, RiftFileSides, RiftInfo, WormholeDetail,
    WormholeSummary, DEFAULT_MAX_UPLOAD_BYTES,
};
use mothership_common::daemon::{self, DaemonStatus, TrackedProject};
use mothership_common::merge::Choice;
use mothership_common::{Checkpoint, Project};
use serde::Serialize;
use std::path::PathBuf;
use tauri_plugin_opener::open_url;
use uuid::Uuid;

#[derive(Serialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    pub server_url: String,
}

#[derive(Serialize)]
pub struct HubHealth {
    pub server_url: String,
    pub reachable: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct CreateProjectResult {
    pub project: Project,
    pub uploaded_files: usize,
    pub skipped_too_large: usize,
    pub skipped_binary: usize,
    pub skipped_unreadable: usize,
}

#[derive(Serialize)]
pub struct HubProject {
    #[serde(flatten)]
    pub project: Project,
    /// Local folder linked to this project, or `None` when the project only exists on the server.
    pub local_path: Option<String>,
}

#[derive(Serialize)]
pub struct BeamResult {
    pub project_name: String,
    pub local_path: String,
    pub rift_id: String,
    pub downloaded: usize,
    pub skipped_unsafe: Vec<String>,
    /// Set when the folder was written but the daemon could not be told to watch it.
    pub daemon_warning: Option<String>,
}

fn server_url() -> String {
    mothership_common::client::active_server_url()
}

fn client() -> Result<MothershipClient, String> {
    MothershipClient::new(&server_url(), load_oauth_token()).map_err(|e| e.to_string())
}

fn parse_id(id: &str) -> Result<Uuid, String> {
    Uuid::parse_str(id).map_err(|e| format!("Invalid id '{}': {}", id, e))
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub fn hub_auth_status() -> AuthStatus {
    AuthStatus {
        signed_in: load_oauth_token().is_some(),
        server_url: server_url(),
    }
}

/// Runs the browser login, the same flow as `mothership auth`. Returns once the browser has finished
/// and the token is saved.
#[tauri::command]
pub async fn hub_auth_start(provider: String) -> Result<(), String> {
    if !matches!(provider.as_str(), "google" | "ghostid") {
        return Err(format!("Unknown provider '{}'", provider));
    }
    let login = start_browser_login(&server_url(), Some(&provider)).await.map_err(err)?;
    open_url(&login.auth_url, None::<String>).map_err(err)?;
    let result = login.wait_for_token().await.map_err(err)?;
    save_oauth_token(&result.token).map_err(err)
}

#[tauri::command]
pub fn hub_auth_logout() -> Result<(), String> {
    clear_oauth_token().map_err(err)
}

#[tauri::command]
pub async fn hub_health() -> Result<HubHealth, String> {
    let url = server_url();
    let result = match client() {
        Ok(c) => c.health().await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    Ok(HubHealth {
        server_url: url,
        reachable: result.is_ok(),
        error: result.err(),
    })
}

#[tauri::command]
pub async fn hub_list_projects() -> Result<Vec<HubProject>, String> {
    let projects = client()?.list_projects().await.map_err(err)?;
    let index = tauri::async_runtime::spawn_blocking(|| {
        index_local_projects(&default_search_roots())
    })
    .await
    .map_err(err)?;
    Ok(projects
        .into_iter()
        .map(|project| {
            let local_path = index.get(&project.id).map(|p| p.display().to_string());
            HubProject { project, local_path }
        })
        .collect())
}

/// Starts the background daemon if it is not already running. The Hub calls this on load.
#[tauri::command]
pub async fn hub_ensure_daemon() -> Result<(), String> {
    ensure_daemon_running().await.map_err(|e| format!("{:#}", e))
}

/// Joins a project's default rift and writes its files to disk, then registers the folder with the daemon.
/// Local projects are beamed in place. Remote projects are written to `local_dir/<project name>`.
/// Existing local files are overwritten, matching the CLI. With `rift_id`, that rift is joined instead of the default.
#[tauri::command]
pub async fn hub_beam(
    project_id: String,
    local_dir: Option<String>,
    rift_id: Option<String>,
) -> Result<BeamResult, String> {
    let id = parse_id(&project_id)?;
    let c = client()?;
    let project = c
        .list_projects()
        .await
        .map_err(err)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Project {} not found on the server", project_id))?;

    let rift_name = match rift_id.filter(|r| !r.trim().is_empty()) {
        Some(raw) => {
            let rid = parse_id(&raw)?;
            let rift = c
                .list_rifts(id)
                .await
                .map_err(err)?
                .into_iter()
                .find(|r| r.id == rid)
                .ok_or_else(|| format!("Rift {} not found in this project", raw))?;
            Some(rift.name)
        }
        None => None,
    };

    let index = tauri::async_runtime::spawn_blocking(|| {
        index_local_projects(&default_search_roots())
    })
    .await
    .map_err(err)?;

    let local_path = match index.get(&id) {
        Some(path) => path.clone(),
        None => {
            let parent = local_dir
                .filter(|d| !d.trim().is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| "Choose a folder to beam this remote project into.".to_string())?;
            if !parent.is_dir() {
                return Err(format!("Not a folder: {}", parent.display()));
            }
            let name = PathBuf::from(&project.name);
            let is_single_name = matches!(
                name.components().collect::<Vec<_>>().as_slice(),
                [std::path::Component::Normal(_)]
            );
            if !is_single_name {
                return Err(format!("Project name '{}' cannot be used as a folder name", project.name));
            }
            let target = parent.join(&name);
            if target.join(".mothership").join("project.json").exists() {
                return Err(format!(
                    "{} is already linked to another Mothership project",
                    target.display()
                ));
            }
            target
        }
    };

    let outcome = beam_project(
        &c,
        &server_url(),
        project.id,
        &project.name,
        &local_path,
        rift_name.as_deref(),
    )
    .await
    .map_err(|e| format!("{:#}", e))?;

    let daemon_warning = match ensure_daemon_running().await {
        Ok(()) => register_with_daemon(project.id, &project.name, &local_path)
            .await
            .err()
            .map(|e| format!("{:#}", e)),
        Err(e) => Some(format!("{:#}", e)),
    };

    Ok(BeamResult {
        project_name: project.name,
        local_path: outcome.local_path.display().to_string(),
        rift_id: outcome.rift_id.to_string(),
        downloaded: outcome.downloaded,
        skipped_unsafe: outcome.skipped_unsafe,
        daemon_warning,
    })
}

/// Creates a server project from a local folder and writes `.mothership/project.json`,
/// the same metadata file the CLI's `init` writes. Refuses folders that are already linked.
#[tauri::command]
pub async fn hub_create_project(
    name: String,
    description: String,
    folder: String,
) -> Result<CreateProjectResult, String> {
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Err(format!("Not a folder: {}", folder));
    }
    let meta_dir = dir.join(".mothership");
    if meta_dir.join("project.json").exists() {
        return Err(format!(
            "{} is already linked to a Mothership project (.mothership/project.json exists)",
            folder
        ));
    }

    let c = client()?;
    let project = c
        .create_gateway(&name, &description, &dir)
        .await
        .map_err(err)?;

    let max = c.max_upload_size().await.unwrap_or(DEFAULT_MAX_UPLOAD_BYTES);
    let report = scan_text_files(&dir, max).map_err(err)?;
    let uploaded_files = report.files.len();
    if !report.files.is_empty() {
        c.upload_files(project.id, report.files).await.map_err(err)?;
    }

    let _ = mothership_common::ignore::write_default_ignore_file(&dir);

    let main_rift_id = c
        .list_rifts(project.id)
        .await
        .ok()
        .and_then(|rifts| {
            rifts
                .iter()
                .find(|r| r.name == "main")
                .or_else(|| rifts.first())
                .map(|r| r.id)
        });

    std::fs::create_dir_all(&meta_dir).map_err(err)?;
    let mut metadata = serde_json::json!({
        "project_id": project.id.to_string(),
        "project_name": project.name,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "mothership_url": server_url(),
        "current_rift": "main",
    });
    if let Some(rid) = main_rift_id {
        metadata["rift_id"] = serde_json::json!(rid.to_string());
    }
    std::fs::write(
        meta_dir.join("project.json"),
        serde_json::to_string_pretty(&metadata).map_err(err)?,
    )
    .map_err(err)?;

    if ensure_daemon_running().await.is_ok() {
        let _ = register_with_daemon(project.id, &project.name, &dir).await;
    }

    Ok(CreateProjectResult {
        project,
        uploaded_files,
        skipped_too_large: report.skipped_too_large.len(),
        skipped_binary: report.skipped_binary.len(),
        skipped_unreadable: report.skipped_unreadable.len(),
    })
}

/// Deletes the server copy only. The local folder and its `.mothership` directory are untouched.
#[tauri::command]
pub async fn hub_delete_project(project_id: String) -> Result<(), String> {
    client()?
        .delete_project(parse_id(&project_id)?)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_list_rifts(project_id: String) -> Result<Vec<RiftInfo>, String> {
    client()?
        .list_rifts(parse_id(&project_id)?)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_create_rift(
    project_id: String,
    name: String,
    description: Option<String>,
) -> Result<String, String> {
    let project = parse_id(&project_id)?;
    let c = client()?;

    // Fork from the local folder's current rift when available, or fall back to main/first rift on server
    let local_rift = tauri::async_runtime::spawn_blocking(move || {
        index_local_projects(&default_search_roots())
            .get(&project)
            .and_then(|p| read_project_metadata(p))
            .and_then(|m| m.rift_id)
            .and_then(|id| Uuid::parse_str(&id).ok())
    })
    .await
    .ok()
    .flatten();

    let fork_from = match local_rift {
        Some(id) => Some(id),
        None => c.list_rifts(project).await.ok().and_then(|rifts| {
            rifts
                .iter()
                .find(|r| r.name == "main")
                .or_else(|| rifts.iter().find(|r| !r.is_conflict_rift))
                .or_else(|| rifts.first())
                .map(|r| r.id)
        }),
    };

    let id = c
        .create_rift(&name, description.as_deref(), project, fork_from, false)
        .await
        .map_err(err)?;
    Ok(id.to_string())
}

/// Points a project's local metadata at a rift. Other keys written by the CLI are kept.
fn update_local_rift(folder: &std::path::Path, rift_name: &str, rift_id: Option<Uuid>) -> Result<(), String> {
    let file = folder.join(".mothership").join("project.json");
    let content = std::fs::read_to_string(&file)
        .map_err(|e| format!("reading {}: {e}", file.display()))?;
    let mut meta: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("parsing {}: {e}", file.display()))?;
    let obj = meta
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", file.display()))?;
    obj.insert("current_rift".into(), serde_json::json!(rift_name));
    if let Some(id) = rift_id {
        obj.insert("rift_id".into(), serde_json::json!(id.to_string()));
    }
    let out = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
    std::fs::write(&file, out).map_err(|e| format!("writing {}: {e}", file.display()))
}

/// Mirrors the CLI's `switch_to_rift`: switch on the server, update the local
/// project.json, then tell the daemon to reconnect its WebSocket to the new rift.
#[tauri::command]
pub async fn hub_switch_rift(project_id: String, rift_name: String) -> Result<String, String> {
    let project = parse_id(&project_id)?;
    let client = client()?;
    client.switch_rift(project, &rift_name).await.map_err(err)?;

    let rift_id = client
        .list_rifts(project)
        .await
        .map_err(err)?
        .into_iter()
        .find(|r| r.name == rift_name)
        .map(|r| r.id);

    let _ = ensure_daemon_running().await;

    // Prefer the daemon's record of the folder, then fall back to scanning for a linked folder
    let (tracked_path, daemon_error) = match daemon::tracked_projects().await {
        Ok(list) => (
            list.into_iter()
                .find(|p| p.project_id == project)
                .map(|p| PathBuf::from(p.project_path)),
            None,
        ),
        Err(e) => (None, Some(e.to_string())),
    };
    let is_tracked = tracked_path.is_some();
    let folder = match tracked_path {
        Some(path) => Some(path),
        None => tauri::async_runtime::spawn_blocking(|| index_local_projects(&default_search_roots()))
            .await
            .map_err(err)?
            .get(&project)
            .cloned(),
    };

    let Some(folder) = folder else {
        return Ok(format!(
            "Switched server to rift \"{rift_name}\". No local folder was found for this project, so project.json was not changed."
        ));
    };
    update_local_rift(&folder, &rift_name, rift_id)?;

    if let Some(e) = daemon_error {
        return Ok(format!(
            "Switched to rift \"{rift_name}\" and updated project.json, but the daemon is not reachable ({e}). Sync will use the new rift after the daemon restarts."
        ));
    }
    if !is_tracked {
        let project_name = read_project_metadata(&folder)
            .map(|m| m.project_name)
            .unwrap_or_else(|| project.to_string());
        match register_with_daemon(project, &project_name, &folder).await {
            Ok(()) => {
                return Ok(format!(
                    "Switched to rift \"{rift_name}\". Local project and daemon sync now use it."
                ))
            }
            Err(e) => {
                return Ok(format!(
                    "Switched to rift \"{rift_name}\" and updated project.json, but could not register with daemon ({e:#})."
                ))
            }
        }
    }

    match daemon::reconnect_project(project).await {
        Ok(()) => Ok(format!(
            "Switched to rift \"{rift_name}\". Local project and daemon sync now use it."
        )),
        Err(e) => Ok(format!(
            "Switched to rift \"{rift_name}\" and updated project.json, but the daemon reconnect failed ({e})."
        )),
    }
}

#[tauri::command]
pub async fn hub_create_checkpoint(
    project_id: String,
    message: Option<String>,
) -> Result<CheckpointData, String> {
    client()?
        .create_checkpoint(parse_id(&project_id)?, message.as_deref())
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_checkpoint_history(project_id: String) -> Result<Vec<Checkpoint>, String> {
    client()?
        .checkpoint_history(parse_id(&project_id)?)
        .await
        .map_err(err)
}

#[derive(Serialize)]
pub struct RestoreResult {
    pub local_path: String,
    pub written: usize,
    pub skipped_unsafe: Vec<String>,
}

/// Overwrites matching files in the project's local folder with the checkpoint's contents.
#[tauri::command]
pub async fn hub_restore_checkpoint(
    project_id: String,
    checkpoint_id: String,
) -> Result<RestoreResult, String> {
    let project = parse_id(&project_id)?;
    let checkpoint = parse_id(&checkpoint_id)?;

    let local_path = index_local_projects(&default_search_roots())
        .get(&project)
        .cloned()
        .ok_or_else(|| "This project has no local folder. Beam it first.".to_string())?;

    let data = client()?
        .restore_checkpoint(project, checkpoint)
        .await
        .map_err(err)?;

    let outcome = restore_files_into(&local_path, &data.files).map_err(|e| format!("{:#}", e))?;
    Ok(RestoreResult {
        local_path: local_path.display().to_string(),
        written: outcome.written,
        skipped_unsafe: outcome.skipped_unsafe,
    })
}

#[derive(Serialize)]
pub struct ServerInfo {
    pub server_url: String,
    pub reachable: bool,
    pub error: Option<String>,
    pub projects: usize,
    pub rifts: usize,
    pub active_wormholes: usize,
}

/// Health plus counts across the projects the user can see.
#[tauri::command]
pub async fn hub_server_info() -> Result<ServerInfo, String> {
    let c = client()?;
    let mut info = ServerInfo {
        server_url: server_url(),
        reachable: false,
        error: None,
        projects: 0,
        rifts: 0,
        active_wormholes: 0,
    };
    if let Err(e) = c.health().await {
        info.error = Some(e.to_string());
        return Ok(info);
    }
    info.reachable = true;
    let projects = c.list_projects().await.map_err(err)?;
    info.projects = projects.len();
    for project in &projects {
        info.rifts += c.list_rifts(project.id).await.map_err(err)?.len();
        info.active_wormholes += c
            .list_wormholes(project.id)
            .await
            .map_err(err)?
            .iter()
            .filter(|w| w.status == "active")
            .count();
    }
    Ok(info)
}

#[derive(Serialize)]
pub struct DaemonPanel {
    pub running: bool,
    pub status: Option<DaemonStatus>,
    pub tracked: Vec<TrackedProject>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn hub_daemon_status() -> DaemonPanel {
    if !daemon::is_running().await {
        return DaemonPanel {
            running: false,
            status: None,
            tracked: Vec::new(),
            error: None,
        };
    }
    let (status, error) = match daemon::status().await {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let tracked = daemon::tracked_projects().await.unwrap_or_default();
    DaemonPanel {
        running: true,
        status,
        tracked,
        error,
    }
}

#[tauri::command]
pub async fn hub_daemon_start() -> Result<(), String> {
    ensure_daemon_running().await.map_err(err)
}

#[tauri::command]
pub async fn hub_daemon_stop() -> Result<(), String> {
    daemon::stop().await.map_err(err)
}

#[tauri::command]
pub async fn hub_daemon_restart() -> Result<(), String> {
    daemon::stop().await.map_err(err)?;
    ensure_daemon_running().await.map_err(err)
}

#[tauri::command]
pub async fn hub_list_wormholes(project_id: String) -> Result<Vec<WormholeSummary>, String> {
    client()?
        .list_wormholes(parse_id(&project_id)?)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_create_wormhole(
    project_id: String,
    name: String,
    rift_ids: Vec<String>,
) -> Result<WormholeSummary, String> {
    let rift_ids = rift_ids
        .iter()
        .map(|id| parse_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    client()?
        .create_wormhole(parse_id(&project_id)?, &name, &rift_ids, false)
        .await
        .map_err(err)
}

/// Full wormhole state, including each side and its conflicts. The UI polls this to watch a wormhole.
#[tauri::command]
pub async fn hub_show_wormhole(wormhole_id: String) -> Result<WormholeDetail, String> {
    client()?
        .get_wormhole(parse_id(&wormhole_id)?)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_resolve_conflict(
    wormhole_id: String,
    path: String,
    conflict_id: String,
    choice: Choice,
) -> Result<WormholeDetail, String> {
    client()?
        .resolve_wormhole_conflict(parse_id(&wormhole_id)?, &path, &conflict_id, choice)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_converge_wormhole(
    wormhole_id: String,
    name: Option<String>,
) -> Result<ConvergeResult, String> {
    client()?
        .converge_wormhole(parse_id(&wormhole_id)?, name.as_deref())
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hub_dissolve_wormhole(wormhole_id: String) -> Result<String, String> {
    let id = client()?
        .dissolve_wormhole(parse_id(&wormhole_id)?)
        .await
        .map_err(err)?;
    Ok(id.to_string())
}

#[tauri::command]
pub async fn hub_delete_wormhole(wormhole_id: String) -> Result<String, String> {
    let id = client()?
        .delete_wormhole(parse_id(&wormhole_id)?)
        .await
        .map_err(err)?;
    Ok(id.to_string())
}

#[tauri::command]
pub async fn hub_toggle_wormhole_rift(
    wormhole_id: String,
    rift_id: String,
    enabled: Option<bool>,
) -> Result<WormholeDetail, String> {
    client()?
        .toggle_wormhole_rift(parse_id(&wormhole_id)?, parse_id(&rift_id)?, enabled)
        .await
        .map_err(err)
}

/// Per-side text for files that differ between a wormhole's rifts. Works while conflicts remain.
#[tauri::command]
pub async fn hub_wormhole_rift_files(wormhole_id: String) -> Result<Vec<RiftFileSides>, String> {
    client()?
        .wormhole_rift_files(parse_id(&wormhole_id)?)
        .await
        .map_err(err)
}

#[derive(Serialize)]
pub struct WormholeBeamResult {
    pub folder: String,
    pub written: usize,
    pub skipped_unsafe: Vec<String>,
    /// Set when the snapshot was written but the daemon could not be told to clean it up later.
    pub daemon_warning: Option<String>,
}

/// Writes the merged files of a converged-ready wormhole into `parent_dir/<wormhole name>`.
/// The folder is registered with the daemon so it is removed once the wormhole is dissolved or converted.
#[tauri::command]
pub async fn hub_beam_wormhole(
    wormhole_id: String,
    parent_dir: String,
) -> Result<WormholeBeamResult, String> {
    let c = client()?;
    let id = parse_id(&wormhole_id)?;
    let detail = c.get_wormhole(id).await.map_err(err)?;
    let files = c.wormhole_files(id).await.map_err(err)?;
    let marker = WormholeMarker {
        wormhole_id: detail.summary.id.to_string(),
        wormhole_name: detail.summary.name.clone(),
        project_id: detail.summary.project_id.to_string(),
    };
    let folder = PathBuf::from(&parent_dir).join(&detail.summary.name);
    let pairs: Vec<(String, String)> = files.into_iter().map(|f| (f.path, f.content)).collect();
    let outcome = beam_wormhole_files(&folder, &marker, &pairs).map_err(err)?;

    let daemon_warning = match daemon::register_snapshot(&outcome.folder, id).await {
        Ok(()) => None,
        Err(e) => Some(format!(
            "Snapshot written, but the daemon could not register it for cleanup: {:#}",
            e
        )),
    };

    Ok(WormholeBeamResult {
        folder: outcome.folder.display().to_string(),
        written: outcome.written,
        skipped_unsafe: outcome.skipped_unsafe,
        daemon_warning,
    })
}

#[derive(Serialize)]
pub struct ActiveRift {
    /// Local folder of the gateway, or `None` when it only exists on the server.
    pub local_path: Option<String>,
    /// Rift recorded in the folder's `.mothership/project.json`, or `None` when not beamed into one.
    pub rift_id: Option<String>,
    pub rift_name: Option<String>,
}

/// Reports which rift a gateway's local folder is beamed into, for the top bar.
#[tauri::command]
pub async fn hub_active_rift(project_id: String) -> Result<ActiveRift, String> {
    let id = parse_id(&project_id)?;
    let index = tauri::async_runtime::spawn_blocking(|| {
        index_local_projects(&default_search_roots())
    })
    .await
    .map_err(err)?;

    let Some(local_path) = index.get(&id).cloned() else {
        return Ok(ActiveRift { local_path: None, rift_id: None, rift_name: None });
    };
    let rift_id = read_project_metadata(&local_path).and_then(|m| m.rift_id);
    let rift_name = match &rift_id {
        Some(raw) => {
            let rid = parse_id(raw)?;
            client()?
                .list_rifts(id)
                .await
                .ok()
                .and_then(|rifts| rifts.into_iter().find(|r| r.id == rid).map(|r| r.name))
        }
        None => None,
    };
    Ok(ActiveRift {
        local_path: Some(local_path.display().to_string()),
        rift_id,
        rift_name,
    })
}


