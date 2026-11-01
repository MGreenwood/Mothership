//! Wormholes: live, non-destructive merge previews across rifts in one project.
//! Merges are recomputed from current live state on every request, so a preview
//! is always current. Converging creates a new rift and archives the wormhole.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use mothership_common::{
    merge::{merge_rifts, snapshot_base, Choice, FileMerge, Resolution},
    protocol::ApiResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::authenticated_user;
use crate::database::WormholeRecord;
use crate::AppState;

type ApiError = (StatusCode, String);
type ApiResult<T> = Result<Json<ApiResponse<T>>, ApiError>;

fn fail(status: StatusCode, message: impl Into<String>) -> ApiError {
    (status, message.into())
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    fail(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn current_user(state: &AppState, headers: &HeaderMap) -> Result<Uuid, ApiError> {
    authenticated_user(state, headers).map_err(|status| fail(status, "not authenticated"))
}

async fn ensure_member(state: &AppState, user_id: Uuid, project_id: Uuid) -> Result<(), ApiError> {
    if state.db.user_has_project_access(user_id, project_id).await.map_err(internal)? {
        Ok(())
    } else {
        Err(fail(StatusCode::FORBIDDEN, "not a member of this project"))
    }
}

async fn load_wormhole(state: &AppState, user_id: Uuid, wormhole_id: Uuid) -> Result<WormholeRecord, ApiError> {
    let wormhole = state
        .db
        .get_wormhole(wormhole_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "wormhole not found"))?;
    ensure_member(state, user_id, wormhole.project_id).await?;
    Ok(wormhole)
}

fn require_active(wormhole: &WormholeRecord) -> Result<(), ApiError> {
    if wormhole.status == "active" {
        Ok(())
    } else {
        Err(fail(StatusCode::CONFLICT, format!("wormhole is {}", wormhole.status)))
    }
}

/// Merge state for a wormhole, computed from the current live state of its rifts
struct Computed {
    merges: BTreeMap<String, FileMerge>,
    resolutions: HashMap<String, HashMap<String, Resolution>>,
}

impl Computed {
    fn resolutions_for(&self, path: &str) -> HashMap<String, Resolution> {
        self.resolutions.get(path).cloned().unwrap_or_default()
    }

    fn unresolved_count(&self) -> usize {
        self.merges
            .iter()
            .map(|(path, merge)| merge.unresolved(&self.resolutions_for(path)).len())
            .sum()
    }
}

async fn compute(state: &AppState, wormhole: &WormholeRecord) -> Result<Computed, ApiError> {
    let mut base: HashMap<PathBuf, String> = HashMap::new();
    for (path, hash) in &wormhole.base_hashes {
        let content = state
            .sync
            .storage
            .get_content(hash)
            .await
            .map_err(internal)?
            .ok_or_else(|| internal(format!("base content missing for {path}")))?;
        base.insert(PathBuf::from(path), content);
    }

    let mut live = Vec::with_capacity(wormhole.rift_ids.len());
    for rift_id in &wormhole.rift_ids {
        live.push(state.sync.storage.get_live_state(*rift_id).await.map_err(internal)?);
    }
    let refs: Vec<&HashMap<PathBuf, String>> = live.iter().collect();

    let merges = merge_rifts(&base, &refs)
        .into_iter()
        .map(|(path, merge)| (path.to_string_lossy().into_owned(), merge))
        .collect();
    let resolutions = state.db.get_wormhole_resolutions(wormhole.id).await.map_err(internal)?;

    Ok(Computed { merges, resolutions })
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct ConflictView {
    pub id: String,
    pub base_start: usize,
    pub base_end: usize,
    pub sides: Vec<Vec<String>>,
    pub fingerprint: String,
    pub resolved: bool,
    pub choice: Option<Choice>,
}

#[derive(Debug, Serialize)]
pub struct FileView {
    pub path: String,
    pub conflicts: Vec<ConflictView>,
}

#[derive(Debug, Serialize)]
pub struct SideView {
    /// Index used by `Choice::Side` and by each conflict's `sides` list
    pub index: usize,
    pub rift_id: Uuid,
    pub rift_name: String,
}

#[derive(Debug, Serialize)]
pub struct WormholeDetail {
    #[serde(flatten)]
    pub summary: WormholeSummary,
    pub sides: Vec<SideView>,
    pub files: Vec<FileView>,
}

async fn side_views(state: &AppState, wormhole: &WormholeRecord) -> Result<Vec<SideView>, ApiError> {
    let mut sides = Vec::with_capacity(wormhole.rift_ids.len());
    for (index, rift_id) in wormhole.rift_ids.iter().enumerate() {
        let rift = state
            .db
            .get_rift(*rift_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| internal(format!("rift {rift_id} missing")))?;
        sides.push(SideView { index, rift_id: *rift_id, rift_name: rift.name });
    }
    Ok(sides)
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub id: Uuid,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ConvergeResponse {
    pub rift_id: Uuid,
    pub rift_name: String,
    pub files_written: usize,
    pub files_removed: usize,
}

fn summarize(wormhole: &WormholeRecord, computed: &Computed) -> WormholeSummary {
    let unresolved_conflicts = if wormhole.status == "active" {
        computed.unresolved_count()
    } else {
        0
    };
    WormholeSummary {
        id: wormhole.id,
        project_id: wormhole.project_id,
        name: wormhole.name.clone(),
        status: wormhole.status.clone(),
        created_at: wormhole.created_at,
        rift_ids: wormhole.rift_ids.clone(),
        converted_rift_id: wormhole.converted_rift_id,
        unresolved_conflicts,
    }
}

fn detail(wormhole: &WormholeRecord, computed: &Computed, sides: Vec<SideView>) -> WormholeDetail {
    let files = computed
        .merges
        .iter()
        .filter_map(|(path, merge)| {
            let resolutions = computed.resolutions_for(path);
            let conflicts: Vec<ConflictView> = merge
                .conflicts()
                .map(|conflict| {
                    let stored = resolutions.get(&conflict.id);
                    let current = stored.filter(|r| r.fingerprint == conflict.fingerprint);
                    ConflictView {
                        id: conflict.id.clone(),
                        base_start: conflict.base_start,
                        base_end: conflict.base_end,
                        sides: conflict.sides.clone(),
                        fingerprint: conflict.fingerprint.clone(),
                        resolved: current.is_some(),
                        choice: current.map(|r| r.choice.clone()),
                    }
                })
                .collect();
            (!conflicts.is_empty()).then(|| FileView { path: path.clone(), conflicts })
        })
        .collect();

    WormholeDetail {
        summary: summarize(wormhole, computed),
        sides,
        files,
    }
}

#[derive(Debug, Deserialize)]
pub struct ListWormholesQuery {
    pub project_id: Uuid,
}

pub async fn list_wormholes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListWormholesQuery>,
) -> ApiResult<Vec<WormholeSummary>> {
    let user_id = current_user(&state, &headers)?;
    ensure_member(&state, user_id, query.project_id).await?;

    let mut summaries = Vec::new();
    for wormhole in state.db.list_project_wormholes(query.project_id).await.map_err(internal)? {
        let computed = compute(&state, &wormhole).await?;
        summaries.push(summarize(&wormhole, &computed));
    }
    Ok(Json(ApiResponse::success(summaries)))
}

#[derive(Debug, Deserialize)]
pub struct CreateWormholeRequest {
    pub project_id: Uuid,
    pub name: String,
    pub rift_ids: Vec<Uuid>,
}

pub async fn create_wormhole(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateWormholeRequest>,
) -> ApiResult<WormholeSummary> {
    let user_id = current_user(&state, &headers)?;
    ensure_member(&state, user_id, req.project_id).await?;

    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(fail(StatusCode::BAD_REQUEST, "wormhole name must be 1-255 characters"));
    }

    let mut rift_ids = req.rift_ids.clone();
    rift_ids.sort();
    rift_ids.dedup();
    if rift_ids.len() < 2 {
        return Err(fail(StatusCode::BAD_REQUEST, "a wormhole needs at least two rifts"));
    }
    for rift_id in &rift_ids {
        let rift = state
            .db
            .get_rift(*rift_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("rift {rift_id} not found")))?;
        if rift.project_id != req.project_id {
            return Err(fail(StatusCode::BAD_REQUEST, format!("rift {rift_id} is not in this project")));
        }
    }

    if state
        .db
        .active_wormhole_name_exists(req.project_id, &name)
        .await
        .map_err(internal)?
    {
        return Err(fail(StatusCode::CONFLICT, format!("an active wormhole named '{name}' already exists")));
    }

    let mut live = Vec::with_capacity(rift_ids.len());
    for rift_id in &rift_ids {
        live.push(state.sync.storage.get_live_state(*rift_id).await.map_err(internal)?);
    }
    let refs: Vec<&HashMap<PathBuf, String>> = live.iter().collect();
    let base = snapshot_base(&refs);

    let mut base_hashes = HashMap::new();
    for (path, content) in &base {
        let hash = state.sync.storage.store_content(content).await.map_err(internal)?;
        base_hashes.insert(path.to_string_lossy().into_owned(), hash);
    }

    let wormhole = state
        .db
        .create_wormhole(req.project_id, &name, user_id, &base_hashes, &rift_ids)
        .await
        .map_err(internal)?;
    let computed = compute(&state, &wormhole).await?;
    Ok(Json(ApiResponse::success(summarize(&wormhole, &computed))))
}

