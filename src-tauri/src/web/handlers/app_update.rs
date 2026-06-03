//! In-place self-update endpoints for the standalone server / Docker
//! runtime: download+verify+swap (`perform_app_update`), relaunch
//! (`restart_app`), and revert (`rollback_app`).
//!
//! All three are gated behind the process-wide `system_op_lock` so a second
//! click can't race a download already in flight. On desktop (Tauri) builds
//! they hard-error — desktop updates through `tauri-plugin-updater`.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::update::runtime::UpdateCapability;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateActionResult {
    /// Version installed (perform) — absent for restart/rollback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the caller should follow up with `restart_app`.
    pub need_restart: bool,
    /// Relaunch delay (ms) the frontend countdown should use.
    pub restart_delay_ms: u64,
    pub capability: UpdateCapability,
}

pub async fn perform_app_update(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<UpdateActionResult>, AppCommandError> {
    perform_impl(state).await.map(Json)
}

pub async fn restart_app(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<UpdateActionResult>, AppCommandError> {
    restart_impl(state).map(Json)
}

pub async fn rollback_app(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<UpdateActionResult>, AppCommandError> {
    rollback_impl(state).await.map(Json)
}

// ─── desktop build: not supported ────────────────────────────────────────

#[cfg(feature = "tauri-runtime")]
async fn perform_impl(_state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    Err(not_supported())
}

#[cfg(feature = "tauri-runtime")]
fn restart_impl(_state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    Err(not_supported())
}

#[cfg(feature = "tauri-runtime")]
async fn rollback_impl(_state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    Err(not_supported())
}

#[cfg(feature = "tauri-runtime")]
fn not_supported() -> AppCommandError {
    AppCommandError::invalid_input("In-place update is only available in server mode")
}

// ─── server build: the real thing ────────────────────────────────────────

#[cfg(not(feature = "tauri-runtime"))]
fn busy() -> AppCommandError {
    AppCommandError::already_exists("An update operation is already in progress")
}

/// Refuse on platforms where in-place self-update is not validated. Windows
/// server self-update is disabled (running-.exe swap + re-exec rebind are
/// untested there); the desktop Windows app updates via tauri-plugin-updater.
#[cfg(not(feature = "tauri-runtime"))]
fn ensure_supported() -> Result<(), AppCommandError> {
    if cfg!(target_os = "windows") {
        return Err(AppCommandError::invalid_input(
            "In-place server self-update is not supported on Windows yet",
        ));
    }
    Ok(())
}

#[cfg(not(feature = "tauri-runtime"))]
async fn perform_impl(state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    use crate::update::install::UpdatePhase;

    ensure_supported()?;

    // Hold the lock for the whole download/verify/swap so a concurrent
    // perform/restart/rollback is rejected rather than racing the swap.
    let _guard = state.system_op_lock.try_lock().map_err(|_| busy())?;

    let emitter = state.emitter.clone();
    let progress = move |phase: UpdatePhase, downloaded: u64, total: Option<u64>| {
        crate::web::event_bridge::emit_event(
            &emitter,
            "app_update_progress",
            serde_json::json!({
                "phase": phase,
                "downloaded": downloaded,
                "total": total,
            }),
        );
    };

    let outcome = crate::update::install::perform_update(&state.data_dir, &progress).await?;

    Ok(UpdateActionResult {
        version: Some(outcome.version),
        need_restart: true,
        restart_delay_ms: crate::update::runtime::restart_delay_ms(),
        capability: crate::update::runtime::capability(),
    })
}

#[cfg(not(feature = "tauri-runtime"))]
fn restart_impl(state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    ensure_supported()?;
    // Just guard against a perform/rollback in flight; the lock is released
    // immediately (the restart itself is fire-and-forget).
    if state.system_op_lock.try_lock().is_err() {
        return Err(busy());
    }
    let restart_delay_ms = crate::update::runtime::restart_delay_ms();
    // Responds first, then exits/re-execs after a short flush delay.
    crate::update::schedule_restart();
    Ok(UpdateActionResult {
        version: None,
        need_restart: false,
        restart_delay_ms,
        capability: crate::update::runtime::capability(),
    })
}

#[cfg(not(feature = "tauri-runtime"))]
async fn rollback_impl(state: Arc<AppState>) -> Result<UpdateActionResult, AppCommandError> {
    ensure_supported()?;
    let _guard = state.system_op_lock.try_lock().map_err(|_| busy())?;
    crate::update::install::rollback()?;
    Ok(UpdateActionResult {
        version: None,
        need_restart: true,
        restart_delay_ms: crate::update::runtime::restart_delay_ms(),
        capability: crate::update::runtime::capability(),
    })
}
