use mothership_common::auth::OAuthProvider;
use mothership_common::client::{
    clear_oauth_token, load_oauth_token, save_oauth_token, scan_text_files, start_oauth_login,
    verify_oauth_token, CheckpointData, MothershipClient, RiftInfo, DEFAULT_MAX_UPLOAD_BYTES,
};
use mothership_common::{Checkpoint, ClientConfig, Project};
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

fn server_url() -> String {
    ClientConfig::default().mothership_url
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

/// Starts the browser login, the same flow as `mothership auth`. Returns the URL it opened.
#[tauri::command]
pub async fn hub_auth_start(provider: String) -> Result<String, String> {
    let provider = match provider.as_str() {
        "google" => OAuthProvider::Google,
        "github" => OAuthProvider::GitHub,
        other => return Err(format!("Unknown provider '{}'", other)),
    };
    let started = start_oauth_login(&server_url(), provider).await.map_err(err)?;
    open_url(&started.auth_url, None::<String>).map_err(err)?;
    Ok(started.auth_url)
}

/// Validates a token pasted from the browser's success page, then saves it.
#[tauri::command]
pub async fn hub_auth_complete(token: String) -> Result<(), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("No token provided. Paste the token from the browser.".to_string());
    }
    if token.len() < 50 {
        return Err("Token seems too short. Make sure you copied the full token.".to_string());
    }
    verify_oauth_token(&server_url(), &token).await.map_err(err)?;
    save_oauth_token(&token).map_err(err)
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
pub async fn hub_list_projects() -> Result<Vec<Project>, String> {
    client()?.list_projects().await.map_err(err)
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

    std::fs::create_dir_all(&meta_dir).map_err(err)?;
    let metadata = serde_json::json!({
        "project_id": project.id.to_string(),
        "project_name": project.name,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "mothership_url": server_url(),
    });
    std::fs::write(
        meta_dir.join("project.json"),
        serde_json::to_string_pretty(&metadata).map_err(err)?,
    )
    .map_err(err)?;

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
    let id = client()?
        .create_rift(&name, description.as_deref(), parse_id(&project_id)?, None)
        .await
        .map_err(err)?;
    Ok(id.to_string())
}

#[tauri::command]
pub async fn hub_switch_rift(project_id: String, rift_name: String) -> Result<String, String> {
    client()?
        .switch_rift(parse_id(&project_id)?, &rift_name)
        .await
        .map_err(err)
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
