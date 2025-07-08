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
        // Production: use environment variable (e.g., "wss://api.mothershipproject.dev")
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwitchRiftRequest {
    pub rift_name: String,
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
) -> Result<ResponseJson<ApiResponse<Uuid>>, StatusCode> {
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

    // For now, create rift in the first project the user has access to
    // TODO: Add project_id to request or get from context
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if projects.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let project_id = projects[0].id;

    // Create the rift
    let rift = match state.db.create_rift(project_id, user_id, Some(req.name.clone())).await {
        Ok(rift) => rift,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    info!("Created rift: {} for user: {} in project: {}", rift.id, user_id, project_id);

    Ok(ResponseJson(ApiResponse::success(rift.id)))
}

pub async fn switch_rift(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SwitchRiftRequest>,
) -> Result<ResponseJson<ApiResponse<String>>, StatusCode> {
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

    // Find the rift by name in user's projects
    let projects = match state.db.get_user_projects(user_id).await {
        Ok(projects) => projects,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut target_rift = None;
    for project in projects {
        let rifts = match state.db.get_project_rifts(project.id).await {
            Ok(rifts) => rifts,
            Err(_) => continue,
        };

        for rift in rifts {
            if rift.name == req.rift_name && rift.collaborators.contains(&user_id) {
                target_rift = Some(rift);
                break;
            }
        }
        if target_rift.is_some() {
            break;
        }
    }

    let rift = target_rift.ok_or(StatusCode::NOT_FOUND)?;

    info!("User {} switched to rift: {} in project: {}", user_id, rift.id, rift.project_id);

    Ok(ResponseJson(ApiResponse::success("Rift switched successfully".to_string())))
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