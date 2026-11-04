use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxPath, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use chrono::Utc;
use mothership_common::{
    client::{
        ConvergeResult, RiftFileSides, RiftInfo, WormholeConflict, WormholeDetail, WormholeFile,
        WormholeFileConflicts, WormholeSide, WormholeSummary,
    },
    merge::{merge_rifts, snapshot_base_with_fallback, Choice, FileMerge, Resolution},
    protocol::{ApiResponse, SyncMessage},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{check_auth, InnerState, MockServerState, StoredRift, StoredWormhole};

fn compute_merges(inner: &InnerState, wh: &StoredWormhole) -> BTreeMap<String, FileMerge> {
    let mut live = Vec::with_capacity(wh.rift_ids.len());
    for rift_id in &wh.rift_ids {
        if !wh.disabled_rift_ids.contains(rift_id) {
            live.push(inner.rifts.get(rift_id).map(|r| r.files.clone()).unwrap_or_default());
        } else {
            live.push(wh.base.clone());
        }
    }
    let refs: Vec<&HashMap<PathBuf, String>> = live.iter().collect();
    merge_rifts(&wh.base, &refs)
        .into_iter()
        .map(|(p, m)| (p.to_string_lossy().into_owned(), m))
        .collect()
}

fn build_detail(inner: &InnerState, wh: &StoredWormhole) -> WormholeDetail {
    let merges = compute_merges(inner, wh);
    let mut unresolved_conflicts = 0usize;
    let mut files = Vec::new();
    for (path, merge) in &merges {
        let res_map = wh.resolutions.get(path).cloned().unwrap_or_default();
        if wh.status == "active" {
            unresolved_conflicts += merge.unresolved(&res_map).len();
        }
        let conflicts: Vec<WormholeConflict> = merge
            .conflicts()
            .map(|c| {
                let current = res_map.get(&c.id).filter(|r| r.fingerprint == c.fingerprint);
                WormholeConflict {
                    id: c.id.clone(),
                    base_start: c.base_start,
                    base_end: c.base_end,
                    sides: c.sides.clone(),
                    fingerprint: c.fingerprint.clone(),
                    resolved: current.is_some(),
                    choice: current.map(|r| r.choice.clone()),
                }
            })
            .collect();
        if !conflicts.is_empty() {
            files.push(WormholeFileConflicts { path: path.clone(), conflicts });
        }
    }
    let sides = wh
        .rift_ids
        .iter()
        .enumerate()
        .map(|(index, rid)| WormholeSide {
            index,
            rift_id: *rid,
            rift_name: inner.rifts.get(rid).map(|r| r.info.name.clone()).unwrap_or_default(),
            enabled: !wh.disabled_rift_ids.contains(rid),
        })
        .collect();
    WormholeDetail {
        summary: WormholeSummary {
            id: wh.id,
            project_id: wh.project_id,
            name: wh.name.clone(),
            status: wh.status.clone(),
            created_at: wh.created_at,
            rift_ids: wh.rift_ids.clone(),
            disabled_rift_ids: wh.disabled_rift_ids.clone(),
            converted_rift_id: wh.converted_rift_id,
            unresolved_conflicts,
        },
        sides,
        files,
    }
}

#[derive(Deserialize)]
pub struct ListQ {
    pub project_id: Uuid,
}

pub async fn list_wormholes(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Query(q): Query<ListQ>,
) -> Result<Json<ApiResponse<Vec<WormholeSummary>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let list = inner
        .wormholes
        .values()
        .filter(|w| w.project_id == q.project_id)
        .map(|w| build_detail(&inner, w).summary)
        .collect();
    Ok(Json(ApiResponse::success(list)))
}

#[derive(Deserialize)]
pub struct CreateWhReq {
    pub project_id: Uuid,
    pub name: String,
    pub rift_ids: Vec<Uuid>,
}

