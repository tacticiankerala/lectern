//! Every type that crosses IPC between the Rust side and the UI. Fields are camelCase in JSON, and
//! each type is exported to `ui/src/generated/<Type>.ts` by ts-rs when `cargo test` runs (see
//! `.cargo/config.toml`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::frontmatter::Frontmatter;
use crate::library::tree::TreeNode;
use crate::library::DocKind;
use crate::render::{OutlineItem, TaskStats};

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ThemeId {
    Paper,
    Daylight,
    Sepia,
    Latte,
    Graphite,
    Midnight,
    Nord,
    Mocha,
}

/// The reading width: a number of characters, or the full window. JSON `100` or `"full"`.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Measure {
    Full,
    // Untagged variants must follow the tagged ones.
    #[serde(untagged)]
    Chars(u16),
}

/// How "Open in editor" launches: the system's default, or a command line.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "mode", rename_all = "camelCase")]
#[ts(export)]
pub enum EditorPref {
    Auto,
    Custom { command: String },
}

/// What an HTML page may do, beyond showing itself in its sandbox: run its scripts and load from
/// the internet (`full`), run its scripts with nothing from the internet (`offline`), or run none
/// of its own scripts (`static`). Lectern's bridge script runs in every mode.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum HtmlPages {
    #[default]
    Full,
    Offline,
    Static,
}

/// A user prefix mapping for absolute paths in notes, such as `/home/me/shared` → `S:\Shared`.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PathMapping {
    pub from: String,
    pub to: String,
}

/// `settings.json`. Missing fields take their defaults and unknown fields are ignored, so files
/// from older and newer versions both load.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    pub theme_mode: ThemeMode,
    pub light_theme: ThemeId,
    pub dark_theme: ThemeId,
    pub body_font: String,
    pub code_font: String,
    /// Pixels, 12–32.
    pub font_size: u8,
    /// 1.3–2.0.
    pub line_height: f64,
    /// 60–160 characters, or full width.
    pub measure: Measure,
    pub code_wrap: bool,
    pub library_visible: bool,
    pub outline_visible: bool,
    pub library_width: u16,
    pub outline_width: u16,
    pub library_roots: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub editor: EditorPref,
    pub auto_update: bool,
    /// Badges on library folders from their README's frontmatter `status:` (on by default).
    pub show_status_badges: bool,
    /// Pixels, 11–20: the text of the library, the outline and the breadcrumb chooser.
    pub sidebar_font_size: u8,
    /// The review comments feature, switched in Preferences (on by default). Off, no sidecar is
    /// loaded or written and no comment UI shows.
    pub review_comments: bool,
    /// The header toggle (on by default). Off, all comment UI is hidden, from the highlights to the
    /// Comments tab.
    pub comments_visible: bool,
    /// What HTML pages may do (scripts and the internet by default). Every window shares it.
    pub html_pages: HtmlPages,
    /// How many recent files the header's recent files menu lists, 5–20 (10 by default).
    pub recent_in_menu: u8,
    /// The WSL distribution a link to a Linux path opens in (`\\wsl.localhost\<name>\…`); `None`
    /// for Windows' default.
    pub wsl_distro: Option<String>,
}

/// A change to some settings; absent fields are left as they are.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields = nullable)]
pub struct SettingsPatch {
    pub theme_mode: Option<ThemeMode>,
    pub light_theme: Option<ThemeId>,
    pub dark_theme: Option<ThemeId>,
    pub body_font: Option<String>,
    pub code_font: Option<String>,
    pub font_size: Option<u8>,
    pub line_height: Option<f64>,
    pub measure: Option<Measure>,
    pub code_wrap: Option<bool>,
    pub library_visible: Option<bool>,
    pub outline_visible: Option<bool>,
    pub library_width: Option<u16>,
    pub outline_width: Option<u16>,
    pub library_roots: Option<Vec<String>>,
    pub path_mappings: Option<Vec<PathMapping>>,
    pub editor: Option<EditorPref>,
    pub auto_update: Option<bool>,
    pub show_status_badges: Option<bool>,
    pub sidebar_font_size: Option<u8>,
    pub review_comments: Option<bool>,
    pub comments_visible: Option<bool>,
    pub html_pages: Option<HtmlPages>,
    pub recent_in_menu: Option<u8>,
    /// A distribution's name, or `""` for Windows' default: `null` here leaves it as it is.
    pub wsl_distro: Option<String>,
}

