//! Dynamic tab completion for `mothership beam`.
//!
//! Inside a gateway that is already beamed (its `.mothership/project.json`
//! has a rift set), the project argument completes that gateway's rifts.
//! Otherwise it completes gateway names from the server. Any failure, such as
//! no auth, no server, or a timeout, produces no suggestions.

use std::future::Future;
use std::time::Duration;

use clap_complete::engine::CompletionCandidate;
use mothership_common::{protocol::{ApiResponse, GatewayRequest}, GatewayProject};
use serde::Deserialize;
use uuid::Uuid;

use crate::{config::ConfigManager, connections, get_http_client, get_rifts};

const COMPLETION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct LocalGatewayMetadata {
    project_id: String,
    rift_id: Option<String>,
    current_rift: Option<String>,
}

/// Candidates for the `project` argument of `mothership beam`.
pub fn beam_project_candidates() -> Vec<CompletionCandidate> {
    match beamed_gateway() {
        Some((project_id, true)) => sorted(run_isolated(rift_names(project_id)).unwrap_or_default()),
        Some((_, false)) | None => sorted(run_isolated(gateway_names()).unwrap_or_default()),
    }
}

/// Returns the project id of the gateway in the current directory, and whether it is beamed.
fn beamed_gateway() -> Option<(Uuid, bool)> {
    let dir = std::env::current_dir().ok()?;
    let content = std::fs::read_to_string(dir.join(".mothership").join("project.json")).ok()?;
    let metadata: LocalGatewayMetadata = serde_json::from_str(&content).ok()?;
    let beamed = metadata.rift_id.is_some() || metadata.current_rift.is_some();
    Some((Uuid::parse_str(&metadata.project_id).ok()?, beamed))
}

async fn gateway_names() -> anyhow::Result<Vec<String>> {
    let config_manager = ConfigManager::new()?;
    if !config_manager.is_authenticated()? {
        return Ok(Vec::new());
    }
    let active_server = connections::get_active_server()?
        .ok_or_else(|| anyhow::anyhow!("no active server"))?;
    let config = config_manager.load_config()?;
    let client = get_http_client(&config);

    let response = client
        .post(format!("{}/gateway", active_server.url))
        .json(&GatewayRequest { include_inactive: false })
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }

    let body: ApiResponse<Vec<GatewayProject>> = response.json().await?;
    Ok(body
        .data
        .unwrap_or_default()
        .into_iter()
        .map(|gateway| gateway.project.name)
        .collect())
}

async fn rift_names(project_id: Uuid) -> anyhow::Result<Vec<String>> {
    let rifts = get_rifts().await?;
    Ok(rifts
        .into_iter()
        .filter(|rift| rift.project_id.map_or(true, |id| id == project_id))
        .map(|rift| rift.name)
        .collect())
}

/// Runs a fetch on its own thread and runtime, since completion runs inside `tokio::main`.
fn run_isolated<T, F>(fut: F) -> Option<T>
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            tokio::time::timeout(COMPLETION_TIMEOUT, fut).await.ok()?.ok()
        })
    })
    .join()
    .ok()
    .flatten()
}

fn sorted(mut names: Vec<String>) -> Vec<CompletionCandidate> {
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names.into_iter().map(CompletionCandidate::new).collect()
}
