use anyhow::{anyhow, Result};
use mothership_common::{
    normalize_rel_path,
    protocol::{BeamRequest, BeamResponse, ApiResponse},
    text_diff::{diff_files, file_diff, FileDiff},
    ProjectId, UserId,
};
use std::collections::HashMap;
use tracing::{error, info};
use axum::{
    extract::{Query, State},
    Json,
    http::{HeaderMap, StatusCode},
    response::Json as ResponseJson,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::AppState;
use crate::events;

/// Handle beam request - joining/syncing with a project
pub async fn handle_beam(
    state: &AppState,
    project_id: ProjectId,
    request: BeamRequest,
    user_id: UserId,
) -> Result<BeamResponse> {
    info!("Processing beam request for project: {} with user: {}", project_id, user_id);

    // Verify project exists
    let project = state
        .db
        .get_project(project_id)
        .await?
        .ok_or_else(|| anyhow!("Project not found"))?;

    // Check if user has access to this project
    if !state.db.user_has_project_access(user_id, project_id).await? {
        return Err(anyhow!("User does not have access to this project"));
    }

    // Get or create user's rift for this project
    info!("DEBUG: Checking for existing rift for user {} in project {}", user_id, project_id);
    
    let rift = if let Some(rift_name) = request.rift_name {
        info!("DEBUG: Specific rift name requested: {}", rift_name);
        // Beam into the rift the user asked for, creating it only if it doesn't exist yet
        match state.db.get_user_rift_by_name(project_id, user_id, &rift_name).await {
            Ok(Some(existing_rift)) => {
                info!("Found existing rift '{}': {} for user {} in project: {}", rift_name, existing_rift.id, user_id, project.name);
                existing_rift
            }
            Ok(None) => {
                info!("No rift named '{}' for user {} in project: {}, creating it", rift_name, user_id, project.name);
                state.db.create_rift(project_id, user_id, Some(rift_name)).await?
            }
            Err(e) => {
                error!("Error checking for rift '{}': {}", rift_name, e);
                return Err(anyhow!("Database error checking for rift: {}", e));
            }
        }
    } else {
        info!("DEBUG: No specific rift name, checking for default rift");
        // Check if user already has a default rift for this project
        match state.db.get_user_rift(project_id, user_id).await {
            Ok(Some(existing_rift)) => {
                info!("Found existing default rift: {} for user {} in project: {}", existing_rift.id, user_id, project.name);
                existing_rift
            }
            Ok(None) => {
                info!("No existing default rift found, creating new default rift for user {} in project: {}", user_id, project.name);
                state.db.create_rift(project_id, user_id, None).await?
            }
            Err(e) => {
                error!("Error checking for existing default rift: {}", e);
                return Err(anyhow!("Database error checking for default rift: {}", e));
            }
        }
    };

    info!("Using rift: {} for user {} in project: {}", rift.id, user_id, project.name);

    // Generate WebSocket URL for real-time sync
    // Use WEBSOCKET_BASE_URL environment variable for production deployments (e.g., Cloudflare tunnels)
    // Falls back to server config for local development
    let websocket_url = if let Ok(base_url) = std::env::var("WEBSOCKET_BASE_URL") {
        // Production: use environment variable (e.g., "wss://api.mothership.rest")
        format!("{}/ws/{}", base_url.trim_end_matches('/'), rift.id)
    } else {
        // Development: use server config
        let protocol = if state.config.server.host == "127.0.0.1" || state.config.server.host == "localhost" {
            "ws"
        } else {
            "wss"
        };
        
        let host = if state.config.server.host == "0.0.0.0" {
            "localhost"
        } else {
            &state.config.server.host
        };
        
        format!("{}://{}:{}/ws/{}", protocol, host, state.config.server.port, rift.id)
    };

    // For now, always require initial sync
    let initial_sync_required = true;
    let checkpoint_count = 0; // TODO: Get actual checkpoint count

    Ok(BeamResponse {
        project_id,
        rift_id: rift.id,
        websocket_url,
        initial_sync_required,
        checkpoint_count,
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftInfo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub author: String,
    pub file_count: usize,
    pub is_conflict_rift: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRiftRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    /// Rift whose current files the new rift starts with
    #[serde(default)]
    pub fork_from: Option<Uuid>,
    /// Private rifts are hidden from anyone who is not a collaborator
    #[serde(default)]
    pub is_private: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwitchRiftRequest {
    pub rift_name: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftAccessRequest {
    pub rift_name: String,
    pub project_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftAccessRequestQuery {
    pub rift_name: String,
    pub project_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftMemberRequest {
    pub rift_name: String,
    pub project_id: Uuid,
    pub username: String,
    /// `true` removes the member; `false` (default) adds them
    #[serde(default)]
    pub remove: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftAccessRequestInfo {
    pub user_id: Uuid,
    pub username: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RiftDiffQuery {
    /// A rift name, or `checkpoint:<id>`
    pub from: String,
    /// A rift name, or `checkpoint:<id>`
    pub to: String,
    /// Include unified diff text for every changed file
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Deserialize)]
pub struct RiftFileDiffQuery {
    pub from: String,
    pub to: String,
    pub path: String,
}

// Real rift handlers with proper authentication and database integration
pub async fn list_rifts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<ResponseJson<ApiResponse<Vec<RiftInfo>>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Get user's projects
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut all_rifts = Vec::new();

    // For each project, get the rifts
    for project in projects {
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue, // Skip projects with errors
        };
        let private_ids = state.db.private_rift_ids_for_project(project.id).await.unwrap_or_default();

        for rift in rifts {
            // Private rifts are only visible to their collaborators
            if private_ids.contains(&rift.id) && !rift.collaborators.contains(&user_id) {
                continue;
            }

            // Get file count from storage engine
            let file_count = match state.sync.storage.get_live_state(rift.id).await {
                Ok(files) => files.len(),
                Err(_) => 0, // Default to 0 if storage error
            };

            // Get user info for author
            let author = match state.db.get_user(rift.collaborators.first().copied().unwrap_or(user_id)).await {
                Ok(Some(user)) => user.username,
                _ => "Unknown".to_string(),
            };

            let is_conflict_rift = mothership_common::is_conflict_rift_name(&rift.name);
            let rift_info = RiftInfo {
                id: rift.id,
                project_id: rift.project_id,
                name: rift.name,
                description: None, // TODO: Add description field to database
                created_at: rift.created_at,
                author,
                file_count,
                is_conflict_rift,
            };

            all_rifts.push(rift_info);
        }
    }

    Ok(ResponseJson(ApiResponse::success(all_rifts)))
}

pub async fn create_rift(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateRiftRequest>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;

    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Validate rift name
    if !is_valid_rift_name(&req.name) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Use the requested project when given; older clients fall back to the user's first project
    let project_id = match req.project_id {
        Some(project_id) => {
            if !state.db.user_has_project_access(user_id, project_id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
                return Err(StatusCode::FORBIDDEN);
            }
            project_id
        }
        None => {
            let projects = match state.db.get_user_projects(user_id).await {
                Ok(projects) => projects,
                Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
            };
            match projects.first() {
                Some(project) => project.id,
                None => return Err(StatusCode::BAD_REQUEST),
            }
        }
    };

    if state.db.rift_name_exists(project_id, &req.name).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        let message = format!("A rift named '{}' already exists in this project", req.name);
        return Ok((StatusCode::CONFLICT, ResponseJson(ApiResponse::<Uuid>::error(message))).into_response());
    }

    // Validate the fork source before creating anything, so a bad source leaves no empty rift behind
    let fork_source = match req.fork_from {
        Some(source_id) => {
            let source = state.db.get_rift(source_id).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .filter(|source| source.project_id == project_id)
                .ok_or(StatusCode::BAD_REQUEST)?;
            Some(source.id)
        }
        None => None,
    };

    // Create the rift
    let rift = match state.db.create_rift_with_privacy(project_id, user_id, Some(req.name.clone()), req.is_private).await {
        Ok(rift) => rift,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if let Some(source_id) = fork_source {
        let files = state.sync.storage.get_live_state(source_id).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for (path, content) in files {
            state.sync.storage.update_live_state(rift.id, path, content).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        info!("Forked rift {} from {}", rift.id, source_id);
    }

    info!("Created rift: {} for user: {} in project: {}", rift.id, user_id, project_id);

    Ok(ResponseJson(ApiResponse::success(rift.id)).into_response())
}

pub async fn switch_rift(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SwitchRiftRequest>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;

    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Find the rift by name, scoped to the requested project when given
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut matches = Vec::new();
    let mut non_member_match = false;
    for project in projects {
        if req.project_id.is_some_and(|id| id != project.id) {
            continue;
        }
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue,
        };

        for rift in rifts {
            if rift.name != req.rift_name {
                continue;
            }
            if rift.collaborators.contains(&user_id) {
                matches.push(rift);
            } else {
                non_member_match = true;
            }
        }
    }

    let rift = match matches.len() {
        0 if non_member_match => {
            let message = format!("You are not a member of rift '{}'", req.rift_name);
            return Ok((StatusCode::FORBIDDEN, ResponseJson(ApiResponse::<String>::error(message))).into_response());
        }
        0 => return Err(StatusCode::NOT_FOUND),
        1 => matches.remove(0),
        _ => {
            let message = format!("Multiple rifts named '{}' exist; specify the project", req.rift_name);
            return Ok((StatusCode::CONFLICT, ResponseJson(ApiResponse::<String>::error(message))).into_response());
        }
    };

    info!("User {} switched to rift: {} in project: {}", user_id, rift.id, rift.project_id);

    Ok(ResponseJson(ApiResponse::success("Rift switched successfully".to_string())).into_response())
}

/// Resolve the authenticated user from the bearer token
fn authenticated_user(state: &AppState, headers: &HeaderMap) -> Result<Uuid, StatusCode> {
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let token = auth_header.strip_prefix("Bearer ").ok_or(StatusCode::UNAUTHORIZED)?;
    let claims = state.auth.verify_token(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    Uuid::parse_str(&claims.sub).map_err(|_| StatusCode::UNAUTHORIZED)
}

/// Ask to join a rift; the caller must already be a member of its project
pub async fn request_rift_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RiftAccessRequest>,
) -> Result<ResponseJson<ApiResponse<String>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    if !state.db.user_has_project_access(user_id, req.project_id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }

    let (rift_id, _, _) = state.db.find_rift_access_info(req.project_id, &req.rift_name).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if state.db.is_rift_collaborator(rift_id, user_id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Ok(ResponseJson(ApiResponse::success("You already have access to this rift".to_string())));
    }

    state.db.create_rift_access_request(rift_id, user_id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    info!("User {} requested access to rift {}", user_id, rift_id);

    Ok(ResponseJson(ApiResponse::success("Access request submitted".to_string())))
}

/// Pending access requests for a rift; only the rift owner (its creator) may view them
pub async fn list_rift_access_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RiftAccessRequestQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<RiftAccessRequestInfo>>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    let (rift_id, owner, _) = state.db.find_rift_access_info(query.project_id, &query.rift_name).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if owner != Some(user_id) {
        return Err(StatusCode::FORBIDDEN);
    }

    let requests = state.db.list_rift_access_requests(rift_id).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .map(|(user_id, username, requested_at)| RiftAccessRequestInfo { user_id, username, requested_at })
        .collect();

    Ok(ResponseJson(ApiResponse::success(requests)))
}

/// Add or remove a rift member; only the rift owner may manage members
pub async fn manage_rift_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RiftMemberRequest>,
) -> Result<ResponseJson<ApiResponse<String>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;

    let (rift_id, owner, _) = state.db.find_rift_access_info(req.project_id, &req.rift_name).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if owner != Some(user_id) {
        return Err(StatusCode::FORBIDDEN);
    }

    let target = state.db.get_user_by_username(&req.username).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if req.remove {
        state.db.remove_rift_member(rift_id, target.id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        info!("Owner {} removed {} from rift {}", user_id, target.id, rift_id);
        events::record(&state.db, rift_id, Some(user_id), events::ACCESS_CHANGE, serde_json::json!({
            "action": "remove",
            "target_user_id": target.id,
            "target_username": req.username,
        }));
        Ok(ResponseJson(ApiResponse::success(format!("Removed {} from '{}'", req.username, req.rift_name))))
    } else {
        state.db.add_rift_member(req.project_id, rift_id, target.id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        info!("Owner {} added {} to rift {}", user_id, target.id, rift_id);
        events::record(&state.db, rift_id, Some(user_id), events::ACCESS_CHANGE, serde_json::json!({
            "action": "add",
            "target_user_id": target.id,
            "target_username": req.username,
        }));
        Ok(ResponseJson(ApiResponse::success(format!("Added {} to '{}'", req.username, req.rift_name))))
    }
}

pub async fn get_current_rift(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<ResponseJson<ApiResponse<Option<RiftInfo>>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Get user's projects and find the first one with a rift
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    for project in projects {
        if let Ok(Some(rift)) = state.db.get_user_rift(project.id, user_id).await {
            // Get file count from storage engine
            let file_count = match state.sync.storage.get_live_state(rift.id).await {
                Ok(files) => files.len(),
                Err(_) => 0,
            };

            // Get user info for author
            let author = match state.db.get_user(rift.collaborators.first().copied().unwrap_or(user_id)).await {
                Ok(Some(user)) => user.username,
                _ => "Unknown".to_string(),
            };

            let rift_info = RiftInfo {
                id: rift.id,
                project_id: rift.project_id,
                name: rift.name,
                description: None,
                created_at: rift.created_at,
                author,
                file_count,
                is_conflict_rift: false,
            };

            return Ok(ResponseJson(ApiResponse::success(Some(rift_info))));
        }
    }

    Ok(ResponseJson(ApiResponse::success(None)))
}

/// Files on one side of a diff: a rift's live state, or the full state at a checkpoint.
/// A side is either a rift name the caller collaborates on, or `checkpoint:<id>`.
async fn diff_side(state: &AppState, user_id: Uuid, spec: &str) -> Result<HashMap<PathBuf, String>, StatusCode> {
    if let Some(id) = spec.strip_prefix("checkpoint:") {
        let checkpoint_id = Uuid::parse_str(id).map_err(|_| StatusCode::BAD_REQUEST)?;
        let checkpoint = state
            .sync
            .storage
            .load_checkpoint(checkpoint_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;
        crate::chat::ensure_rift_access(&state.db, checkpoint.rift_id, user_id)
            .await
            .map_err(|e| crate::chat::status_for(&e))?;
        return state
            .sync
            .storage
            .get_checkpoint_files(checkpoint_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    let rift_id = rift_named_for_user(state, user_id, spec).await?;
    state
        .sync
        .storage
        .get_live_state(rift_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// The caller's rift with this name. Rift names repeat across projects, so an ambiguous name is a conflict.
async fn rift_named_for_user(state: &AppState, user_id: Uuid, name: &str) -> Result<Uuid, StatusCode> {
    let projects = state
        .db
        .get_user_projects(user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut matches = Vec::new();
    for project in projects {
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue,
        };
        matches.extend(
            rifts
                .into_iter()
                .filter(|rift| rift.name == name && rift.collaborators.contains(&user_id))
                .map(|rift| rift.id),
        );
    }

    match matches.len() {
        0 => Err(StatusCode::NOT_FOUND),
        1 => Ok(matches[0]),
        _ => Err(StatusCode::CONFLICT),
    }
}

/// GET /api/rifts/diff?from=&to=&full=: one summary per changed file, with unified text when `full`
pub async fn get_rift_diffs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RiftDiffQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<FileDiff>>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;
    let left = diff_side(&state, user_id, &query.from).await?;
    let right = diff_side(&state, user_id, &query.to).await?;
    Ok(ResponseJson(ApiResponse::success(diff_files(&left, &right, query.full))))
}

/// GET /api/rifts/diff/file?from=&to=&path=: the unified diff for one file
pub async fn get_rift_file_diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RiftFileDiffQuery>,
) -> Result<ResponseJson<ApiResponse<FileDiff>>, StatusCode> {
    let user_id = authenticated_user(&state, &headers)?;
    let left = diff_side(&state, user_id, &query.from).await?;
    let right = diff_side(&state, user_id, &query.to).await?;

    let path = normalize_rel_path(std::path::Path::new(&query.path));
    let diff = file_diff(
        &path,
        content_at(&left, &path),
        content_at(&right, &path),
        true,
    )
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(ResponseJson(ApiResponse::success(diff)))
}

/// Looks a file up by its normalized path, so `src\main.rs` and `src/main.rs` match the same file
fn content_at<'a>(files: &'a HashMap<PathBuf, String>, path: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|(p, _)| normalize_rel_path(p) == path)
        .map(|(_, content)| content.as_str())
}

fn is_valid_rift_name(name: &str) -> bool {
    let valid_chars = name.chars().all(|c| {
        c.is_alphanumeric() || c == '-' || c == '_'
    });
    valid_chars && !name.is_empty() && name.len() <= 64
} 