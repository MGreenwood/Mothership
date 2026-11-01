//! Shared HTTP client for the Mothership server API.
//!
//! The CLI and the GUI both use this module so that project, rift, and
//! checkpoint operations go through one implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder, Response, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{OAuthProvider, OAuthRequest, OAuthResponse, OAuthSource};
use crate::merge::Choice;
use crate::protocol::{ApiResponse, BeamRequest, BeamResponse};
use crate::{Checkpoint, Project};

/// Fallback when the server does not report its upload limit.
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 5 * 1024 * 1024;
/// Number of files sent per `POST /projects/:id/files` request.
pub const UPLOAD_BATCH_SIZE: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiftInfo {
    pub id: Uuid,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub author: String,
    pub file_count: usize,
    pub is_conflict_rift: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointData {
    pub checkpoint_id: Uuid,
    pub file_count: usize,
}

/// A wormhole as listed by `GET /api/wormholes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeSummary {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub rift_ids: Vec<Uuid>,
    pub converted_rift_id: Option<Uuid>,
    pub unresolved_conflicts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeSide {
    /// Index used by `Choice::Side` and by each conflict's `sides` list
    pub index: usize,
    pub rift_id: Uuid,
    pub rift_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeConflict {
    pub id: String,
    pub base_start: usize,
    pub base_end: usize,
    /// One entry per side; each side holds the lines it contributes
    pub sides: Vec<Vec<String>>,
    pub fingerprint: String,
    pub resolved: bool,
    pub choice: Option<Choice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeFileConflicts {
    pub path: String,
    pub conflicts: Vec<WormholeConflict>,
}

/// A wormhole with its sides and current conflicts (`GET /api/wormholes/:id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeDetail {
    #[serde(flatten)]
    pub summary: WormholeSummary,
    pub sides: Vec<WormholeSide>,
    pub files: Vec<WormholeFileConflicts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergeResult {
    pub rift_id: Uuid,
    pub rift_name: String,
    pub files_written: usize,
    pub files_removed: usize,
}

/// One merged file from `GET /api/wormholes/:id/files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeFile {
    pub path: String,
    pub content: String,
}

/// Result of scanning a directory for uploadable text files.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: HashMap<PathBuf, String>,
    pub skipped_too_large: Vec<PathBuf>,
    pub skipped_binary: Vec<PathBuf>,
    pub skipped_unreadable: Vec<PathBuf>,
}

/// Credentials file written by `mothership login`.
#[derive(Debug, Deserialize)]
struct StoredCredentials {
    access_token: String,
}

/// Reads the OAuth access token from `<config_dir>/mothership/credentials.json`.
pub fn load_oauth_token() -> Option<String> {
    let path = dirs::config_dir()?.join("mothership").join("credentials.json");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let creds: StoredCredentials = serde_json::from_str(&content).ok()?;
    Some(creds.access_token)
}

/// Starts the browser login flow, same as `mothership auth`. The caller opens `auth_url`.
pub async fn start_oauth_login(base_url: &str, provider: OAuthProvider) -> Result<OAuthResponse> {
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string());
    let request = OAuthRequest {
        provider,
        machine_id: Uuid::new_v4().to_string(),
        machine_name: format!("{}-mothership-gui", hostname),
        platform: std::env::consts::OS.to_string(),
        hostname,
        source: OAuthSource::CLI,
        callback_url: None,
    };
    let url = format!("{}/auth/oauth/start", base_url.trim_end_matches('/'));
    let body: ApiResponse<OAuthResponse> = Client::new()
        .post(url)
        .json(&request)
        .send()
        .await?
        .json()
        .await?;
    if !body.success {
        return Err(anyhow!(body.error.unwrap_or_else(|| "Unknown error".to_string())));
    }
    body.data.ok_or_else(|| anyhow!("Server returned no OAuth data"))
}

/// Confirms a pasted token with the server before it is saved.
pub async fn verify_oauth_token(base_url: &str, token: &str) -> Result<()> {
    let url = format!("{}/auth/check", base_url.trim_end_matches('/'));
    let response = Client::new().get(url).bearer_auth(token).send().await?;
    if !response.status().is_success() {
        return Err(anyhow!("Invalid token. Please try the authentication process again."));
    }
    Ok(())
}

/// Path of the shared credentials file used by the CLI, GUI, and daemon.
fn credentials_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| anyhow!("Could not find config directory"))?
        .join("mothership")
        .join("credentials.json"))
}

