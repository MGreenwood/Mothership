//! Tracks wormhole snapshot folders and removes them once their wormhole is dissolved or converted.
//!
//! A snapshot is a one-time copy of a wormhole's merged files. The GUI registers it here after
//! beaming. The sweep deletes a folder only when its `.mothership/wormhole.json` still names the
//! registered wormhole, so a folder that was replaced or reused is never touched.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use mothership_common::client::MothershipClient;
use mothership_common::beam::WormholeMarker;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::daemon::{get_active_server_url, load_auth_token};

/// How often the daemon checks registered snapshots
pub const SNAPSHOT_SWEEP_INTERVAL_SECS: u64 = 60;

const MARKER_PATH: &[&str] = &[".mothership", "wormhole.json"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub folder: PathBuf,
    pub wormhole_id: Uuid,
}

/// Registered snapshots, persisted to `%APPDATA%\mothership\snapshots.json`
pub struct SnapshotRegistry {
    entries: Mutex<Vec<Snapshot>>,
    path: Option<PathBuf>,
}

impl SnapshotRegistry {
    pub fn load() -> Self {
        let path = dirs::config_dir().map(|dir| dir.join("mothership").join("snapshots.json"));
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            entries: Mutex::new(entries),
            path,
        }
    }

    pub fn register(&self, folder: PathBuf, wormhole_id: Uuid) -> Result<()> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if !entries.iter().any(|s| s.folder == folder && s.wormhole_id == wormhole_id) {
            entries.push(Snapshot { folder, wormhole_id });
        }
        self.persist(&entries)
    }

    /// Removes snapshots whose wormhole is gone. Errors reaching the server keep the snapshot.
    pub async fn sweep(&self) {
        let snapshots = self.entries.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if snapshots.is_empty() {
            return;
        }

        let mut keep = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            if should_remove(&snapshot).await {
                match std::fs::remove_dir_all(&snapshot.folder) {
                    Ok(()) => info!("Removed snapshot {} (wormhole {} is gone)", snapshot.folder.display(), snapshot.wormhole_id),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        warn!("Could not remove snapshot {}: {}", snapshot.folder.display(), e);
                        keep.push(snapshot);
                    }
                }
            } else if still_tracked(&snapshot) {
                keep.push(snapshot);
            } else {
                info!("Dropping snapshot record for {} (folder is gone or no longer belongs to the wormhole)", snapshot.folder.display());
            }
        }

        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        *entries = keep;
        if let Err(e) = self.persist(&entries) {
            warn!("Failed to save snapshot registry: {}", e);
        }
    }

    fn persist(&self, entries: &[Snapshot]) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(entries)?)
            .with_context(|| format!("Failed to write {}", path.display()))
    }
}

/// True only when the folder still carries this wormhole's marker.
fn still_tracked(snapshot: &Snapshot) -> bool {
    marker_matches(&snapshot.folder, snapshot.wormhole_id)
}

fn marker_matches(folder: &Path, wormhole_id: Uuid) -> bool {
    let marker_path = MARKER_PATH.iter().fold(folder.to_path_buf(), |p, part| p.join(part));
    std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<WormholeMarker>(&raw).ok())
        .is_some_and(|marker| marker.wormhole_id == wormhole_id.to_string())
}

/// Asks the server whether the wormhole is dissolved or converted. Any failure means keep.
async fn should_remove(snapshot: &Snapshot) -> bool {
    if !snapshot.folder.exists() || !marker_matches(&snapshot.folder, snapshot.wormhole_id) {
        return false;
    }
    let Some(server_url) = get_active_server_url() else {
        return false;
    };
    let client = match MothershipClient::new(&server_url, load_auth_token()) {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client.get_wormhole(snapshot.wormhole_id).await {
        Ok(detail) => matches!(detail.summary.status.as_str(), "dissolved" | "converted"),
        Err(e) => {
            debug!("Snapshot {} kept; wormhole status unavailable: {}", snapshot.folder.display(), e);
            false
        }
    }
}