pub async fn create_wormhole(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Json(req): Json<CreateWhReq>,
) -> Result<Json<ApiResponse<WormholeSummary>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let mut rift_ids = req.rift_ids;
    rift_ids.sort();
    rift_ids.dedup();
    let live: Vec<HashMap<PathBuf, String>> = rift_ids
        .iter()
        .map(|id| inner.rifts.get(id).map(|r| r.files.clone()).unwrap_or_default())
        .collect();
    let refs: Vec<&HashMap<PathBuf, String>> = live.iter().collect();
    let main_base = inner
        .rifts
        .values()
        .find(|r| r.info.project_id == Some(req.project_id) && r.info.name == "main")
        .map(|r| r.files.clone());
    let base = snapshot_base_with_fallback(&refs, main_base.as_ref());
    let wh = StoredWormhole {
        id: Uuid::new_v4(),
        project_id: req.project_id,
        name: req.name,
        status: "active".into(),
        created_at: Utc::now(),
        rift_ids,
        disabled_rift_ids: Vec::new(),
        converted_rift_id: None,
        base,
        resolutions: HashMap::new(),
    };
    let summary = build_detail(&inner, &wh).summary;
    inner.wormholes.insert(wh.id, wh);
    Ok(Json(ApiResponse::success(summary)))
}

pub async fn get_wormhole(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<WormholeDetail>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let wh = inner.wormholes.get(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    Ok(Json(ApiResponse::success(build_detail(&inner, wh))))
}

#[derive(Deserialize)]
pub struct ResolveReq {
    pub path: String,
    pub conflict_id: String,
    pub choice: Choice,
}

pub async fn resolve_conflict(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<ResolveReq>,
) -> Result<Json<ApiResponse<WormholeDetail>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let merges = {
        let wh = inner.wormholes.get(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
        compute_merges(&inner, wh)
    };
    let conflict = merges
        .get(&req.path)
        .and_then(|m| m.conflicts().find(|c| c.id == req.conflict_id))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "conflict not found".into()))?;
    let fp = conflict.fingerprint.clone();
    let wh = inner.wormholes.get_mut(&id).unwrap();
    wh.resolutions.entry(req.path).or_default().insert(
        req.conflict_id,
        Resolution { fingerprint: fp, choice: req.choice },
    );
    let detail = build_detail(&inner, inner.wormholes.get(&id).unwrap());
    Ok(Json(ApiResponse::success(detail)))
}

#[derive(Deserialize)]
pub struct ToggleReq {
    pub rift_id: Uuid,
    pub enabled: Option<bool>,
}

pub async fn toggle_rift(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<ToggleReq>,
) -> Result<Json<ApiResponse<WormholeDetail>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let wh = inner.wormholes.get_mut(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    if !wh.rift_ids.contains(&req.rift_id) {
        return Err((StatusCode::BAD_REQUEST, "rift not in wormhole".into()));
    }
    let currently_enabled = !wh.disabled_rift_ids.contains(&req.rift_id);
    let next = req.enabled.unwrap_or(!currently_enabled);
    if next {
        wh.disabled_rift_ids.retain(|r| *r != req.rift_id);
    } else if !wh.disabled_rift_ids.contains(&req.rift_id) {
        wh.disabled_rift_ids.push(req.rift_id);
    }
    let detail = build_detail(&inner, inner.wormholes.get(&id).unwrap());
    let _ = state.tx.send((format!("wormhole_{id}"), SyncMessage::WormholeUpdated { wormhole_id: id }));
    Ok(Json(ApiResponse::success(detail)))
}

#[derive(Deserialize)]
pub struct ConvergeReq {
    pub name: Option<String>,
}

