use anyhow::{anyhow, Result};
use mothership_common::{
    protocol::{BeamRequest, BeamResponse},
    ProjectId, UserId,
};
use tracing::{error, info};

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
    let websocket_url = format!("ws://localhost:7523/sync/{}", rift.id);

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