/// The WSL distributions Preferences offers for the *WSL distribution* setting.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WslDistros {
    /// Windows' default distribution, which the setting's `None` stands for; `None` when WSL isn't
    /// installed.
    pub default: Option<String>,
    /// Every installed distribution's name; empty when WSL isn't installed.
    pub installed: Vec<String>,
}

/// A window's settings as a command answers or `settings-changed` carries them, with the revision
/// they were taken at. The revision rises with every change to any window's settings, so the UI
/// drops a snapshot older than one it has applied, whichever way it arrived. It is no part of
/// `Settings`, which `settings.json` holds.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsSnapshot {
    pub settings: Settings,
    pub rev: u64,
}

/// Where the reader was in a document: the nearest heading and the pixel offset below it, falling
/// back to the top block's source line, then to the scroll fraction.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SavedPosition {
    pub heading_id: Option<String>,
    pub offset: f64,
    pub line: Option<u32>,
    pub fraction: f64,
}

/// One breadcrumb segment: a root, a folder or the file.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Crumb {
    pub name: String,
    pub path: String,
    /// The folder's `README.md`, opened when the segment is clicked.
    pub readme: Option<String>,
}

/// A rendered document, ready to show. An HTML page or a PDF shows in a frame: its `html` and
/// `outline` are empty, with no frontmatter or tasks.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocPayload {
    pub path: String,
    pub kind: DocKind,
    /// Where the frame loads an HTML page or a PDF from: a URL of this open's own
    /// (`http://lxpage.localhost/o/<token>/<file name>`), which reaches the file's folder and
    /// below. `None` for every other kind.
    pub frame_url: Option<String>,
    /// The key an HTML page's bridge signs its messages with: the UI takes none without it.
    /// `None` for every other kind.
    pub bridge_key: Option<String>,
    pub title: String,
    pub html: String,
    pub outline: Vec<OutlineItem>,
    pub frontmatter: Option<Frontmatter>,
    pub tasks: TaskStats,
    pub word_count: u32,
    /// Last modified, in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// The file was not valid UTF-8 and was decoded with replacement characters.
    pub lossy: bool,
    pub position: Option<SavedPosition>,
    pub breadcrumbs: Vec<Crumb>,
    /// The library root holding the document, if any.
    pub root_path: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OpenErrorKind {
    NotFound,
    Permission,
    Binary,
    /// A text file too large to show.
    TooLarge,
    Io,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenError {
    pub kind: OpenErrorKind,
    pub message: String,
    pub path: String,
}

// One result per open, moved straight into the response, so boxing the document buys nothing.
#[expect(clippy::large_enum_variant)]
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(export)]
pub enum OpenResult {
    Ok { doc: DocPayload },
    Err { error: OpenError },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum RootState {
    Scanning,
    Ready,
    Unavailable { reason: String },
}

/// One library root in the sidebar. `tree` is absent until a scan or snapshot provides one.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RootView {
    pub path: String,
    pub name: String,
    pub state: RootState,
    pub tree: Option<TreeNode>,
    /// The last scan stopped at the file cap, so some files are missing from the tree, quick
    /// open, search and link resolution.
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LibraryPayload {
    pub roots: Vec<RootView>,
}

/// What a path the user chose (the file dialog, a drop, Add folder) opened: a file opens; a folder
/// joins the library unless it nests with a root, and opens its README when it has one. The
/// library comes back as it now is.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserOpen {
    pub doc: Option<OpenResult>,
    pub library: LibraryPayload,
    /// The path is a folder chosen in a blank window, which has no library to add it to: nothing
    /// opened or joined. The UI asks for a new workspace's name, then creates it with the folder.
    pub folder: bool,
}

/// A file offered by quick open.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Candidate {
    pub path: String,
    pub name: String,
    /// The path relative to `root`, `/`-separated.
    pub rel: String,
    pub root: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentEntry {
    pub path: String,
    pub title: String,
    /// When it was last opened, in milliseconds since the Unix epoch.
    pub opened_ms: i64,
}

