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

use crate::ignore::IgnoreRules;
use crate::merge::Choice;
use crate::protocol::{ApiResponse, BeamRequest, BeamResponse};
use crate::{Checkpoint, Project};

/// Fallback when the server does not report its upload limit.
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 5 * 1024 * 1024;
/// Maximum number of files sent per `POST /projects/:id/files` request.
pub const UPLOAD_BATCH_SIZE: usize = 20;
/// Maximum total file content per upload request. Kept well under the server's
/// request body limit, even after JSON escaping.
pub const UPLOAD_BATCH_BYTES: usize = 4 * 1024 * 1024;

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

/// One entry in a rift's event log, as returned by `GET /api/rifts/:id/events`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiftEvent {
    pub id: i64,
    pub event_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_username: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Options for reading a rift's event log.
/// Forward reads return events after `after`, oldest first. `latest` returns the newest events,
/// and `before` returns the events just older than an id. `wait_secs` holds a forward read open
/// until something new is recorded, so followers do not need to poll.
#[derive(Debug, Clone)]
pub struct RiftEventQuery {
    pub after: i64,
    pub before: Option<i64>,
    pub latest: bool,
    pub limit: i64,
    pub wait_secs: u64,
}

/// Checkpoint metadata plus the full contents of every file in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreData {
    pub checkpoint: Checkpoint,
    pub files: HashMap<PathBuf, String>,
}

fn default_side_enabled() -> bool {
    true
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
    #[serde(default)]
    pub disabled_rift_ids: Vec<Uuid>,
    pub converted_rift_id: Option<Uuid>,
    pub unresolved_conflicts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormholeSide {
    /// Index used by `Choice::Side` and by each conflict's `sides` list
    pub index: usize,
    pub rift_id: Uuid,
    pub rift_name: String,
    #[serde(default = "default_side_enabled")]
    pub enabled: bool,
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

/// A path whose text differs between a wormhole's rifts, from `GET /api/wormholes/:id/rift-files`.
/// `contents[i]` is the text on side `i`, or `None` when that rift lacks the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiftFileSides {
    pub path: String,
    pub contents: Vec<Option<String>>,
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

/// Resolves the active Mothership server URL across CLI, GUI, and daemon:
/// 1. `<config_dir>/mothership/connections.json` (`active_server`)
/// 2. `<config_dir>/mothership/config.json` (`mothership_url`)
/// 3. `ClientConfig::default().mothership_url`
pub fn active_server_url() -> String {
    #[derive(Deserialize)]
    struct ServerConn {
        url: String,
    }
    #[derive(Deserialize)]
    struct ConnectionsFile {
        active_server: Option<String>,
        #[serde(default)]
        servers: HashMap<String, ServerConn>,
    }
    #[derive(Deserialize)]
    struct ConfigFile {
        mothership_url: Option<String>,
    }

    if let Some(dir) = dirs::config_dir().map(|d| d.join("mothership")) {
        if let Ok(raw) = std::fs::read_to_string(dir.join("connections.json")) {
            if let Ok(conns) = serde_json::from_str::<ConnectionsFile>(&raw) {
                if let Some(active) = conns.active_server {
                    if let Some(server) = conns.servers.get(&active) {
                        if !server.url.trim().is_empty() {
                            return server.url.clone();
                        }
                    }
                    if !active.trim().is_empty() {
                        return active;
                    }
                }
            }
        }
        if let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) {
            if let Ok(cfg) = serde_json::from_str::<ConfigFile>(&raw) {
                if let Some(url) = cfg.mothership_url.filter(|u| !u.trim().is_empty()) {
                    return url;
                }
            }
        }
    }

    crate::ClientConfig::default().mothership_url
}

static SHARED_HTTP: std::sync::OnceLock<Client> = std::sync::OnceLock::new();

/// Process-wide shared `reqwest::Client` for connection-pool reuse.
pub fn shared_http_client() -> &'static Client {
    SHARED_HTTP.get_or_init(Client::new)
}

