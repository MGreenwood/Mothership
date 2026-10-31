//! `mothership wormhole` commands: live, non-destructive merge previews across rifts.

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use clap::Subcommand;
use colored::Colorize;
use mothership_common::merge::Choice;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    connections, get_current_project_metadata, get_oauth_token, get_rifts, local_project_id,
    response_error_message, ApiResponse, RiftInfo,
};

#[derive(Subcommand)]
pub enum WormholeAction {
    /// Create a wormhole that previews merging the given rifts
    Create {
        /// Wormhole name
        name: String,
        /// Two or more rift names to merge
        #[arg(required = true, num_args = 2..)]
        rifts: Vec<String>,
    },
    /// List wormholes in this project
    List,
    /// Show a wormhole's merged state and conflicts
    Show {
        /// Wormhole ID or name
        wormhole: String,
    },
    /// Resolve a conflict by taking one of its sides
    Resolve {
        /// Wormhole ID or name
        wormhole: String,
        /// File path, as shown by `wormhole show`
        path: String,
        /// Conflict ID, as shown by `wormhole show`
        conflict: String,
        /// Side index, as shown by `wormhole show`
        #[arg(long)]
        side: usize,
    },
    /// Create a new rift from the merged result (blocked while conflicts remain)
    Converge {
        /// Wormhole ID or name
        wormhole: String,
        /// Name for the new rift (defaults to the wormhole name)
        #[arg(long)]
        name: Option<String>,
    },
    /// Archive a wormhole without creating a rift
    Dissolve {
        /// Wormhole ID or name
        wormhole: String,
    },
}

#[derive(Deserialize)]
struct WormholeSummary {
    id: Uuid,
    name: String,
    status: String,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
    rift_ids: Vec<Uuid>,
    converted_rift_id: Option<Uuid>,
    unresolved_conflicts: usize,
}

#[derive(Deserialize)]
struct ConflictView {
    id: String,
    base_start: usize,
    base_end: usize,
    sides: Vec<Vec<String>>,
    resolved: bool,
    choice: Option<Choice>,
}

#[derive(Deserialize)]
struct FileView {
    path: String,
    conflicts: Vec<ConflictView>,
}

#[derive(Deserialize)]
struct WormholeDetail {
    #[serde(flatten)]
    summary: WormholeSummary,
    files: Vec<FileView>,
}

#[derive(Deserialize)]
struct ConvergeResponse {
    rift_name: String,
    files_written: usize,
    files_removed: usize,
}

#[derive(Deserialize)]
struct DissolveResponse {
    id: Uuid,
}

struct Session {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

fn session() -> Result<Session> {
    let server = connections::get_active_server()?
        .ok_or_else(|| anyhow!("No active server connection. Please run 'mothership connect <server-url>' first."))?;
    let token = get_oauth_token().ok_or_else(|| anyhow!("Not authenticated. Please run 'mothership auth' first."))?;
    Ok(Session {
        client: reqwest::Client::new(),
        base_url: server.url,
        token,
    })
}

impl Session {
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        read_response(response).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header("Authorization", format!("Bearer {}", self.token))
            .json(body)
            .send()
            .await?;
        read_response(response).await
    }
}

async fn read_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        bail!("{}", response_error_message(response).await);
    }
    let api: ApiResponse<T> = response.json().await?;
    if !api.success {
        bail!("{}", api.error.unwrap_or_else(|| "Unknown error".to_string()));
    }
    api.data.ok_or_else(|| anyhow!("No data received from server"))
}

pub async fn run(action: WormholeAction) -> Result<()> {
    get_current_project_metadata()?;
    let project_id = local_project_id().ok_or_else(|| anyhow!("Could not read this project's ID from .mothership/project.json"))?;
    let session = session()?;

    match action {
        WormholeAction::Create { name, rifts } => create(&session, project_id, &name, &rifts).await,
        WormholeAction::List => list(&session, project_id).await,
        WormholeAction::Show { wormhole } => {
            let id = resolve_wormhole_id(&session, project_id, &wormhole).await?;
            show(&session, id).await
        }
        WormholeAction::Resolve { wormhole, path, conflict, side } => {
            let id = resolve_wormhole_id(&session, project_id, &wormhole).await?;
            resolve(&session, id, &path, &conflict, side).await
        }
        WormholeAction::Converge { wormhole, name } => {
            let id = resolve_wormhole_id(&session, project_id, &wormhole).await?;
            converge(&session, id, name).await
        }
        WormholeAction::Dissolve { wormhole } => {
            let id = resolve_wormhole_id(&session, project_id, &wormhole).await?;
            dissolve(&session, id).await
        }
    }
}

fn find_rift(rifts: &[RiftInfo], reference: &str) -> Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return Ok(id);
    }
    let matches: Vec<&RiftInfo> = rifts.iter().filter(|rift| rift.name == reference).collect();
    match matches.as_slice() {
        [one] => Ok(one.id),
        [] => bail!("No rift named '{}' in this project", reference),
        _ => bail!("More than one rift is named '{}'; use its ID", reference),
    }
}

fn rift_name(rifts: &[RiftInfo], id: Uuid) -> String {
    rifts
        .iter()
        .find(|rift| rift.id == id)
        .map(|rift| rift.name.clone())
        .unwrap_or_else(|| id.to_string())
}

