#![allow(dead_code)]

pub mod wormhole_handlers;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxPath, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use mothership_common::{
    auth::{OAuthRequest, OAuthResponse},
    client::{CheckpointData, RestoreData, RiftAccessRequestInfo, RiftEvent, RiftInfo},
    merge::Resolution,
    protocol::{ApiResponse, BeamRequest, BeamResponse, SyncMessage},
    ChangeType, Checkpoint, FileChange, Project, ProjectSettings,
};
use serde::Deserialize;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

pub const TEST_TOKEN: &str = "test-valid-oauth-token-123456789012345678901234567890";

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ms-e2e-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
pub struct StoredRift {
    pub info: RiftInfo,
    pub files: HashMap<PathBuf, String>,
}

#[derive(Clone)]
pub struct StoredWormhole {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub status: String,
    pub created_at: chrono::DateTime<Utc>,
    pub rift_ids: Vec<Uuid>,
    pub disabled_rift_ids: Vec<Uuid>,
    pub converted_rift_id: Option<Uuid>,
    pub base: HashMap<PathBuf, String>,
    pub resolutions: HashMap<String, HashMap<String, Resolution>>,
}

#[derive(Default)]
pub struct InnerState {
    pub projects: HashMap<Uuid, Project>,
    pub rifts: HashMap<Uuid, StoredRift>,
    pub active_rift_by_project: HashMap<Uuid, Uuid>,
    pub checkpoints: HashMap<Uuid, Vec<(Checkpoint, HashMap<PathBuf, String>)>>,
    pub wormholes: HashMap<Uuid, StoredWormhole>,
    pub force_unsafe_path_on_join: bool,
    /// Username of the caller. Tests switch this to act as another user.
    pub caller: String,
    pub private_rifts: HashSet<Uuid>,
    pub rift_members: HashMap<Uuid, HashSet<String>>,
    /// Pending access requests per rift: (username, requested_at).
    pub access_requests: HashMap<Uuid, Vec<(String, chrono::DateTime<Utc>)>>,
    /// Event log per rift, oldest first. Tests append to it directly.
    pub rift_events: HashMap<Uuid, Vec<RiftEvent>>,
}

/// A rift's owner is its author. Private rifts are visible to the owner and to members.
fn can_access_rift(inner: &InnerState, rift_id: Uuid) -> bool {
    let Some(rift) = inner.rifts.get(&rift_id) else {
        return false;
    };
    if !inner.private_rifts.contains(&rift_id) {
        return true;
    }
    rift.info.author == inner.caller
        || inner.rift_members.get(&rift_id).is_some_and(|m| m.contains(&inner.caller))
}

fn find_rift_id(inner: &InnerState, project_id: Uuid, name: &str) -> Option<Uuid> {
    inner
        .rifts
        .values()
        .find(|r| r.info.project_id == Some(project_id) && r.info.name == name)
        .map(|r| r.info.id)
}

#[derive(Clone)]
pub struct MockServerState {
    pub inner: Arc<RwLock<InnerState>>,
    pub tx: broadcast::Sender<(String, SyncMessage)>,
    pub base_url: Arc<RwLock<String>>,
}

pub fn check_auth(headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let ok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s == format!("Bearer {TEST_TOKEN}"));
    if ok {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "unauthorized".into()))
    }
}

