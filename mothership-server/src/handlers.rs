use anyhow::{anyhow, Result};
use mothership_common::{
    protocol::{BeamRequest, BeamResponse},
    ProjectId,
};
use tracing::info;
use uuid::Uuid;

use crate::AppState;

/// Handle beam request - joining/syncing with a project
pub async fn handle_beam(
    state: &AppState,
    project_id: ProjectId,
    request: BeamRequest,
) -> Result<BeamResponse> {
    info!("Processing beam request for project: {}", project_id);

    // Verify project exists
    let project = state
        .db
        .get_project(project_id)
        .await?
        .ok_or_else(|| anyhow!("Project not found"))?;

    // TODO: Get user from auth token
    // For now, use a demo user (Alice from demo data)
    let user_id = Uuid::new_v4(); // In real implementation, extract from JWT token

    // Check if user has access to this project
    if !state.db.user_has_project_access(user_id, project_id).await? {
        return Err(anyhow!("User does not have access to this project"));
    }

    // Create or get user's rift for this project
    let rift = if let Some(rift_name) = request.rift_name {
        // TODO: Check if rift exists, create if not
        state.db.create_rift(project_id, user_id, Some(rift_name)).await?
    } else {
        // Create default rift for user
        state.db.create_rift(project_id, user_id, None).await?
    };

    info!("Created/found rift: {} for user in project: {}", rift.id, project.name);

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