/// Writes the token in the same format as the CLI's `save_credentials`.
pub fn save_oauth_token(token: &str) -> Result<()> {
    #[derive(Serialize)]
    struct CredentialsFile<'a> {
        access_token: &'a str,
        user_email: Option<String>,
        user_name: Option<String>,
        stored_at: String,
    }
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating credentials directory")?;
    }
    let file = CredentialsFile {
        access_token: token,
        user_email: None,
        user_name: None,
        stored_at: Utc::now().to_rfc3339(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?).context("writing credentials")?;
    Ok(())
}

/// Removes the stored token. Missing file is not an error.
pub fn clear_oauth_token() -> Result<()> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(path).context("removing credentials")?;
    }
    Ok(())
}

/// Returns true for paths the initial upload should skip.
pub fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    if path_str.contains(".mothership") {
        return true;
    }

    let ignore_patterns = [
        ".git", ".svn", ".hg",
        "node_modules", "target", "build", "dist",
        ".DS_Store", "Thumbs.db",
        ".env", ".env.local", ".env.production",
        "*.log", "*.tmp", "*.temp",
    ];

    for pattern in &ignore_patterns {
        if pattern.contains('*') {
            let suffix = pattern.replace('*', "");
            if path_str.ends_with(&suffix) {
                return true;
            }
        } else if path_str.contains(pattern) {
            return true;
        }
    }

    false
}

/// Walks `dir` and collects UTF-8 text files no larger than `max_size` bytes.
pub fn scan_text_files(dir: &Path, max_size: u64) -> Result<ScanReport> {
    let mut report = ScanReport::default();

    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_ignore_path(e.path()))
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(dir) else {
            continue;
        };
        let relative = relative.to_path_buf();

        match std::fs::read(path) {
            Ok(bytes) => {
                if bytes.len() as u64 > max_size {
                    report.skipped_too_large.push(relative);
                    continue;
                }
                match String::from_utf8(bytes) {
                    Ok(content) => {
                        report.files.insert(relative, content);
                    }
                    Err(_) => report.skipped_binary.push(relative),
                }
            }
            Err(_) => report.skipped_unreadable.push(relative),
        }
    }

    Ok(report)
}

#[derive(Serialize)]
struct CreateGatewayRequest<'a> {
    name: &'a str,
    description: &'a str,
    project_path: &'a Path,
}

#[derive(Serialize)]
struct UploadFilesRequest {
    project_id: Uuid,
    files: HashMap<PathBuf, String>,
}

#[derive(Serialize)]
struct CreateRiftRequest<'a> {
    name: &'a str,
    description: Option<&'a str>,
    project_id: Option<Uuid>,
    fork_from: Option<Uuid>,
}

#[derive(Serialize)]
struct SwitchRiftRequest<'a> {
    rift_name: &'a str,
    project_id: Option<Uuid>,
}

#[derive(Serialize)]
struct CreateCheckpointRequest<'a> {
    message: Option<&'a str>,
    timestamp: DateTime<Utc>,
}

/// Client for the Mothership server REST API.
pub struct MothershipClient {
    http: Client,
    base: Url,
    token: Option<String>,
}

impl MothershipClient {
    pub fn new(base_url: &str, token: Option<String>) -> Result<Self> {
        let base = Url::parse(base_url.trim_end_matches('/')).context("invalid server URL")?;
        Ok(Self {
            http: Client::new(),
            base,
            token,
        })
    }