async fn resolve_wormhole_id(session: &Session, project_id: Uuid, reference: &str) -> Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return Ok(id);
    }
    let wormholes: Vec<WormholeSummary> = session.get(&format!("/api/wormholes?project_id={}", project_id)).await?;
    let matches: Vec<&WormholeSummary> = wormholes
        .iter()
        .filter(|wormhole| wormhole.status == "active" && wormhole.name == reference)
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id),
        [] => bail!("No active wormhole named '{}' in this project", reference),
        _ => bail!("More than one active wormhole is named '{}'; use its ID", reference),
    }
}

async fn create(session: &Session, project_id: Uuid, name: &str, rift_refs: &[String]) -> Result<()> {
    let rifts = get_rifts().await?;
    let mut rift_ids = Vec::with_capacity(rift_refs.len());
    for reference in rift_refs {
        rift_ids.push(find_rift(&rifts, reference)?);
    }

    let body = json!({
        "project_id": project_id,
        "name": name,
        "rift_ids": rift_ids,
    });
    let summary: WormholeSummary = session.post("/api/wormholes", &body).await?;
    println!("{} {} ({})", "Created wormhole".green(), summary.name.bold(), summary.id);
    println!("See conflicts with: mothership wormhole show {}", summary.name);
    Ok(())
}

async fn list(session: &Session, project_id: Uuid) -> Result<()> {
    let wormholes: Vec<WormholeSummary> = session.get(&format!("/api/wormholes?project_id={}", project_id)).await?;
    if wormholes.is_empty() {
        println!("No wormholes in this project.");
        return Ok(());
    }
    for wormhole in &wormholes {
        println!(
            "{}  {}  [{}]  {} unresolved",
            wormhole.id,
            wormhole.name.bold(),
            wormhole.status,
            wormhole.unresolved_conflicts
        );
    }
    Ok(())
}

async fn show(session: &Session, id: Uuid) -> Result<()> {
    let detail: WormholeDetail = session.get(&format!("/api/wormholes/{}", id)).await?;
    let rifts = get_rifts().await?;
    let summary = &detail.summary;

    println!("{} [{}]", summary.name.bold(), summary.status);
    println!("ID: {}", summary.id);
    let names: Vec<String> = summary.rift_ids.iter().map(|rift_id| rift_name(&rifts, *rift_id)).collect();
    println!("Rifts: {}", names.join(", "));

    if let Some(rift_id) = summary.converted_rift_id {
        println!("Converted into rift: {}", rift_name(&rifts, rift_id));
        return Ok(());
    }
    if summary.status != "active" {
        return Ok(());
    }
    if summary.unresolved_conflicts == 0 {
        println!("\n{}", "No unresolved conflicts. Ready to converge.".green());
        return Ok(());
    }

    println!("\nUnresolved conflicts: {}", summary.unresolved_conflicts);
    for file in &detail.files {
        println!("\n{}", file.path.bold());
        for conflict in &file.conflicts {
            let state = if conflict.resolved { "resolved".green() } else { "unresolved".yellow() };
            println!("  conflict {} ({}) {}", conflict.id, describe_base(conflict.base_start, conflict.base_end), state);
            for (index, side) in conflict.sides.iter().enumerate() {
                println!("    side {}:", index);
                if side.is_empty() {
                    println!("      (deleted)");
                }
                for line in side {
                    println!("      | {}", line);
                }
            }
            if let Some(choice) = &conflict.choice {
                println!("    chosen: {}", describe_choice(choice));
            }
        }
    }
    println!(
        "\nResolve with: mothership wormhole resolve {} <path> <conflict> --side <n>",
        summary.name
    );
    Ok(())
}

fn describe_base(start: usize, end: usize) -> String {
    if end > start {
        format!("base lines {}-{}", start + 1, end)
    } else {
        format!("insert before base line {}", start + 1)
    }
}

fn describe_choice(choice: &Choice) -> String {
    match choice {
        Choice::Side(index) => format!("side {}", index),
        Choice::Custom(_) => "custom text".to_string(),
    }
}

async fn resolve(session: &Session, id: Uuid, path: &str, conflict: &str, side: usize) -> Result<()> {
    let body = json!({
        "path": path,
        "conflict_id": conflict,
        "choice": Choice::Side(side),
    });
    let detail: WormholeDetail = session.post(&format!("/api/wormholes/{}/resolve", id), &body).await?;
    println!(
        "{} conflict {} in {} with side {}.",
        "Resolved".green(),
        conflict,
        path,
        side
    );
    println!("Unresolved conflicts remaining: {}", detail.summary.unresolved_conflicts);
    Ok(())
}

async fn converge(session: &Session, id: Uuid, name: Option<String>) -> Result<()> {
    let body = json!({ "name": name });
    let response: ConvergeResponse = session.post(&format!("/api/wormholes/{}/converge", id), &body).await?;
    println!(
        "{} new rift '{}' ({} files written, {} removed).",
        "Converged into".green(),
        response.rift_name.bold(),
        response.files_written,
        response.files_removed
    );
    println!("Switch to it with: mothership rift switch {}", response.rift_name);
    Ok(())
}

async fn dissolve(session: &Session, id: Uuid) -> Result<()> {
    let response: DissolveResponse = session.post(&format!("/api/wormholes/{}/dissolve", id), &json!({})).await?;
    println!("Dissolved wormhole {}.", response.id);
    Ok(())
}
