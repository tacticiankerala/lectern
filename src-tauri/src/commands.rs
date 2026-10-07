//! The commands the UI invokes, each a thin call into the calling window's `WindowState` (or into
//! the `App`). Tauri runs synchronous commands on the main thread, so every command that may touch
//! the file system (or wait on a share) is async and runs its work on the blocking pool. Arguments
//! arrive camelCased.

use std::sync::Arc;

use lectern_core::ipc::{
    Candidate, FollowResult, FollowTarget, LibraryPayload, OpenResult, OpenWhere, RecentEntry,
    SavedPosition, Settings, SettingsPatch, StartupPayload, UpdateInfo, UserOpen, WorkspaceOutcome,
    WorkspaceSummary,
};
use lectern_core::review::ops::ReviewOp;
use lectern_core::review::view::ReviewPayload;
use lectern_core::search::FileHits;
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::state::{App, WindowState};
use crate::updater::{self, Updates};
use crate::{app, win};

type Shared<'a> = State<'a, Arc<App>>;

/// The answer to a window that has no state of its own yet.
const NOT_READY: &str = "window not ready";

/// The state of the window that invoked the command.
fn window_state(window: &WebviewWindow, app: &App) -> Result<Arc<WindowState>, String> {
    app.window(window.label())
        .ok_or_else(|| NOT_READY.to_owned())
}

/// Runs `f` on the blocking pool.
async fn blocking<S, T, F>(state: Arc<S>, f: F) -> Result<T, String>
where
    S: Send + Sync + 'static,
    T: Send + 'static,
    F: FnOnce(&Arc<S>) -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || f(&state))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn startup(window: WebviewWindow, state: Shared<'_>) -> Result<StartupPayload, String> {
    blocking(window_state(&window, &state)?, |s| s.startup()).await
}

#[tauri::command]
pub async fn open_document(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<OpenResult, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.open_document(&path)
    })
    .await
}

/// Opens a file or folder the user chose (the file dialog, a drop, Add folder) as a launch argument
/// would: its network host is trusted, and a folder joins the library unless it nests with a root.
#[tauri::command]
pub async fn open_user_path(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<UserOpen, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.open_user_path(&path)
    })
    .await
}

#[tauri::command]
pub async fn get_library(
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<LibraryPayload, String> {
    Ok(window_state(&window, &state)?.library_payload())
}

#[tauri::command]
pub async fn add_root(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<LibraryPayload, String> {
    blocking(window_state(&window, &state)?, move |s| s.add_root(&path)).await?
}

#[tauri::command]
pub async fn remove_root(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<LibraryPayload, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.remove_root(&path)
    })
    .await
}

#[tauri::command]
pub async fn retry_root(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<LibraryPayload, String> {
    blocking(window_state(&window, &state)?, move |s| s.retry_root(&path)).await
}

#[tauri::command]
pub async fn quick_open_candidates(
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Vec<Candidate>, String> {
    Ok(window_state(&window, &state)?.quick_open_candidates())
}

#[tauri::command]
pub async fn search(
    query: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Vec<FileHits>, String> {
    blocking(window_state(&window, &state)?, move |s| s.search(&query)).await
}

#[tauri::command]
pub async fn follow(
    target: FollowTarget,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<FollowResult, String> {
    blocking(window_state(&window, &state)?, move |s| s.follow(&target)).await
}

#[tauri::command]
pub async fn reveal_in_explorer(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<(), String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.reveal_in_explorer(&path)
    })
    .await?
}

#[tauri::command]
pub async fn open_in_editor(
    path: String,
    line: Option<u32>,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<(), String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.open_in_editor(&path, line)
    })
    .await?
}

/// The open note's review comments.
#[tauri::command]
pub async fn load_review(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<ReviewPayload, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.load_review(&path)
    })
    .await?
}

/// Saves a change to the open note's review comments; returns them as saved.
#[tauri::command]
pub async fn review_op(
    path: String,
    op: ReviewOp,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<ReviewPayload, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.review_op(&path, op)
    })
    .await?
}

/// The calling window's settings.
#[tauri::command]
pub fn get_settings(window: WebviewWindow, state: Shared<'_>) -> Result<Settings, String> {
    Ok(window_state(&window, &state)?.settings())
}

