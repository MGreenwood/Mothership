use anyhow::{anyhow, Result};
use mothership_common::{
    protocol::{BeamRequest, BeamResponse, ApiResponse},
    ProjectId, UserId,
};
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
    info!("🔍 DEBUG: Checking for existing rift for user {} in project {}", user_id, project_id);
    
    let rift = if let Some(rift_name) = request.rift_name {
        info!("🔍 DEBUG: Specific rift name requested: {}", rift_name);
        // Check if rift with specific name exists for user
        match state.db.get_user_rift(project_id, user_id).await {
            Ok(Some(existing_rift)) => {
                info!("✅ Found existing rift: {} for user {} in project: {}", existing_rift.id, user_id, project.name);
                existing_rift
            }
            Ok(None) => {
                info!("❌ No existing rift found, creating new rift with name '{}' for user {} in project: {}", rift_name, user_id, project.name);
                state.db.create_rift(project_id, user_id, Some(rift_name)).await?
            }
            Err(e) => {
                error!("🚨 Error checking for existing rift: {}", e);
                return Err(anyhow!("Database error checking for rift: {}", e));
            }
        }
    } else {
        info!("🔍 DEBUG: No specific rift name, checking for default rift");
        // Check if user already has a default rift for this project
        match state.db.get_user_rift(project_id, user_id).await {
            Ok(Some(existing_rift)) => {
                info!("✅ Found existing default rift: {} for user {} in project: {}", existing_rift.id, user_id, project.name);
                existing_rift
            }
            Ok(None) => {
                info!("❌ No existing default rift found, creating new default rift for user {} in project: {}", user_id, project.name);
                state.db.create_rift(project_id, user_id, None).await?
            }
            Err(e) => {
                error!("🚨 Error checking for existing default rift: {}", e);
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwitchRiftRequest {
    pub rift_name: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RiftDiff {
    pub path: PathBuf,
    pub change_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct RiftDiffQuery {
    pub from: String,
    pub to: String,
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

        for rift in rifts {
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

            let rift_info = RiftInfo {
                id: rift.id,
                project_id: rift.project_id,
                name: rift.name,
                description: None, // TODO: Add description field to database
                created_at: rift.created_at,
                author,
                file_count,
                is_conflict_rift: false, // TODO: Add conflict rift detection
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
    let rift = match state.db.create_rift(project_id, user_id, Some(req.name.clone())).await {
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
    for project in projects {
        if req.project_id.is_some_and(|id| id != project.id) {
            continue;
        }
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue,
        };

        for rift in rifts {
            if rift.name == req.rift_name && rift.collaborators.contains(&user_id) {
                matches.push(rift);
            }
        }
    }

    let rift = match matches.len() {
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

pub async fn get_rift_diffs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RiftDiffQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<RiftDiff>>>, StatusCode> {
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

    // Find the rifts by name in user's projects
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut from_rift = None;
    let mut to_rift = None;

    for project in projects {
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue,
        };

        for rift in rifts {
            if rift.collaborators.contains(&user_id) {
                if rift.name == query.from {
                    from_rift = Some(rift.clone());
                }
                if rift.name == query.to {
                    to_rift = Some(rift.clone());
                }
            }
        }
    }

    let from_rift = from_rift.ok_or(StatusCode::NOT_FOUND)?;
    let to_rift = to_rift.ok_or(StatusCode::NOT_FOUND)?;

    // Get file states from storage engine
    let from_files = match state.sync.storage.get_live_state(from_rift.id).await {
        Ok(files) => files,
        Err(_) => return Ok(ResponseJson(ApiResponse::success(vec![]))),
    };

    let to_files = match state.sync.storage.get_live_state(to_rift.id).await {
        Ok(files) => files,
        Err(_) => return Ok(ResponseJson(ApiResponse::success(vec![]))),
    };

    // Calculate differences
    let mut diffs = Vec::new();
    let mut all_paths = std::collections::HashSet::new();
    
    for path in from_files.keys() {
        all_paths.insert(path.clone());
    }
    for path in to_files.keys() {
        all_paths.insert(path.clone());
    }

    for path in all_paths {
        let from_content = from_files.get(&path);
        let to_content = to_files.get(&path);

        if from_content != to_content {
            // Simple change count - in reality this would be more sophisticated
            let change_count = 1; // Placeholder
            diffs.push(RiftDiff {
                path,
                change_count,
            });
        }
    }

    Ok(ResponseJson(ApiResponse::success(diffs)))
}

fn is_valid_rift_name(name: &str) -> bool {
    let valid_chars = name.chars().all(|c| {
        c.is_alphanumeric() || c == '-' || c == '_'
    });
    valid_chars && !name.is_empty() && name.len() <= 64
} 