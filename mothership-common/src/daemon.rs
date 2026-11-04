//! Client for the local daemon's IPC API (`http://localhost:7525`).
//!
//! The GUI uses this for its daemon panel. Starting the daemon lives in
//! [`crate::beam::ensure_daemon_running`].

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::beam::{daemon_healthy, DAEMON_URL};

/// Mirrors `DaemonStatus` in `mothership-daemon/src/daemon.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub is_running: bool,
    pub projects_tracked: usize,
    pub files_syncing: usize,
    /// Defaults to 0 so an older daemon's status still parses.
    #[serde(default)]
    pub files_synced: usize,
    pub last_sync: Option<DateTime<Utc>>,
    pub server_connected: bool,
}

/// Mirrors `TrackedProject` in `mothership-daemon/src/daemon.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedProject {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_path: String,
    pub added_at: DateTime<Utc>,
}

/// The daemon reports `status: "success" | "error"` (see `ApiResponse` in `ipc_server.rs`).
#[derive(Deserialize)]
struct ApiResponse<T> {
    status: String,
    data: Option<T>,
    error: Option<String>,
}

fn unwrap<T>(resp: ApiResponse<T>) -> Result<T> {
    if resp.status != "success" {
        bail!("{}", resp.error.unwrap_or_else(|| "daemon returned an error".to_string()));
    }
    resp.data.ok_or_else(|| anyhow!("daemon response had no data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daemon_status_envelope() {
        let json = r#"{"status":"success","data":{"is_running":true,"projects_tracked":2,"files_syncing":0,"last_sync":"2026-11-01T03:18:38.523708800Z","server_connected":false},"error":null}"#;
        let resp: ApiResponse<DaemonStatus> = serde_json::from_str(json).unwrap();
        let status = unwrap(resp).unwrap();
        assert!(status.is_running);
        assert_eq!(status.projects_tracked, 2);
        assert!(status.last_sync.is_some());
    }

    #[test]
    fn surfaces_daemon_error_envelope() {
        let json = r#"{"status":"error","data":null,"error":"boom"}"#;
        let resp: ApiResponse<DaemonStatus> = serde_json::from_str(json).unwrap();
        let err = unwrap(resp).unwrap_err().to_string();
        assert_eq!(err, "boom");
    }
}

fn http(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(timeout).build()?)
}

/// Returns `true` if the daemon answers `/health`.
pub async fn is_running() -> bool {
    match http(Duration::from_secs(2)) {
        Ok(client) => daemon_healthy(&client).await,
        Err(_) => false,
    }
}

pub async fn status() -> Result<DaemonStatus> {
    let resp: ApiResponse<DaemonStatus> = http(Duration::from_secs(3))?
        .get(format!("{DAEMON_URL}/status"))
        .send()
        .await
        .context("Daemon is not reachable")?
        .json()
        .await?;
    unwrap(resp)
}

pub async fn tracked_projects() -> Result<Vec<TrackedProject>> {
    let resp: ApiResponse<Vec<TrackedProject>> = http(Duration::from_secs(3))?
        .get(format!("{DAEMON_URL}/projects"))
        .send()
        .await
        .context("Daemon is not reachable")?
        .json()
        .await?;
    unwrap(resp)
}

/// Asks the daemon to delete `folder` once `wormhole_id` is dissolved or converted.
pub async fn register_snapshot(folder: &std::path::Path, wormhole_id: Uuid) -> Result<()> {
    #[derive(Serialize)]
    struct RegisterSnapshotRequest<'a> {
        folder: &'a std::path::Path,
        wormhole_id: Uuid,
    }

    let resp: ApiResponse<()> = http(Duration::from_secs(3))?
        .post(format!("{DAEMON_URL}/snapshots/register"))
        .json(&RegisterSnapshotRequest { folder, wormhole_id })
        .send()
        .await
        .context("Daemon is not reachable")?
        .json()
        .await?;
    unwrap(resp)
}

/// Restarts the daemon's WebSocket for `project_id` so it re-reads `rift_id` from project.json
/// and waits until the new rift's files are written to disk.
pub async fn reconnect_project(project_id: Uuid) -> Result<()> {
    let resp: ApiResponse<String> = http(Duration::from_secs(30))?
        .post(format!("{DAEMON_URL}/projects/{project_id}/reconnect"))
        .send()
        .await
        .context("Daemon is not reachable")?
        .json()
        .await?;
    unwrap(resp).map(|_| ())
}

/// Flushes pending changes for `project_id` and pauses its watcher/WebSocket before re-beaming.
pub async fn pause_project(project_id: Uuid) -> Result<()> {
    let resp: ApiResponse<String> = http(Duration::from_secs(15))?
        .post(format!("{DAEMON_URL}/projects/{project_id}/pause"))
        .send()
        .await
        .context("Daemon is not reachable")?
        .json()
        .await?;
    unwrap(resp).map(|_| ())
}

/// Asks the daemon to exit, then waits up to 5 seconds for it to stop answering.
pub async fn stop() -> Result<()> {
    let client = http(Duration::from_secs(3))?;
    if !daemon_healthy(&client).await {
        return Ok(());
    }
    let _ = client.post(format!("{DAEMON_URL}/shutdown")).send().await;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !daemon_healthy(&client).await {
            return Ok(());
        }
    }
    bail!("Daemon did not stop within 5 seconds")
}