/// Everything the UI needs for its first paint.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StartupPayload {
    pub settings: Settings,
    /// The revision `settings` were taken at (`SettingsSnapshot`).
    pub settings_rev: u64,
    pub library: LibraryPayload,
    pub recent: Vec<RecentEntry>,
    /// The document given on the command line, or the last one open, rendered during startup.
    pub initial: Option<OpenResult>,
    pub version: String,
    pub portable: bool,
    /// A one-time message for the user, such as settings having been reset.
    pub startup_notice: Option<String>,
    /// The workspace this window shows; `None` for a blank window.
    pub workspace: Option<WorkspaceSummary>,
    /// Every workspace, in creation order, with this window's marked current: the UI words the
    /// title from it before the first paint, with no call of its own.
    pub workspaces: Vec<WorkspaceSummary>,
    /// Whether this page runs the process's automatic update check: true in the window that first
    /// asked for its startup payload, each time it starts again (it turned to another workspace),
    /// until a check has run.
    pub primary: bool,
}

/// A workspace as the header's workspace chip and a blank window's list show it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    /// Shown in some window, this one or another.
    pub open: bool,
    /// Shown in the window that asked.
    pub current: bool,
    pub roots: Vec<String>,
    /// It has a theme of its own; otherwise it shows the shared one ("Same as other windows").
    pub own_theme: bool,
}

/// Where a workspace the user chose opens: in the window they chose it from, or a new one.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OpenWhere {
    Here,
    NewWindow,
}

/// What opening a workspace did: this window reloads to show it, the window already showing it
/// was focused, or it opened in a new window.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WorkspaceOutcome {
    Reload,
    Focused,
    Opened,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FollowKind {
    Doc,
    File,
    Path,
    External,
    Broken,
}

/// A link or inline-code path the user clicked.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FollowTarget {
    pub kind: FollowKind,
    pub target: String,
    pub line: Option<u32>,
    pub anchor: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "action", rename_all = "camelCase")]
#[ts(export)]
pub enum FollowResult {
    /// Show this document in the reader.
    OpenDoc {
        path: String,
        anchor: Option<String>,
        line: Option<u32>,
    },
    /// Handed to the shell or an editor; nothing changes in the reader.
    Opened,
    NotFound {
        message: String,
    },
}

/// A path in the two forms a terminal takes: `C:\…` or `\\server\…` for Windows, `/…` for WSL.
/// `None` where the path has no such form, such as a share no mapping reaches from WSL.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PathForms {
    pub windows: Option<String>,
    pub wsl: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    pub portable: bool,
}

/// Event payload: the open document changed on disk.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocChanged {
    pub path: String,
}

/// Event payload: what an open HTML page has loaded from the web, and what it tried to load and was
/// blocked from, as the browser's policy reports tell it. `token` names the open it is for (the
/// page's frame URL carries it); each list of hosts grows in the order they were first reported, up
/// to its cap.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageLoads {
    pub token: String,
    /// The first hosts the page loaded from, at most 50.
    pub hosts: Vec<String>,
    /// How many hosts it loaded from in all: more than `hosts` names once it is full.
    pub count: u32,
    /// The first hosts a load from was blocked, at most 50.
    pub blocked_hosts: Vec<String>,
    /// How many hosts a load from was blocked in all.
    pub blocked_count: u32,
    /// The page was served without the web (`offline` or `static`): Preferences can let it load.
    pub offline: bool,
}

/// Where the keyboard focus is, as the UI tells Rust: in Lectern's own page, or inside the frame
/// of an HTML page or a PDF, whose keys only Rust sees.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum FrameFocus {
    #[default]
    None,
    Page,
    Pdf,
}

/// Event payload: one of Lectern's own shortcuts, pressed while the focus was inside a frame, as
/// a `keydown` would describe it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FrameKey {
    /// `KeyboardEvent.key`: `"o"`, `"O"` with Shift, `"F5"`, `"Escape"`, `"+"`.
    pub key: String,
    /// `KeyboardEvent.code`: `"KeyO"`, `"F5"`, `"Equal"`, `"NumpadAdd"`.
    pub code: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Event payload: a second launch (or a drop) asked to open `path`.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenRequest {
    pub path: String,
    /// When the request started, for perf marks.
    pub t0_ms: Option<f64>,
    /// `path` is a folder launched into a blank window, which has no workspace to add it to: the
    /// UI asks for a new workspace's name, then creates it with the folder.
    pub folder: bool,
}