pub async fn spawn_test_server() -> (String, MockServerState) {
    let (tx, _) = broadcast::channel(256);
    let state = MockServerState {
        inner: Arc::new(RwLock::new(InnerState {
            caller: "tester".into(),
            ..Default::default()
        })),
        tx,
        base_url: Arc::new(RwLock::new(String::new())),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/max-upload-size", get(|| async { Json(ApiResponse::success(1024 * 1024u64)) }))
        .route("/auth/oauth/start", post(oauth_start))
        .route("/auth/check", get(auth_check))
        .route("/gateway/create", post(create_gateway))
        .route("/projects", get(list_projects))
        .route("/projects/:id", delete(delete_project))
        .route("/projects/:id/files", post(upload_files))
        .route("/projects/:id/beam", post(beam_project))
        .route("/projects/:id/checkpoints", post(create_checkpoint))
        .route("/projects/:id/history", get(checkpoint_history))
        .route("/projects/:id/checkpoints/:cid/restore", post(restore_checkpoint))
        .route("/api/rifts", get(list_rifts).post(create_rift))
        .route("/api/rifts/switch", post(switch_rift))
        .route("/api/rifts/access-requests", get(list_access_requests).post(request_rift_access))
        .route("/api/rifts/:id/events", get(rift_events))
        .route("/api/rifts/members", post(manage_rift_members))
        .route("/api/wormholes", get(wormhole_handlers::list_wormholes).post(wormhole_handlers::create_wormhole))
        .route("/api/wormholes/:id", get(wormhole_handlers::get_wormhole).delete(wormhole_handlers::delete_wormhole))
        .route("/api/wormholes/:id/resolve", post(wormhole_handlers::resolve_conflict))
        .route("/api/wormholes/:id/toggle", post(wormhole_handlers::toggle_rift))
        .route("/api/wormholes/:id/converge", post(wormhole_handlers::converge_wormhole))
        .route("/api/wormholes/:id/files", get(wormhole_handlers::wormhole_files))
        .route("/api/wormholes/:id/rift-files", get(wormhole_handlers::wormhole_rift_files))
        .route("/api/wormholes/:id/dissolve", post(wormhole_handlers::dissolve_wormhole))
        .route("/ws/:rift_id", get(ws_rift))
        .route("/ws/wormholes/:id", get(wormhole_handlers::ws_wormhole))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    *state.base_url.write().await = url.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, state)
}

async fn oauth_start(Json(_req): Json<OAuthRequest>) -> Json<ApiResponse<OAuthResponse>> {
    Json(ApiResponse::success(OAuthResponse {
        auth_url: "https://auth.example.test/login".into(),
        state: "state-123".into(),
        expires_in: 3600,
    }))
}

async fn auth_check(headers: HeaderMap) -> Result<StatusCode, (StatusCode, String)> {
    check_auth(&headers)?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct CreateGatewayReq {
    name: String,
    description: String,
}

async fn create_gateway(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Json(req): Json<CreateGatewayReq>,
) -> Result<Json<ApiResponse<Project>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let project_id = Uuid::new_v4();
    let project = Project {
        id: project_id,
        name: req.name,
        description: req.description,
        members: vec![Uuid::new_v4()],
        created_at: Utc::now(),
        settings: ProjectSettings::default(),
    };
    let main_rift_id = Uuid::new_v4();
    inner.rifts.insert(
        main_rift_id,
        StoredRift {
            info: RiftInfo {
                id: main_rift_id,
                project_id: Some(project_id),
                name: "main".into(),
                description: Some("Main rift".into()),
                created_at: Utc::now(),
                author: "tester".into(),
                file_count: 0,
                is_conflict_rift: false,
            },
            files: HashMap::new(),
        },
    );
    inner.active_rift_by_project.insert(project_id, main_rift_id);
    inner.projects.insert(project_id, project.clone());
    Ok(Json(ApiResponse::success(project)))
}

async fn list_projects(
    State(state): State<MockServerState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<Vec<Project>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    Ok(Json(ApiResponse::success(inner.projects.values().cloned().collect())))
}

