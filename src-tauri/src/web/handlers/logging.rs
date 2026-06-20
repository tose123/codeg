use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::logging as logging_commands;
use crate::commands::logging::{LogFileInfo, LogSettingsView};
use crate::logging::hub::LogRecord;
use crate::logging::{LogLevel, LogSettings};

// Wrapper structs mirror Tauri's named-parameter convention: the frontend sends
// `{ settings }` / `{ limit, minLevel, search }` (camelCase) and in web mode the
// whole JSON body arrives as-is.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLogSettingsParams {
    pub settings: LogSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRecentLogsParams {
    pub limit: usize,
    #[serde(default)]
    pub min_level: Option<LogLevel>,
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadLogFileParams {
    pub name: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

pub async fn get_log_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<LogSettingsView>, AppCommandError> {
    Ok(Json(
        logging_commands::get_log_settings_core(&state.db.conn).await?,
    ))
}

pub async fn set_log_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SetLogSettingsParams>,
) -> Result<Json<LogSettings>, AppCommandError> {
    Ok(Json(
        logging_commands::set_log_settings_core(&state.db.conn, params.settings, &state.emitter)
            .await?,
    ))
}

pub async fn get_recent_logs(
    Json(params): Json<GetRecentLogsParams>,
) -> Result<Json<Vec<LogRecord>>, AppCommandError> {
    Ok(Json(logging_commands::get_recent_logs_core(
        params.limit,
        params.min_level,
        params.search.as_deref(),
    )))
}

pub async fn list_log_files() -> Result<Json<Vec<LogFileInfo>>, AppCommandError> {
    Ok(Json(logging_commands::list_log_files_core()))
}

pub async fn read_log_file(
    Json(params): Json<ReadLogFileParams>,
) -> Result<Json<String>, AppCommandError> {
    Ok(Json(logging_commands::read_log_file_core(
        &params.name,
        params.max_bytes,
    )?))
}