pub async fn get_wormhole(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxPath(wormhole_id): AxPath<Uuid>,
) -> ApiResult<WormholeDetail> {
    let user_id = current_user(&state, &headers)?;
    let wormhole = load_wormhole(&state, user_id, wormhole_id).await?;
    let computed = compute(&state, &wormhole).await?;
    let sides = side_views(&state, &wormhole).await?;
    Ok(Json(ApiResponse::success(detail(&wormhole, &computed, sides))))
}

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub path: String,
    pub conflict_id: String,
    pub choice: Choice,
}

pub async fn resolve_conflict(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxPath(wormhole_id): AxPath<Uuid>,
    Json(req): Json<ResolveRequest>,
) -> ApiResult<WormholeDetail> {
    let user_id = current_user(&state, &headers)?;
    let wormhole = load_wormhole(&state, user_id, wormhole_id).await?;
    require_active(&wormhole)?;

    let computed = compute(&state, &wormhole).await?;
    let merge = computed
        .merges
        .get(&req.path)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no merge for file {}", req.path)))?;
    let conflict = merge
        .conflicts()
        .find(|conflict| conflict.id == req.conflict_id)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "conflict not found; the rifts may have changed"))?;

    if let Choice::Side(index) = &req.choice {
        if *index >= conflict.sides.len() {
            return Err(fail(StatusCode::BAD_REQUEST, format!("side {index} does not exist for this conflict")));
        }
    }

    let resolution = Resolution {
        fingerprint: conflict.fingerprint.clone(),
        choice: req.choice,
    };
    state
        .db
        .upsert_wormhole_resolution(wormhole.id, &req.path, &req.conflict_id, &resolution)
        .await
        .map_err(internal)?;

    let computed = compute(&state, &wormhole).await?;
    let sides = side_views(&state, &wormhole).await?;
    Ok(Json(ApiResponse::success(detail(&wormhole, &computed, sides))))
}