async fn delete_project(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<()>>, (StatusCode, String)> {
    check_auth(&headers)?;
    state.inner.write().await.projects.remove(&id);
    Ok(Json(ApiResponse::success(())))
}

#[derive(Deserialize)]
struct UploadReq {
    files: HashMap<PathBuf, String>,
}

async fn upload_files(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UploadReq>,
) -> Result<Json<ApiResponse<usize>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let rift_id = *inner.active_rift_by_project.get(&id).unwrap();
    let rift = inner.rifts.get_mut(&rift_id).unwrap();
    let count = req.files.len();
    for (p, c) in req.files {
        rift.files.insert(PathBuf::from(mothership_common::normalize_rel_path(&p)), c);
    }
    rift.info.file_count = rift.files.len();
    Ok(Json(ApiResponse::success(count)))
}

async fn beam_project(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<BeamRequest>,
) -> Result<Json<ApiResponse<BeamResponse>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let rift_id = match req.rift_name {
        Some(ref name) => inner
            .rifts
            .values()
            .find(|r| r.info.project_id == Some(id) && &r.info.name == name)
            .map(|r| r.info.id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "rift not found".into()))?,
        None => *inner.active_rift_by_project.get(&id).unwrap(),
    };
    let base_url = state.base_url.read().await.replace("http://", "ws://");
    Ok(Json(ApiResponse::success(BeamResponse {
        project_id: id,
        rift_id,
        websocket_url: format!("{base_url}/ws/{rift_id}"),
        initial_sync_required: true,
        checkpoint_count: 0,
    })))
}

async fn list_rifts(
    State(state): State<MockServerState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<Vec<RiftInfo>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    Ok(Json(ApiResponse::success(inner.rifts.values().map(|r| r.info.clone()).collect())))
}

#[derive(Deserialize)]
struct CreateRiftReq {
    name: String,
    description: Option<String>,
    project_id: Option<Uuid>,
    fork_from: Option<Uuid>,
    #[serde(default)]
    is_private: bool,
}

async fn create_rift(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Json(req): Json<CreateRiftReq>,
) -> Result<Json<ApiResponse<Uuid>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let project_id = req.project_id.unwrap();
    let base_rift_id = req.fork_from.or_else(|| inner.active_rift_by_project.get(&project_id).copied()).unwrap();
    let base_files = inner.rifts.get(&base_rift_id).map(|r| r.files.clone()).unwrap_or_default();
    let new_id = Uuid::new_v4();
    inner.rifts.insert(
        new_id,
        StoredRift {
            info: RiftInfo {
                id: new_id,
                project_id: Some(project_id),
                name: req.name,
                description: req.description,
                created_at: Utc::now(),
                author: "tester".into(),
                file_count: base_files.len(),
                is_conflict_rift: false,
            },
            files: base_files,
        },
    );
    if req.is_private {
        inner.private_rifts.insert(new_id);
    }
    Ok(Json(ApiResponse::success(new_id)))
}

#[derive(Deserialize)]
struct SwitchRiftReq {
    rift_name: String,
    project_id: Option<Uuid>,
}

async fn switch_rift(State(state): State<MockServerState>, headers: HeaderMap, Json(req): Json<SwitchRiftReq>) -> Response {
    if let Err(e) = check_auth(&headers) {
        return e.into_response();
    }
    let mut inner = state.inner.write().await;
    let project_id = req.project_id.unwrap();
    let Some(rift_id) = find_rift_id(&inner, project_id, &req.rift_name) else {
        return (StatusCode::NOT_FOUND, "rift not found").into_response();
    };
    if !can_access_rift(&inner, rift_id) {
        let body = ApiResponse::<String>::error("You are not a member of this rift".into());
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }
    inner.active_rift_by_project.insert(project_id, rift_id);
    Json(ApiResponse::success(req.rift_name)).into_response()
}

#[derive(Deserialize)]
struct AccessListQuery {
    rift_name: String,
    project_id: Uuid,
}

