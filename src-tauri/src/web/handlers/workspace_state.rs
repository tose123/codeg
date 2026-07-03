use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::workspace_state as workspace_state_commands;
use crate::workspace_state::WorkspaceSnapshotResponse;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRootPathParams {
    pub root_path: String,
    // Absent = legacy full subscriber (tree/git snapshots wanted).
    pub wants_tree_git: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshotParams {
    pub root_path: String,
    pub since_seq: Option<u64>,
}

pub async fn start_workspace_state_stream(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<WorkspaceRootPathParams>,
) -> Result<Json<WorkspaceSnapshotResponse>, AppCommandError> {
    let result = workspace_state_commands::start_workspace_state_stream_core(
        state.emitter.clone(),
        params.root_path,
        params.wants_tree_git.unwrap_or(true),
    )
    .await?;
    Ok(Json(result))
}

pub async fn stop_workspace_state_stream(
    Json(params): Json<WorkspaceRootPathParams>,
) -> Result<Json<()>, AppCommandError> {
    workspace_state_commands::stop_workspace_state_stream_core(
        params.root_path,
        params.wants_tree_git.unwrap_or(true),
    )
    .await?;
    Ok(Json(()))
}

pub async fn get_workspace_snapshot(
    Json(params): Json<WorkspaceSnapshotParams>,
) -> Result<Json<WorkspaceSnapshotResponse>, AppCommandError> {
    let result =
        workspace_state_commands::get_workspace_snapshot_core(params.root_path, params.since_seq)
            .await?;
    Ok(Json(result))
}
