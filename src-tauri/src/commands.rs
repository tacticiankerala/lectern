//! The commands the UI invokes, each a thin call into `AppState`. Tauri runs synchronous
//! commands on the main thread, so every command that may touch the file system (or wait on a
//! share) is async and runs its work on the blocking pool. Arguments arrive camelCased.

use std::sync::Arc;

use lectern_core::ipc::{
    Candidate, FollowResult, FollowTarget, LibraryPayload, OpenResult, RecentEntry, SavedPosition,
    Settings, SettingsPatch, StartupPayload, UpdateInfo, UserOpen,
};
use lectern_core::search::FileHits;
use tauri::{AppHandle, State, WebviewWindow};

use crate::state::AppState;
use crate::updater::{self, Updates};
use crate::{app, win};

type Shared<'a> = State<'a, Arc<AppState>>;

/// Runs `f` on the blocking pool.
async fn blocking<T, F>(state: &Arc<AppState>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Arc<AppState>) -> T + Send + 'static,
{
    let state = Arc::clone(state);
    tauri::async_runtime::spawn_blocking(move || f(&state))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn startup(state: Shared<'_>) -> Result<StartupPayload, String> {
    blocking(state.inner(), |s| s.startup()).await
}

#[tauri::command]
pub async fn open_document(path: String, state: Shared<'_>) -> Result<OpenResult, String> {
    blocking(state.inner(), move |s| s.open_document(&path)).await
}

/// Opens a file or folder the user chose (the file dialog, a drop, Add folder) as a launch argument
/// would: its network host is trusted, and a folder joins the library unless it nests with a root.
#[tauri::command]
pub async fn open_user_path(path: String, state: Shared<'_>) -> Result<UserOpen, String> {
    blocking(state.inner(), move |s| s.open_user_path(&path)).await
}

#[tauri::command]
pub async fn get_library(state: Shared<'_>) -> Result<LibraryPayload, String> {
    Ok(state.library_payload())
}

#[tauri::command]
pub async fn add_root(path: String, state: Shared<'_>) -> Result<LibraryPayload, String> {
    blocking(state.inner(), move |s| s.add_root(&path)).await?
}

#[tauri::command]
pub async fn remove_root(path: String, state: Shared<'_>) -> Result<LibraryPayload, String> {
    blocking(state.inner(), move |s| s.remove_root(&path)).await
}

#[tauri::command]
pub async fn retry_root(path: String, state: Shared<'_>) -> Result<LibraryPayload, String> {
    blocking(state.inner(), move |s| s.retry_root(&path)).await
}

#[tauri::command]
pub async fn quick_open_candidates(state: Shared<'_>) -> Result<Vec<Candidate>, String> {
    Ok(state.quick_open_candidates())
}

#[tauri::command]
pub async fn search(query: String, state: Shared<'_>) -> Result<Vec<FileHits>, String> {
    blocking(state.inner(), move |s| s.search(&query)).await
}

#[tauri::command]
pub async fn follow(target: FollowTarget, state: Shared<'_>) -> Result<FollowResult, String> {
    blocking(state.inner(), move |s| s.follow(&target)).await
}

#[tauri::command]
pub async fn reveal_in_explorer(path: String, state: Shared<'_>) -> Result<(), String> {
    blocking(state.inner(), move |s| s.reveal_in_explorer(&path)).await?
}

#[tauri::command]
pub async fn open_in_editor(
    path: String,
    line: Option<u32>,
    state: Shared<'_>,
) -> Result<(), String> {
    blocking(state.inner(), move |s| s.open_in_editor(&path, line)).await?
}

#[tauri::command]
pub fn get_settings(state: Shared<'_>) -> Settings {
    state.settings()
}

#[tauri::command]
pub async fn set_settings(patch: SettingsPatch, state: Shared<'_>) -> Result<Settings, String> {
    blocking(state.inner(), move |s| s.set_settings(patch)).await
}

#[tauri::command]
pub fn save_position(path: String, position: SavedPosition, state: Shared<'_>) {
    state.save_position(&path, position);
}

/// Drops a file from the recent files for good; returns the recent files left.
#[tauri::command]
pub fn remove_recent(path: String, state: Shared<'_>) -> Vec<RecentEntry> {
    state.remove_recent(&path)
}

#[tauri::command]
pub fn set_chrome_colors(
    bg: String,
    fg: String,
    dark: bool,
    window: WebviewWindow,
) -> Result<(), String> {
    app::apply_chrome(&window, &bg, &fg, dark)
}

#[tauri::command]
pub async fn list_system_fonts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(win::system_font_families)
        .await
        .map_err(|e| e.to_string())
}

/// `first-paint` also quits when Lectern was started with `--exit-after-paint`.
#[tauri::command]
pub async fn perf_mark(name: String, ms: Option<f64>, state: Shared<'_>) -> Result<(), String> {
    blocking(state.inner(), move |s| s.perf_mark(&name, ms)).await
}

/// Asks GitHub Releases for a newer Lectern.
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<Option<UpdateInfo>, String> {
    updater::check(&app, &updates).await
}

/// Installs the update found (and restarts), or for a portable copy opens the Releases page.
#[tauri::command]
pub async fn install_update(app: AppHandle, updates: State<'_, Updates>) -> Result<(), String> {
    updater::install(&app, &updates).await
}

#[tauri::command]
pub async fn show_window(window: WebviewWindow, state: Shared<'_>) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    blocking(state.inner(), |s| s.window_shown()).await
}