pub async fn converge_wormhole(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<ConvergeReq>,
) -> Result<Json<ApiResponse<ConvergeResult>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let wh = inner.wormholes.get(&id).cloned().ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    let detail = build_detail(&inner, &wh);
    if detail.summary.unresolved_conflicts > 0 {
        return Err((StatusCode::CONFLICT, "unresolved conflicts remain".into()));
    }
    let merges = compute_merges(&inner, &wh);
    let mut merged_files = HashMap::new();
    let mut files_removed = 0usize;
    for (path, merge) in &merges {
        let res_map = wh.resolutions.get(path).cloned().unwrap_or_default();
        let lines = merge.render(&res_map).unwrap();
        if lines.is_empty() {
            files_removed += 1;
        } else {
            merged_files.insert(PathBuf::from(path), lines.join("\n"));
        }
    }
    let rift_name = req.name.unwrap_or_else(|| wh.name.clone());
    let new_rift_id = Uuid::new_v4();
    let files_written = merged_files.len();
    inner.rifts.insert(
        new_rift_id,
        StoredRift {
            info: RiftInfo {
                id: new_rift_id,
                project_id: Some(wh.project_id),
                name: rift_name.clone(),
                description: None,
                created_at: Utc::now(),
                author: "tester".into(),
                file_count: files_written,
                is_conflict_rift: false,
            },
            files: merged_files,
        },
    );
    let wh_mut = inner.wormholes.get_mut(&id).unwrap();
    wh_mut.status = "converged".into();
    wh_mut.converted_rift_id = Some(new_rift_id);
    Ok(Json(ApiResponse::success(ConvergeResult {
        rift_id: new_rift_id,
        rift_name,
        files_written,
        files_removed,
    })))
}

pub async fn wormhole_files(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<Vec<WormholeFile>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let wh = inner.wormholes.get(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    let detail = build_detail(&inner, wh);
    if detail.summary.unresolved_conflicts > 0 {
        return Err((StatusCode::CONFLICT, "unresolved conflicts remain".into()));
    }
    let merges = compute_merges(&inner, wh);
    let mut out = Vec::new();
    for (path, merge) in &merges {
        let res_map = wh.resolutions.get(path).cloned().unwrap_or_default();
        let lines = merge.render(&res_map).unwrap();
        if !lines.is_empty() {
            out.push(WormholeFile { path: path.clone(), content: lines.join("\n") });
        }
    }
    Ok(Json(ApiResponse::success(out)))
}

pub async fn wormhole_rift_files(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<Vec<RiftFileSides>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let wh = inner.wormholes.get(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    let live: Vec<HashMap<PathBuf, String>> = wh
        .rift_ids
        .iter()
        .map(|rid| inner.rifts.get(rid).map(|r| r.files.clone()).unwrap_or_default())
        .collect();
    let mut all_paths: Vec<PathBuf> = live.iter().flat_map(|s| s.keys().cloned()).collect();
    all_paths.sort();
    all_paths.dedup();
    let mut out = Vec::new();
    for p in all_paths {
        let contents: Vec<Option<String>> = live.iter().map(|s| s.get(&p).cloned()).collect();
        if !contents.windows(2).all(|w| w[0] == w[1]) {
            out.push(RiftFileSides { path: p.to_string_lossy().into_owned(), contents });
        }
    }
    Ok(Json(ApiResponse::success(out)))
}

#[derive(Deserialize, Default)]
pub struct DissolveReq {
    #[serde(default)]
    pub delete: bool,
}

#[derive(Serialize)]
pub struct StatusResp {
    pub id: Uuid,
    pub status: String,
}

pub async fn dissolve_wormhole(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    body: Option<Json<DissolveReq>>,
) -> Result<Json<ApiResponse<StatusResp>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    if body.map(|Json(b)| b.delete).unwrap_or(false) {
        inner.wormholes.remove(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
        return Ok(Json(ApiResponse::success(StatusResp { id, status: "deleted".into() })));
    }
    let wh = inner.wormholes.get_mut(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    wh.status = "dissolved".into();
    Ok(Json(ApiResponse::success(StatusResp { id, status: "dissolved".into() })))
}

pub async fn delete_wormhole(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<StatusResp>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    inner.wormholes.remove(&id).ok_or_else(|| (StatusCode::NOT_FOUND, "wormhole not found".into()))?;
    Ok(Json(ApiResponse::success(StatusResp { id, status: "deleted".into() })))
}

pub async fn ws_wormhole(
    ws: WebSocketUpgrade,
    State(state): State<MockServerState>,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let mut rx = state.tx.subscribe();
    ws.on_upgrade(move |mut socket: WebSocket| async move {
        let chan = format!("wormhole_{id}");
        while let Ok((c, msg)) = rx.recv().await {
            if c == chan {
                let text = serde_json::to_string(&msg).unwrap();
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    })
}