async fn list_access_requests(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Query(q): Query<AccessListQuery>,
) -> Result<Json<ApiResponse<Vec<RiftAccessRequestInfo>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let rift_id = find_rift_id(&inner, q.project_id, &q.rift_name)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "rift not found".into()))?;
    if inner.rifts[&rift_id].info.author != inner.caller {
        return Err((StatusCode::FORBIDDEN, "only the rift owner can list access requests".into()));
    }
    let requests = inner
        .access_requests
        .get(&rift_id)
        .map(|reqs| {
            reqs.iter()
                .map(|(username, requested_at)| RiftAccessRequestInfo {
                    user_id: Uuid::new_v4(),
                    username: username.clone(),
                    requested_at: *requested_at,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(ApiResponse::success(requests)))
}

#[derive(Deserialize)]
struct AccessRequestBody {
    rift_name: String,
    project_id: Uuid,
}

async fn request_rift_access(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Json(req): Json<AccessRequestBody>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let rift_id = find_rift_id(&inner, req.project_id, &req.rift_name)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "rift not found".into()))?;
    if !inner.private_rifts.contains(&rift_id) {
        return Err((StatusCode::BAD_REQUEST, "rift is not private".into()));
    }
    if can_access_rift(&inner, rift_id) {
        return Ok(Json(ApiResponse::success("You already have access".into())));
    }
    let caller = inner.caller.clone();
    let pending = inner.access_requests.entry(rift_id).or_default();
    if !pending.iter().any(|(u, _)| *u == caller) {
        pending.push((caller, Utc::now()));
    }
    Ok(Json(ApiResponse::success("Access request sent".into())))
}

#[derive(Deserialize)]
struct RiftMemberBody {
    rift_name: String,
    project_id: Uuid,
    username: String,
    remove: bool,
}

async fn manage_rift_members(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    Json(req): Json<RiftMemberBody>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let rift_id = find_rift_id(&inner, req.project_id, &req.rift_name)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "rift not found".into()))?;
    if inner.rifts[&rift_id].info.author != inner.caller {
        return Err((StatusCode::FORBIDDEN, "only the rift owner can manage members".into()));
    }
    if req.remove {
        if let Some(members) = inner.rift_members.get_mut(&rift_id) {
            members.remove(&req.username);
        }
    } else {
        inner.rift_members.entry(rift_id).or_default().insert(req.username.clone());
        if let Some(pending) = inner.access_requests.get_mut(&rift_id) {
            pending.retain(|(u, _)| *u != req.username);
        }
    }
    Ok(Json(ApiResponse::success("Member updated".into())))
}


#[derive(Deserialize)]
struct CheckpointReq {
    message: Option<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

/// Mirrors the server: a whole-rift checkpoint records every file; a partial one records only the
/// selection and keeps the rest of the previous checkpoint's state.
async fn create_checkpoint(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(project_id): AxPath<Uuid>,
    Json(req): Json<CheckpointReq>,
) -> Result<Json<ApiResponse<CheckpointData>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let rift_id = *inner.active_rift_by_project.get(&project_id).unwrap();
    let live = inner.rifts.get(&rift_id).unwrap().files.clone();
    let previous = inner
        .checkpoints
        .get(&project_id)
        .and_then(|v| v.last())
        .map(|(cp, files)| (cp.id, files.clone()));

    let (recorded, partial, parent, full_state) = match req.paths.as_deref() {
        None => (live.clone(), false, None, live.clone()),
        Some(selectors) => {
            let Some((parent_id, base)) = previous else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "this rift has no checkpoint yet; make a whole-rift checkpoint first".into(),
                ));
            };
            let picked = mothership_common::select_paths(&live, selectors)
                .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
            let mut full = base;
            full.extend(picked.clone());
            (picked, true, Some(parent_id), full)
        }
    };

    let cp_id = Uuid::new_v4();
    let changes: Vec<FileChange> = recorded
        .iter()
        .map(|(p, c)| FileChange {
            path: p.clone(),
            change_type: ChangeType::Modified,
            content_hash: mothership_common::content_hash(c),
            diff: None,
            size: c.len() as u64,
        })
        .collect();
    let file_count = changes.len();
    let cp = Checkpoint {
        id: cp_id,
        rift_id,
        author: Uuid::new_v4(),
        timestamp: Utc::now(),
        changes,
        parent,
        message: req.message,
        auto_generated: false,
        partial,
    };
    inner.checkpoints.entry(project_id).or_default().push((cp, full_state));
    Ok(Json(ApiResponse::success(CheckpointData {
        checkpoint_id: cp_id,
        file_count,
    })))
}

