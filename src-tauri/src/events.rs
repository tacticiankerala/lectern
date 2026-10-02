//! Events sent to the UI, and the `Host` through which the app state reaches Tauri: emitting
//! events and quitting. Tests swap in a fake host.

use std::path::{Path, PathBuf};

use lectern_core::ipc::{DocChanged, LibraryPayload, OpenRequest};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

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

/// An event for the UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    DocChanged(PathBuf),
    DocRemoved(PathBuf),
    OpenRequest(OpenRequest),
    LibraryUpdated(LibraryPayload),
    IndexReady(PathBuf),
}

/// What the app state needs from Tauri.
pub trait Host: Send + Sync {
    fn emit(&self, event: UiEvent);
    fn exit(&self);
}

/// The real host.
pub struct TauriHost(pub AppHandle);

impl Host for TauriHost {
    fn emit(&self, event: UiEvent) {
        match event {
            UiEvent::DocChanged(path) => emit(&self.0, DOC_CHANGED, doc(&path)),
            UiEvent::DocRemoved(path) => emit(&self.0, DOC_REMOVED, doc(&path)),
            UiEvent::OpenRequest(request) => emit(&self.0, OPEN_REQUEST, request),
            UiEvent::LibraryUpdated(library) => emit(&self.0, LIBRARY_UPDATED, library),
            UiEvent::IndexReady(root) => emit(&self.0, INDEX_READY, root.to_string_lossy()),
        }
    }

    fn exit(&self) {
        self.0.exit(0);
    }
}

fn doc(path: &Path) -> DocChanged {
    DocChanged {
        path: path.to_string_lossy().into_owned(),
    }
}

fn emit<P: Serialize + Clone>(app: &AppHandle, event: &str, payload: P) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("couldn't send {event}: {e}");
    }
}