#[derive(Debug, Default, Deserialize)]
pub struct ConvergeRequest {
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn converge_wormhole(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxPath(wormhole_id): AxPath<Uuid>,
    Json(req): Json<ConvergeRequest>,
) -> ApiResult<ConvergeResponse> {
    let user_id = current_user(&state, &headers)?;
    let wormhole = load_wormhole(&state, user_id, wormhole_id).await?;
    require_active(&wormhole)?;

    let computed = compute(&state, &wormhole).await?;
    let unresolved = computed.unresolved_count();
    if unresolved > 0 {
        return Err(fail(
            StatusCode::CONFLICT,
            format!("{unresolved} unresolved conflict(s) must be resolved before converging"),
        ));
    }

    let rift_name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| wormhole.name.clone());
    if state
        .db
        .rift_name_exists(wormhole.project_id, &rift_name)
        .await
        .map_err(internal)?
    {
        return Err(fail(
            StatusCode::CONFLICT,
            format!("a rift named '{rift_name}' already exists in this project"),
        ));
    }

    let rift = state
        .db
        .create_rift(wormhole.project_id, user_id, Some(rift_name.clone()))
        .await
        .map_err(internal)?;

    let mut files_written = 0;
    let mut files_removed = 0;
    for (path, merge) in &computed.merges {
        let lines = merge
            .render(&computed.resolutions_for(path))
            .ok_or_else(|| internal(format!("unresolved conflict in {path} during converge")))?;
        if lines.is_empty() {
            files_removed += 1;
            continue;
        }
        state
            .sync
            .storage
            .update_live_state(rift.id, PathBuf::from(path), lines.join("\n"))
            .await
            .map_err(internal)?;
        files_written += 1;
    }

    state
        .db
        .set_wormhole_status(wormhole.id, "converged", Some(rift.id))
        .await
        .map_err(internal)?;

    Ok(Json(ApiResponse::success(ConvergeResponse {
        rift_id: rift.id,
        rift_name,
        files_written,
        files_removed,
    })))
}

pub async fn dissolve_wormhole(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxPath(wormhole_id): AxPath<Uuid>,
) -> ApiResult<StatusResponse> {
    let user_id = current_user(&state, &headers)?;
    let wormhole = load_wormhole(&state, user_id, wormhole_id).await?;
    require_active(&wormhole)?;

    state
        .db
        .set_wormhole_status(wormhole.id, "dissolved", None)
        .await
        .map_err(internal)?;
    Ok(Json(ApiResponse::success(StatusResponse {
        id: wormhole.id,
        status: "dissolved".to_string(),
    })))
}