async fn checkpoint_history(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(project_id): AxPath<Uuid>,
) -> Result<Json<ApiResponse<Vec<Checkpoint>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    let list = inner
        .checkpoints
        .get(&project_id)
        .map(|v| v.iter().map(|(c, _)| c.clone()).collect())
        .unwrap_or_default();
    Ok(Json(ApiResponse::success(list)))
}

/// Mirrors the server's paging: `latest`, `before`, or forward reads from `after`. It does not
/// long-poll; the server tests cover `wait`.
async fn rift_events(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath(rift_id): AxPath<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<RiftEvent>>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let inner = state.inner.read().await;
    if !can_access_rift(&inner, rift_id) {
        return Err((StatusCode::FORBIDDEN, "not a rift collaborator".into()));
    }

    let number = |key: &str| params.get(key).and_then(|v| v.parse::<i64>().ok());
    let limit = number("limit").unwrap_or(200).clamp(1, 1000) as usize;
    let events = inner.rift_events.get(&rift_id).cloned().unwrap_or_default();

    let selected: Vec<RiftEvent> = if params.get("latest").is_some_and(|v| v == "true") {
        events[events.len().saturating_sub(limit)..].to_vec()
    } else if let Some(before) = number("before") {
        let older: Vec<RiftEvent> = events.into_iter().filter(|e| e.id < before).collect();
        older[older.len().saturating_sub(limit)..].to_vec()
    } else {
        let after = number("after").unwrap_or(0);
        events.into_iter().filter(|e| e.id > after).take(limit).collect()
    };
    Ok(Json(ApiResponse::success(selected)))
}

async fn restore_checkpoint(
    State(state): State<MockServerState>,
    headers: HeaderMap,
    AxPath((project_id, cid)): AxPath<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<RestoreData>>, (StatusCode, String)> {
    check_auth(&headers)?;
    let mut inner = state.inner.write().await;
    let (cp, files) = inner
        .checkpoints
        .get(&project_id)
        .and_then(|v| v.iter().find(|(c, _)| c.id == cid))
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "checkpoint not found".into()))?;
    let rift_id = *inner.active_rift_by_project.get(&project_id).unwrap();
    inner.rifts.get_mut(&rift_id).unwrap().files = files.clone();
    Ok(Json(ApiResponse::success(RestoreData { checkpoint: cp, files })))
}

async fn ws_rift(
    ws: WebSocketUpgrade,
    State(state): State<MockServerState>,
    AxPath(rift_id): AxPath<Uuid>,
) -> Response {
    ws.on_upgrade(move |mut socket: WebSocket| async move {
        while let Some(Ok(Message::Text(text))) = socket.recv().await {
            if let Ok(SyncMessage::JoinRift { .. }) = serde_json::from_str::<SyncMessage>(&text) {
                let inner = state.inner.read().await;
                let mut current_files = inner.rifts.get(&rift_id).map(|r| r.files.clone()).unwrap_or_default();
                if inner.force_unsafe_path_on_join {
                    current_files.insert(PathBuf::from("../evil.txt"), "bad".into());
                    current_files.insert(PathBuf::from(".mothership/project.json"), "bad".into());
                }
                let reply = SyncMessage::RiftJoined {
                    rift_id,
                    current_files,
                    participants: vec!["tester".into()],
                    last_checkpoint: None,
                };
                let _ = socket.send(Message::Text(serde_json::to_string(&reply).unwrap())).await;
            }
        }
    })
}