/// How long to wait for the browser to return to the loopback callback.
const BROWSER_LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// A browser sign-in in progress. The caller opens `auth_url`, then calls [`BrowserLogin::wait_for_token`].
pub struct BrowserLogin {
    pub auth_url: String,
    server_url: String,
    state: String,
    listener: tokio::net::TcpListener,
}

/// Credentials from a completed browser sign-in.
pub struct LoginResult {
    pub token: String,
    pub username: Option<String>,
    pub email: Option<String>,
}

/// Starts a browser login on a loopback callback. `provider` is `Some("google")`, `Some("ghostid")`,
/// or `None` to let the server show its sign-in choices.
pub async fn start_browser_login(base_url: &str, provider: Option<&str>) -> Result<BrowserLogin> {
    let server_url = base_url.trim_end_matches('/').to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let state = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

    let started: serde_json::Value = shared_http_client()
        .post(format!("{}/auth/cli/start", server_url))
        .json(&serde_json::json!({ "callback_port": port, "state": state }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut auth_url = started["auth_url"]
        .as_str()
        .ok_or_else(|| anyhow!("Server did not return a login URL"))?
        .to_string();
    if let Some(provider) = provider {
        auth_url.push_str(&format!("&provider={}", provider));
    }
    Ok(BrowserLogin { auth_url, server_url, state, listener })
}

impl BrowserLogin {
    /// Waits for the browser callback, exchanges the one-time code for a token, then redirects
    /// the browser to the server's success page.
    pub async fn wait_for_token(self) -> Result<LoginResult> {
        use tokio::io::AsyncWriteExt;
        let (code, mut stream) = tokio::time::timeout(
            BROWSER_LOGIN_TIMEOUT,
            wait_for_callback(self.listener, &self.state),
        )
        .await
        .map_err(|_| anyhow!("Timed out waiting for browser sign-in. Try again."))??;

        let exchanged = exchange_login_code(&self.server_url, &code, &self.state).await;
        let response = match &exchanged {
            Ok((_, success_url)) => format!(
                "HTTP/1.1 302 Found\r\nLocation: {success_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
            Err(_) => callback_response(
                "400 Bad Request",
                "Sign-in could not be completed. Run the login again from your terminal or app.",
            ),
        };
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;

        exchanged.map(|(login, _)| login)
    }
}

async fn exchange_login_code(server_url: &str, code: &str, state: &str) -> Result<(LoginResult, String)> {
    let exchanged: serde_json::Value = shared_http_client()
        .post(format!("{}/auth/cli/exchange", server_url))
        .json(&serde_json::json!({ "code": code, "state": state }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let token = exchanged["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Server did not return a token"))?
        .to_string();
    let success_url = exchanged["success_url"].as_str().unwrap_or_default().to_string();
    let login = LoginResult {
        token,
        username: exchanged["username"].as_str().map(str::to_string),
        email: exchanged["email"].as_str().map(str::to_string),
    };
    Ok((login, success_url))
}

/// Waits for the browser to hit the loopback callback with a matching state, then returns the code
/// and the open connection so the browser can be redirected once the code is exchanged.
/// Stray requests get an error response and the wait continues.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<(String, tokio::net::TcpStream)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let read = stream.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        let target = request.split_whitespace().nth(1).unwrap_or("").to_string();

        let params: HashMap<String, String> = match target.strip_prefix("/callback?") {
            Some(query) => Url::parse(&format!("http://127.0.0.1/callback?{query}"))
                .map(|url| url.query_pairs().into_owned().collect())
                .unwrap_or_default(),
            None => HashMap::new(),
        };
        let code = params.get("code").cloned();
        let state_ok = params.get("state").is_some_and(|s| s == expected_state);

        if let (true, Some(code)) = (state_ok, code.clone()) {
            return Ok((code, stream));
        }
        let response = if target.starts_with("/callback?") {
            callback_response(
                "400 Bad Request",
                "This login response was not valid. Run the login again from your terminal or app.",
            )
        } else {
            callback_response("404 Not Found", "Not found.")
        };
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}

fn callback_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
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

/// Splits files into upload batches limited by file count and total content bytes.
pub fn split_upload_batches(files: Vec<(PathBuf, String)>) -> Vec<Vec<(PathBuf, String)>> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0usize;
    for (path, content) in files {
        let size = content.len();
        let full = current.len() >= UPLOAD_BATCH_SIZE
            || (!current.is_empty() && current_bytes + size > UPLOAD_BATCH_BYTES);
        if full {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes += size;
        current.push((path, content));
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Walks `dir` and collects UTF-8 text files no larger than `max_size` bytes.
/// Paths matching the directory's `.mothershipignore` rules are skipped.
pub fn scan_text_files(dir: &Path, max_size: u64) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let rules = IgnoreRules::load(dir)?;

    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !rules.is_ignored(e.path(), e.file_type().is_dir()))
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
                        let normalized = PathBuf::from(crate::normalize_rel_path(&relative));
                        report.files.insert(normalized, content);
                    }
                    Err(_) => report.skipped_binary.push(relative),
                }
            }
            Err(_) => report.skipped_unreadable.push(relative),
        }
    }

    Ok(report)
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    fn file(name: &str, size: usize) -> (PathBuf, String) {
        (PathBuf::from(name), "a".repeat(size))
    }

    #[test]
    fn batches_split_on_file_count() {
        let files: Vec<_> = (0..45).map(|i| file(&format!("f{i}"), 1)).collect();
        let batches = split_upload_batches(files);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), vec![20, 20, 5]);
    }

    #[test]
    fn batches_split_on_byte_budget() {
        let half = UPLOAD_BATCH_BYTES / 2 + 1;
        let files = vec![file("a", half), file("b", half), file("c", 10)];
        let batches = split_upload_batches(files);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn oversized_single_file_gets_its_own_batch() {
        let files = vec![file("big", UPLOAD_BATCH_BYTES * 2), file("small", 1)];
        let batches = split_upload_batches(files);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 1]);
    }

    #[test]
    fn empty_input_gives_no_batches() {
        assert!(split_upload_batches(Vec::new()).is_empty());
    }
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
    is_private: bool,
}

