//! Tauri command adapters. Each one is a thin wrapper over an `AppState` method; all
//! validation (workspace-confined paths, stamps, settings) lives in `state.rs`.
//! Blocking work runs on the blocking pool so the async runtime and UI stay responsive.
//!
//! JS argument names are camelCase (`onEvent`, `onOutput`); errors arrive as `{code, message}`.

use crate::state::{
    parse_settings_input, AppError, AppState, DetectedDto, DirEntryDto, ImageDto, ProbeDto, ReadFileResult, StampDto, UiEvent, WorkspaceInfo,
};
use ide_host::settings::Settings;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

type S<'a> = State<'a, Arc<AppState>>;

async fn blocking<T, F>(state: &S<'_>, f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, AppError> + Send + 'static,
{
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || f(st))
        .await
        .map_err(|e| AppError::new("io", format!("task failed: {e}")))?
}

fn channel_sink<T: serde::Serialize + Clone + Send + Sync + 'static>(
    ch: Channel<T>,
) -> impl Fn(&T) -> bool + Send + Sync + 'static {
    move |ev| ch.send(ev.clone()).is_ok()
}

/// Native folder picker (runs in Rust so the webview needs no dialog permission).
/// Returns `null` when the user cancels.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, AppError> {
    tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| AppError::new("io", e.to_string()))
}

#[tauri::command]
pub async fn open_folder(state: S<'_>, path: String) -> Result<WorkspaceInfo, AppError> {
    state.open_folder(&path).await
}

#[tauri::command]
pub async fn list_dir(state: S<'_>, rel: String) -> Result<Vec<DirEntryDto>, AppError> {
    blocking(&state, move |st| st.list_dir(&rel)).await
}

#[tauri::command]
pub async fn read_file(state: S<'_>, rel: String) -> Result<ReadFileResult, AppError> {
    blocking(&state, move |st| st.read_file(&rel)).await
}

#[tauri::command]
pub async fn save_file(state: S<'_>, rel: String, text: String, stamp: Option<StampDto>) -> Result<StampDto, AppError> {
    blocking(&state, move |st| st.save_file(&rel, &text, stamp.as_ref())).await
}

/// Register the webview listener for all UI events without starting a turn.
#[tauri::command]
pub fn subscribe_events(state: S<'_>, on_event: Channel<UiEvent>) {
    state.subscribe(Arc::new(channel_sink(on_event)));
}

/// Run a turn. `on_event` replaces the current listener (same stream as `subscribe_events`).
/// Resolves after the turn; `TurnEnd` is delivered on the channel just before it resolves.
#[tauri::command]
pub async fn agent_prompt(
    state: S<'_>,
    mode: String,
    text: String,
    images: Option<Vec<ImageDto>>,
    on_event: Channel<UiEvent>,
) -> Result<(), AppError> {
    state.set_sink(Arc::new(channel_sink(on_event)));
    state.agent_prompt_with(&mode, &text, &images.unwrap_or_default()).await
}

#[tauri::command]
pub fn agent_cancel(state: S<'_>) {
    state.agent_cancel();
}

#[tauri::command]
pub async fn resolve_approval(state: S<'_>, id: u64, accept: bool) -> Result<(), AppError> {
    state.resolve_approval(id, accept).await
}

/// The UI reports the outcome of an `ApplyEdit` (applied and saved, or why not).
#[tauri::command]
pub async fn search_files(state: S<'_>, query: String, limit: Option<usize>) -> Result<Vec<String>, AppError> {
    blocking(&state, move |st| st.search_files(&query, limit.unwrap_or(50))).await
}

#[tauri::command]
pub fn confirm_edit(state: S<'_>, id: u64, ok: bool, error: Option<String>) -> Result<(), AppError> {
    state.confirm_edit(id, ok, error)
}

#[tauri::command]
pub async fn probe_endpoint(state: S<'_>, url: String, api_key: Option<String>) -> Result<ProbeDto, AppError> {
    state.probe_endpoint(&url, api_key).await
}

#[tauri::command]
pub async fn detect_servers(state: S<'_>) -> Result<Vec<DetectedDto>, AppError> {
    state.detect_servers().await
}

#[tauri::command]
pub fn get_settings(state: S<'_>) -> Settings {
    state.get_settings()
}

#[tauri::command]
pub async fn set_settings(state: S<'_>, settings: serde_json::Value) -> Result<Settings, AppError> {
    state.set_settings(parse_settings_input(settings)?).await
}

#[tauri::command]
pub async fn start_user_terminal(
    state: S<'_>,
    rows: u16,
    cols: u16,
    on_output: Channel<Vec<u8>>,
) -> Result<u64, AppError> {
    state
        .start_user_terminal(rows, cols, Box::new(move |b: Vec<u8>| on_output.send(b).is_ok()))
        .await
}

#[tauri::command]
pub async fn terminal_write(state: S<'_>, id: u64, data: Vec<u8>) -> Result<(), AppError> {
    blocking(&state, move |st| st.terminal_write(id, &data)).await
}

#[tauri::command]
pub async fn terminal_resize(state: S<'_>, id: u64, rows: u16, cols: u16) -> Result<(), AppError> {
    blocking(&state, move |st| st.terminal_resize(id, rows, cols)).await
}

#[tauri::command]
pub fn close_terminal(state: S<'_>, id: u64) {
    state.close_terminal(id);
}

#[tauri::command]
pub fn copy_diagnostics(state: S<'_>) -> String {
    state.diagnostics()
}

#[tauri::command]
pub fn recovery_save(state: S<'_>, rel: String, text: String) -> Result<(), AppError> {
    state.recovery_save(&rel, &text)
}

#[tauri::command]
pub fn recovery_list(state: S<'_>) -> Result<Vec<ide_host::recovery::RecoveredBuffer>, AppError> {
    state.recovery_list()
}

#[tauri::command]
pub fn recovery_clear(state: S<'_>, rel: String) -> Result<(), AppError> {
    state.recovery_clear(&rel)
}

#[tauri::command]
pub async fn find_local_models(state: S<'_>, dirs: Vec<String>) -> Result<Vec<ide_host::models::LocalModel>, AppError> {
    state.find_local_models(dirs).await
}

#[tauri::command]
pub async fn find_engines(state: S<'_>, explicit: Option<String>) -> Result<Vec<ide_host::models::Engine>, AppError> {
    state.find_engines(explicit).await
}

#[tauri::command]
pub async fn model_info(state: S<'_>, path: String) -> Result<crate::state::ModelInfoDto, AppError> {
    state.model_info(path).await
}

#[tauri::command]
pub async fn launch_model(state: S<'_>, opts: ide_host::launcher::LaunchOpts) -> Result<(), AppError> {
    state.launch_model(opts).await
}

#[tauri::command]
pub async fn stop_model(state: S<'_>) -> Result<(), AppError> {
    state.stop_model().await;
    Ok(())
}

#[tauri::command]
pub async fn model_server_status(state: S<'_>) -> Result<ide_host::launcher::ServerStatus, AppError> {
    Ok(state.model_server_status().await)
}
