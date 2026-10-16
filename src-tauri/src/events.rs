//! Events sent to the UI, each to one window or to every window, and the `Host` through which the
//! app state reaches Tauri: emitting events, opening and focusing windows, and quitting. Tests swap
//! in a fake host.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lectern_core::agent::show::FocusStep;
use lectern_core::agent::AgentCall;
use lectern_core::ipc::{
    AgentFocus, DocChanged, FrameKey, LibraryPayload, OpenRequest, PageLoads, SettingsSnapshot,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget};

/// The open document changed on disk (`DocChanged`). Also sent when it should be re-rendered
/// silently, such as once the library index can resolve its wikilinks.
pub const DOC_CHANGED: &str = "doc-changed";
/// The open document was deleted or moved away (`DocChanged`).
pub const DOC_REMOVED: &str = "doc-removed";
/// A second launch (or a late boot render) asks the UI to open a file (`OpenRequest`).
pub const OPEN_REQUEST: &str = "open-request";
/// A library root's state or tree changed (`LibraryPayload`).
pub const LIBRARY_UPDATED: &str = "library-updated";
/// A root's index, frontmatter names included, is complete (the root's path).
pub const INDEX_READY: &str = "index-ready";
/// The open document's review sidecar was created, changed or deleted (`DocChanged`, holding the
/// document's path).
pub const REVIEW_CHANGED: &str = "review-changed";
/// The window's settings changed (`SettingsSnapshot`: the window's own, with their revision).
pub const SETTINGS_CHANGED: &str = "settings-changed";
/// The workspaces changed (no payload); sent to every window.
pub const WORKSPACES_CHANGED: &str = "workspaces-changed";
/// The window's close button was pressed while its UI holds comment text that isn't saved yet,
/// so it stayed open (no payload): the UI asks whether to let the text go.
pub const CLOSE_REQUESTED: &str = "close-requested";
/// The open HTML page loaded from the web, or was blocked from it (`PageLoads`).
pub const PAGE_LOADS: &str = "page-loads";
/// One of Lectern's shortcuts was pressed while the focus was inside a frame (`FrameKey`).
pub const FRAME_KEY: &str = "frame-key";
/// An agent asks the window's UI for something only it knows, such as what it shows
/// (`AgentCall`); the UI answers with the command `agent_reply`.
pub const AGENT_REQUEST: &str = "agent-request";

/// Which windows an event is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The window with this label.
    Window(String),
    /// Every window.
    All,
}

/// An event for the UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    DocChanged(PathBuf),
    DocRemoved(PathBuf),
    OpenRequest(OpenRequest),
    LibraryUpdated(LibraryPayload),
    IndexReady(PathBuf),
    /// Holds the document's path, not the sidecar's.
    ReviewChanged(PathBuf),
    /// The settings of the window it is sent to, with their revision.
    SettingsChanged(SettingsSnapshot),
    WorkspacesChanged,
    CloseRequested,
    PageLoads(PageLoads),
    FrameKey(FrameKey),
    AgentRequest(AgentCall),
}

/// What the app state needs from Tauri.
pub trait Host: Send + Sync {
    fn emit(&self, target: Target, event: UiEvent);
    fn exit(&self);
    /// Builds the window `label`, whose state the app already holds: hidden until its first
    /// paint, placed and coloured from that state. `focus` is whether it takes the focus.
    fn open_window(&self, label: &str, focus: bool) -> Result<(), String>;
    /// Brings the window `label` forward, unminimised, if it is showing yet.
    fn focus_window(&self, label: &str);
    /// Brings the window `label` forward for an agent as `mode` has it (`win::bring_forward`), and
    /// says how far it came, waiting at most `within` to hear; `None` when it isn't showing, or
    /// didn't say in time.
    fn agent_focus(&self, label: &str, mode: AgentFocus, within: Duration) -> Option<FocusStep>;
}

/// The real host.
pub struct TauriHost(pub AppHandle);

impl Host for TauriHost {
    /// A window's UI listens on its webview window's target, so an event for one window reaches
    /// only that window.
    fn emit(&self, target: Target, event: UiEvent) {
        let target = match target {
            Target::Window(label) => EventTarget::webview_window(label),
            Target::All => EventTarget::Any,
        };
        let app = &self.0;
        match event {
            UiEvent::DocChanged(path) => emit(app, target, DOC_CHANGED, doc(&path)),
            UiEvent::DocRemoved(path) => emit(app, target, DOC_REMOVED, doc(&path)),
            UiEvent::OpenRequest(request) => emit(app, target, OPEN_REQUEST, request),
            UiEvent::LibraryUpdated(library) => emit(app, target, LIBRARY_UPDATED, library),
            UiEvent::IndexReady(root) => emit(app, target, INDEX_READY, root.to_string_lossy()),
            UiEvent::ReviewChanged(path) => emit(app, target, REVIEW_CHANGED, doc(&path)),
            UiEvent::SettingsChanged(settings) => emit(app, target, SETTINGS_CHANGED, settings),
            UiEvent::WorkspacesChanged => emit(app, target, WORKSPACES_CHANGED, ()),
            UiEvent::CloseRequested => emit(app, target, CLOSE_REQUESTED, ()),
            UiEvent::PageLoads(loads) => emit(app, target, PAGE_LOADS, loads),
            UiEvent::FrameKey(key) => emit(app, target, FRAME_KEY, key),
            UiEvent::AgentRequest(call) => emit(app, target, AGENT_REQUEST, call),
        }
    }

    fn exit(&self) {
        self.0.exit(0);
    }

    fn open_window(&self, label: &str, focus: bool) -> Result<(), String> {
        crate::app::build_window(&self.0, label, focus)
    }

    fn focus_window(&self, label: &str) {
        crate::app::focus_window(&self.0, label);
    }

    fn agent_focus(&self, label: &str, mode: AgentFocus, within: Duration) -> Option<FocusStep> {
        crate::app::agent_focus(&self.0, label, mode, within)
    }
}

fn doc(path: &Path) -> DocChanged {
    DocChanged {
        path: path.to_string_lossy().into_owned(),
    }
}

fn emit<P: Serialize + Clone>(app: &AppHandle, target: EventTarget, event: &str, payload: P) {
    if let Err(e) = app.emit_to(target, event, payload) {
        log::warn!("couldn't send {event}: {e}");
    }
}