#[derive(Serialize)]
struct RiftAccessRequestBody<'a> {
    rift_name: &'a str,
    project_id: Uuid,
}

#[derive(Serialize)]
struct RiftMemberBody<'a> {
    rift_name: &'a str,
    project_id: Uuid,
    username: &'a str,
    remove: bool,
}

/// A pending request to join a private rift, as shown to the rift owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiftAccessRequestInfo {
    pub user_id: Uuid,
    pub username: String,
    pub requested_at: DateTime<Utc>,
}

/// Result of asking the server to switch to a rift.
#[derive(Debug, Clone)]
pub enum SwitchOutcome {
    Switched(String),
    /// The rift exists in the project but the caller is not a member.
    NotMember(String),
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
    /// Files or folders (ending in `/`) to capture. Omitted for a whole-rift checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<&'a [String]>,
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
            http: shared_http_client().clone(),
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

    /// Uploads files in size-limited batches. Returns the number of files sent.
    pub async fn upload_files(&self, project_id: Uuid, files: HashMap<PathBuf, String>) -> Result<usize> {
        let entries: Vec<(PathBuf, String)> = files.into_iter().collect();
        let total = entries.len();
        let project_id_str = project_id.to_string();

        for batch in split_upload_batches(entries) {
            let body = UploadFilesRequest {
                project_id,
                files: batch.into_iter().collect(),
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
        is_private: bool,
    ) -> Result<Uuid> {
        let body = CreateRiftRequest {
            name,
            description,
            project_id: Some(project_id),
            fork_from,
            is_private,
        };
        let resp: ApiResponse<Uuid> = self.post_api(&["api", "rifts"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Switches the user's active rift on the server. The daemon must reconnect separately.
    pub async fn switch_rift(&self, project_id: Uuid, rift_name: &str) -> Result<String> {
        match self.switch_rift_outcome(project_id, rift_name).await? {
            SwitchOutcome::Switched(msg) => Ok(msg),
            SwitchOutcome::NotMember(msg) => Err(anyhow!("{}", msg)),
        }
    }

    /// Like [`switch_rift`](Self::switch_rift), but reports a non-member 403 as `SwitchOutcome::NotMember`
    /// so the caller can offer to request access.
    pub async fn switch_rift_outcome(&self, project_id: Uuid, rift_name: &str) -> Result<SwitchOutcome> {
        let body = SwitchRiftRequest {
            rift_name,
            project_id: Some(project_id),
        };
        let url = self.endpoint(&["api", "rifts", "switch"])?;
        let response = self.authed(self.http.post(url).json(&body)).send().await?;
        let forbidden = response.status() == reqwest::StatusCode::FORBIDDEN;
        if !response.status().is_success() && !forbidden {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("server returned {}: {}", status, text));
        }
        let resp: ApiResponse<String> = response.json().await?;
        if forbidden {
            return Ok(SwitchOutcome::NotMember(
                resp.error.unwrap_or_else(|| "You are not a member of this rift".to_string()),
            ));
        }
        Self::unwrap_api(resp).map(SwitchOutcome::Switched)
    }

    /// Asks the rift owner for access to a private rift in `project_id`. The caller must be a project member.
    pub async fn request_rift_access(&self, project_id: Uuid, rift_name: &str) -> Result<String> {
        let body = RiftAccessRequestBody { rift_name, project_id };
        let resp: ApiResponse<String> = self.post_api(&["api", "rifts", "access-requests"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Lists pending access requests for a rift. Only the rift owner may call this.
    pub async fn list_rift_access_requests(
        &self,
        project_id: Uuid,
        rift_name: &str,
    ) -> Result<Vec<RiftAccessRequestInfo>> {
        let url = self.endpoint(&["api", "rifts", "access-requests"])?;
        let project_id = project_id.to_string();
        let response = self
            .authed(
                self.http
                    .get(url)
                    .query(&[("rift_name", rift_name), ("project_id", project_id.as_str())]),
            )
            .send()
            .await?;
        let response = Self::ensure_success(response).await?;
        let resp: ApiResponse<Vec<RiftAccessRequestInfo>> = response.json().await?;
        Self::unwrap_api(resp)
    }

    /// Adds (or removes, when `remove` is true) a user as a member of a rift. Owner only.
    pub async fn manage_rift_member(
        &self,
        project_id: Uuid,
        rift_name: &str,
        username: &str,
        remove: bool,
    ) -> Result<String> {
        let body = RiftMemberBody {
            rift_name,
            project_id,
            username,
            remove,
        };
        let resp: ApiResponse<String> = self.post_api(&["api", "rifts", "members"], &body).await?;
        Self::unwrap_api(resp)
    }

    /// Creates a checkpoint. With `paths`, only those files and folders (folders end in `/`) are
    /// captured; without them the whole rift is.
    pub async fn create_checkpoint(
        &self,
        project_id: Uuid,
        message: Option<&str>,
        paths: Option<&[String]>,
    ) -> Result<CheckpointData> {
        let body = CreateCheckpointRequest {
            message,
            timestamp: Utc::now(),
            paths,
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

    /// Reads a rift's event log. See [`RiftEventQuery`] for the paging and waiting options.
    pub async fn rift_events(&self, rift_id: Uuid, query: &RiftEventQuery) -> Result<Vec<RiftEvent>> {
        let rift_id = rift_id.to_string();
        let url = self.endpoint(&["api", "rifts", &rift_id, "events"])?;
        let mut params = vec![
            ("after", query.after.to_string()),
            ("limit", query.limit.to_string()),
            ("latest", query.latest.to_string()),
            ("wait", query.wait_secs.to_string()),
        ];
        if let Some(before) = query.before {
            params.push(("before", before.to_string()));
        }
        let response = self.authed(self.http.get(url).query(&params)).send().await?;
        let response = Self::ensure_success(response).await?;
        let resp: ApiResponse<Vec<RiftEvent>> = response.json().await?;
        Self::unwrap_api(resp)
    }

    /// Fetches a checkpoint's files from the server. Nothing is written to disk here.
    pub async fn restore_checkpoint(&self, project_id: Uuid, checkpoint_id: Uuid) -> Result<RestoreData> {
        let project_id = project_id.to_string();
        let checkpoint_id = checkpoint_id.to_string();
        let url = self.endpoint(&["projects", &project_id, "checkpoints", &checkpoint_id, "restore"])?;
        let response = self.authed(self.http.post(url)).send().await?;
        let response = Self::ensure_success(response).await?;
        let resp: ApiResponse<RestoreData> = response.json().await?;
        Self::unwrap_api(resp)
    }

    /// Deletes a checkpoint's metadata from the caller's rift on the server.
    pub async fn delete_checkpoint(&self, project_id: Uuid, checkpoint_id: Uuid) -> Result<()> {
        let project_id = project_id.to_string();
        let checkpoint_id = checkpoint_id.to_string();
        let url = self.endpoint(&["projects", &project_id, "checkpoints", &checkpoint_id])?;
        let response = self.authed(self.http.delete(url)).send().await?;
        let response = Self::ensure_success(response).await?;
        let resp: ApiResponse<String> = response.json().await?;
        Self::unwrap_api(resp).map(|_| ())
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
    pub async fn create_wormhole(
        &self,
        project_id: Uuid,
        name: &str,
        rift_ids: &[Uuid],
        is_private: bool,
    ) -> Result<WormholeSummary> {
        let body = CreateWormholeRequest {
            project_id,
            name,
            rift_ids,
            is_private,
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

    /// Permanently deletes a wormhole from the server.
    pub async fn delete_wormhole(&self, wormhole_id: Uuid) -> Result<Uuid> {
        let id = wormhole_id.to_string();
        let resp: ApiResponse<WormholeStatusResponse> = self
            .post_api(
                &["api", "wormholes", &id, "dissolve"],
                &serde_json::json!({ "delete": true }),
            )
            .await?;
        Ok(Self::unwrap_api(resp)?.id)
    }

    /// Toggles (or sets) whether a rift's modular changes are active in a wormhole.
    pub async fn toggle_wormhole_rift(
        &self,
        wormhole_id: Uuid,
        rift_id: Uuid,
        enabled: Option<bool>,
    ) -> Result<WormholeDetail> {
        let id = wormhole_id.to_string();
        let body = ToggleWormholeRiftRequest { rift_id, enabled };
        let resp: ApiResponse<WormholeDetail> = self
            .post_api(&["api", "wormholes", &id, "toggle"], &body)
            .await?;
        Self::unwrap_api(resp)
    }

    /// Per-side file text for paths that differ between a wormhole's rifts.
    pub async fn wormhole_rift_files(&self, wormhole_id: Uuid) -> Result<Vec<RiftFileSides>> {
        let id = wormhole_id.to_string();
        let resp: ApiResponse<Vec<RiftFileSides>> = self
            .get_api(&["api", "wormholes", &id, "rift-files"])
            .await?;
        Self::unwrap_api(resp)
    }
}

#[derive(Serialize)]
struct CreateWormholeRequest<'a> {
    project_id: Uuid,
    name: &'a str,
    rift_ids: &'a [Uuid],
    is_private: bool,
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

#[derive(Serialize)]
struct ToggleWormholeRiftRequest {
    rift_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct WormholeStatusResponse {
    id: Uuid,
}