    /// Builds a URL from path segments. Each segment is percent-encoded.
    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        {
            let mut parts = url
                .path_segments_mut()
                .map_err(|_| anyhow!("server URL cannot be used as a base"))?;
            parts.pop_if_empty();
            for segment in segments {
                parts.push(segment);
            }
        }
        Ok(url)
    }

    fn authed(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn ensure_success(response: Response) -> Result<Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(anyhow!("server returned {}: {}", status, body))
    }

    async fn get_api<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<T> {
        let url = self.endpoint(segments)?;
        let response = self.authed(self.http.get(url)).send().await?;
        let response = Self::ensure_success(response).await?;
        Ok(response.json::<T>().await?)
    }

    async fn post_api<B: Serialize, T: DeserializeOwned>(&self, segments: &[&str], body: &B) -> Result<T> {
        let url = self.endpoint(segments)?;
        let response = self.authed(self.http.post(url).json(body)).send().await?;
        let response = Self::ensure_success(response).await?;
        Ok(response.json::<T>().await?)
    }

    /// Unwraps an `ApiResponse`, turning `success: false` into an error.
    fn unwrap_api<T>(response: ApiResponse<T>) -> Result<T> {
        if !response.success {
            return Err(anyhow!(
                "{}",
                response.error.unwrap_or_else(|| "unknown server error".to_string())
            ));
        }
        response
            .data
            .ok_or_else(|| anyhow!("server response had no data"))
    }

    /// Checks that the server answers `GET /health`.
    pub async fn health(&self) -> Result<()> {
        let url = self.endpoint(&["health"])?;
        let response = self.http.get(url).send().await?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    /// The bearer token this client sends, if any. Needed for the sync WebSocket.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let resp: ApiResponse<Vec<Project>> = self.get_api(&["projects"]).await?;
        Self::unwrap_api(resp)
    }

    /// Joins a rift for a project (`POST /projects/{id}/beam`). `rift_name: None` uses the default rift.
    pub async fn beam(&self, project_id: Uuid, rift_name: Option<&str>) -> Result<BeamResponse> {
        let body = BeamRequest {
            project_id,
            rift_name: rift_name.map(str::to_string),
            force_sync: false,
        };
        let id = project_id.to_string();
        let resp: ApiResponse<BeamResponse> = self.post_api(&["projects", &id, "beam"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Finds a project by exact name or by UUID string.
    pub async fn find_project(&self, name_or_id: &str) -> Result<Option<Project>> {
        let projects = self.list_projects().await?;
        Ok(projects.into_iter().find(|p| {
            p.name == name_or_id || p.id.to_string() == name_or_id
        }))
    }

    /// Creates a project on the server from a local directory.
    pub async fn create_gateway(&self, name: &str, description: &str, project_path: &Path) -> Result<Project> {
        let body = CreateGatewayRequest {
            name,
            description,
            project_path,
        };
        let resp: ApiResponse<Project> = self.post_api(&["gateway", "create"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Deletes the project from the server. Local files are not touched.
    pub async fn delete_project(&self, project_id: Uuid) -> Result<()> {
        let url = self.endpoint(&["projects", &project_id.to_string()])?;
        let response = self.authed(self.http.delete(url)).send().await?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    /// Uploads files in batches of `UPLOAD_BATCH_SIZE`. Returns the number of files sent.
    pub async fn upload_files(&self, project_id: Uuid, files: HashMap<PathBuf, String>) -> Result<usize> {
        let entries: Vec<(PathBuf, String)> = files.into_iter().collect();
        let total = entries.len();
        let project_id_str = project_id.to_string();

        for batch in entries.chunks(UPLOAD_BATCH_SIZE) {
            let body = UploadFilesRequest {
                project_id,
                files: batch.iter().cloned().collect(),
            };
            let url = self.endpoint(&["projects", &project_id_str, "files"])?;
            let response = self.authed(self.http.post(url).json(&body)).send().await?;
            Self::ensure_success(response).await?;
        }

        Ok(total)
    }

    /// Returns the server's upload size limit in bytes.
    pub async fn max_upload_size(&self) -> Result<u64> {
        let resp: ApiResponse<u64> = self.get_api(&["max-upload-size"]).await?;
        Ok(Self::unwrap_api(resp).unwrap_or(DEFAULT_MAX_UPLOAD_BYTES))
    }

    /// Lists rifts belonging to `project_id`.
    pub async fn list_rifts(&self, project_id: Uuid) -> Result<Vec<RiftInfo>> {
        let resp: ApiResponse<Vec<RiftInfo>> = self.get_api(&["api", "rifts"]).await?;
        let mut rifts = Self::unwrap_api(resp)?;
        rifts.retain(|r| r.project_id == Some(project_id));
        Ok(rifts)
    }

    /// Creates a rift in `project_id`. Returns the new rift's id.
    pub async fn create_rift(
        &self,
        name: &str,
        description: Option<&str>,
        project_id: Uuid,
        fork_from: Option<Uuid>,
    ) -> Result<Uuid> {
        let body = CreateRiftRequest {
            name,
            description,
            project_id: Some(project_id),
            fork_from,
        };
        let resp: ApiResponse<Uuid> = self.post_api(&["api", "rifts"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Switches the user's active rift on the server. The daemon must reconnect separately.
    pub async fn switch_rift(&self, project_id: Uuid, rift_name: &str) -> Result<String> {
        let body = SwitchRiftRequest {
            rift_name,
            project_id: Some(project_id),
        };
        let resp: ApiResponse<String> = self.post_api(&["api", "rifts", "switch"], &body).await?;
        Self::unwrap_api(resp)
    }

    pub async fn create_checkpoint(&self, project_id: Uuid, message: Option<&str>) -> Result<CheckpointData> {
        let body = CreateCheckpointRequest {
            message,
            timestamp: Utc::now(),
        };
        let project_id = project_id.to_string();
        let resp: ApiResponse<CheckpointData> = self
            .post_api(&["projects", &project_id, "checkpoints"], &body)
            .await?;
        Self::unwrap_api(resp)
    }

    pub async fn checkpoint_history(&self, project_id: Uuid) -> Result<Vec<Checkpoint>> {
        let project_id = project_id.to_string();
        let resp: ApiResponse<Vec<Checkpoint>> = self
            .get_api(&["projects", &project_id, "history"])
            .await?;
        Self::unwrap_api(resp)
    }

    /// Lists wormholes for a project, including active and dissolved ones.
    pub async fn list_wormholes(&self, project_id: Uuid) -> Result<Vec<WormholeSummary>> {
        let url = self.endpoint(&["api", "wormholes"])?;
        let query = project_id.to_string();
        let response = self
            .authed(self.http.get(url).query(&[("project_id", query.as_str())]))
            .send()
            .await?;
        let response = Self::ensure_success(response).await?;
        let resp: ApiResponse<Vec<WormholeSummary>> = response.json().await?;
        Self::unwrap_api(resp)
    }

    /// Creates a wormhole across at least two rifts in the same project.
    pub async fn create_wormhole(&self, project_id: Uuid, name: &str, rift_ids: &[Uuid]) -> Result<WormholeSummary> {
        let body = CreateWormholeRequest {
            project_id,
            name,
            rift_ids,
        };
        let resp: ApiResponse<WormholeSummary> = self.post_api(&["api", "wormholes"], &body).await?;
        Self::unwrap_api(resp)
    }

    pub async fn get_wormhole(&self, wormhole_id: Uuid) -> Result<WormholeDetail> {
        let id = wormhole_id.to_string();
        let resp: ApiResponse<WormholeDetail> = self.get_api(&["api", "wormholes", &id]).await?;
        Self::unwrap_api(resp)
    }

    /// Records the choice for one conflict. Returns the updated wormhole.
    pub async fn resolve_wormhole_conflict(
        &self,
        wormhole_id: Uuid,
        path: &str,
        conflict_id: &str,
        choice: Choice,
    ) -> Result<WormholeDetail> {
        let body = ResolveWormholeRequest {
            path,
            conflict_id,
            choice,
        };
        let id = wormhole_id.to_string();
        let resp: ApiResponse<WormholeDetail> = self
            .post_api(&["api", "wormholes", &id, "resolve"], &body)
            .await?;
        Self::unwrap_api(resp)
    }

    /// Creates a new rift from the merged result. Fails with 409 while conflicts remain.
    pub async fn converge_wormhole(&self, wormhole_id: Uuid, name: Option<&str>) -> Result<ConvergeResult> {
        let body = ConvergeWormholeRequest { name };
        let id = wormhole_id.to_string();
        let resp: ApiResponse<ConvergeResult> = self
            .post_api(&["api", "wormholes", &id, "converge"], &body)
            .await?;
        Self::unwrap_api(resp)
    }

    /// Returns the merged file contents. Fails with 409 while conflicts remain.
    pub async fn wormhole_files(&self, wormhole_id: Uuid) -> Result<Vec<WormholeFile>> {
        let id = wormhole_id.to_string();
        let resp: ApiResponse<Vec<WormholeFile>> = self
            .get_api(&["api", "wormholes", &id, "files"])
            .await?;
        Self::unwrap_api(resp)
    }

    pub async fn dissolve_wormhole(&self, wormhole_id: Uuid) -> Result<Uuid> {
        let id = wormhole_id.to_string();
        let resp: ApiResponse<WormholeStatusResponse> = self
            .post_api(&["api", "wormholes", &id, "dissolve"], &serde_json::json!({}))
            .await?;
        Ok(Self::unwrap_api(resp)?.id)
    }
}

#[derive(Serialize)]
struct CreateWormholeRequest<'a> {
    project_id: Uuid,
    name: &'a str,
    rift_ids: &'a [Uuid],
}

#[derive(Serialize)]
struct ResolveWormholeRequest<'a> {
    path: &'a str,
    conflict_id: &'a str,
    choice: Choice,
}

#[derive(Serialize)]
struct ConvergeWormholeRequest<'a> {
    name: Option<&'a str>,
}

#[derive(Deserialize)]
struct WormholeStatusResponse {
    id: Uuid,
}