/// Changes settings from the calling window; returns its settings.
#[tauri::command]
pub async fn set_settings(
    patch: SettingsPatch,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Settings, String> {
    blocking(window_state(&window, &state)?, move |s| {
        s.set_settings(patch)
    })
    .await
}

#[tauri::command]
pub fn save_position(
    path: String,
    position: SavedPosition,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<(), String> {
    window_state(&window, &state)?.save_position(&path, position);
    Ok(())
}

/// Drops a file from the calling window's recent files for good; returns the recent files left.
#[tauri::command]
pub fn remove_recent(
    path: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Vec<RecentEntry>, String> {
    Ok(window_state(&window, &state)?.remove_recent(&path))
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

/// `first-paint` also quits when Lectern was started with `--exit-after-paint`. The perf log is
/// the process's, so any window may write to it.
#[tauri::command]
pub async fn perf_mark(name: String, ms: Option<f64>, state: Shared<'_>) -> Result<(), String> {
    blocking(Arc::clone(&state), move |s| s.perf_mark(&name, ms)).await
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

/// Shows the calling window at its first paint. A window restored at launch stays behind the
/// window that has the focus.
#[tauri::command]
pub async fn show_window(window: WebviewWindow, state: Shared<'_>) -> Result<(), String> {
    let restored = state.window(window.label()).and_then(|s| {
        let front = s.keeps_focus()?;
        Some((window.app_handle().get_webview_window(&front)?, s))
    });
    window.show().map_err(|e| e.to_string())?;
    if let Some((front, restored)) = restored {
        app::keep_behind(&window, front, restored);
    }
    blocking(window_state(&window, &state)?, |s| s.window_shown()).await
}

/// Every workspace, in creation order, with the calling window's marked current.
#[tauri::command]
pub fn list_workspaces(window: WebviewWindow, state: Shared<'_>) -> Vec<WorkspaceSummary> {
    state.list_workspaces(window.label())
}

/// The name a new workspace is offered.
#[tauri::command]
pub fn suggest_workspace_name(state: Shared<'_>) -> String {
    state.suggest_workspace_name()
}

/// Opens a blank window, which lists the workspaces.
#[tauri::command]
pub async fn new_window(state: Shared<'_>) -> Result<(), String> {
    blocking(Arc::clone(&state), |s| s.new_window()).await?
}

/// Opens the workspace `id` in the calling window (`here`), which then reloads, or in a new one;
/// a workspace another window shows brings that window forward instead.
#[tauri::command]
pub async fn open_workspace(
    id: String,
    r#where: OpenWhere,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<WorkspaceOutcome, String> {
    let caller = window_state(&window, &state)?;
    blocking(Arc::clone(&state), move |s| {
        leaving(&caller, &window, r#where);
        s.open_workspace(window.label(), &id, r#where)
    })
    .await?
}

/// Adds a workspace named `name` (a blank one is named for it), holding the folder `root` when
/// given, and opens it as `open_workspace` does.
#[tauri::command]
pub async fn create_workspace(
    name: String,
    r#where: OpenWhere,
    root: Option<String>,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<WorkspaceOutcome, String> {
    let caller = window_state(&window, &state)?;
    blocking(Arc::clone(&state), move |s| {
        leaving(&caller, &window, r#where);
        s.create_workspace(window.label(), &name, r#where, root.as_deref())
    })
    .await?
}

/// Before the calling window may turn to another workspace: its placement goes into the
/// workspace it shows.
fn leaving(caller: &WindowState, window: &WebviewWindow, place: OpenWhere) {
    if place == OpenWhere::Here {
        caller.remember_window(&window.as_ref().window());
    }
}

/// Renames the workspace `id`; returns the workspaces.
#[tauri::command]
pub fn rename_workspace(
    id: String,
    name: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Vec<WorkspaceSummary>, String> {
    state.rename_workspace(window.label(), &id, &name)
}

/// Forgets the workspace `id`, never its files; returns the workspaces left.
#[tauri::command]
pub async fn delete_workspace(
    id: String,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Vec<WorkspaceSummary>, String> {
    blocking(Arc::clone(&state), move |s| {
        s.delete_workspace(window.label(), &id)
    })
    .await?
}

/// Quits Lectern with every window open: each window's placement is saved, and the next launch
/// reopens them all. Closing windows one by one leaves only the last one open.
#[tauri::command]
pub fn quit(app: AppHandle, state: Shared<'_>) {
    app::remember_every_window(&app);
    state.quit();
}

/// Gives the calling window's workspace a theme of its own, or has it follow the shared theme
/// again; returns the window's settings.
#[tauri::command]
pub fn set_workspace_theme(
    own: bool,
    window: WebviewWindow,
    state: Shared<'_>,
) -> Result<Settings, String> {
    state.set_workspace_theme(window.label(), own)
}
