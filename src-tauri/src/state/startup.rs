//! Startup and launches: the payload the UI asks for first, the document rendered during boot
//! (or the launch that superseded it), and second launches forwarded once the UI is up.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Weak};
use std::thread;

use lectern_core::ipc::{OpenRequest, OpenResult, StartupPayload};

use super::doc::Early;
use super::paths::path_string;
use super::sync::{lock, write};
use super::AppState;
use crate::events::UiEvent;

impl AppState {
    /// The startup payload. The document rendered during boot becomes the initial document,
    /// unless a second launch asked for another one meanwhile.
    pub fn startup(self: &Arc<Self>) -> StartupPayload {
        self.perf.mark("webview-ready", None);
        if !self.snapshots_loaded.wait(self.timings.snapshots) {
            log::warn!("startup went ahead before the library snapshots loaded");
        }
        let seq = self.open_seq.load(Ordering::SeqCst);
        let weak = Arc::downgrade(self);
        let early = self.early.take_or_later(self.timings.early, move |early| {
            if let Some(state) = weak.upgrade() {
                state.early_landed_late(early, seq);
            }
        });
        // A launch already queued supersedes the boot document, which is then never opened.
        let superseded = self.opens.has_pending();
        let mut initial = early.and_then(|early| self.adopt_early(early, superseded));
        // Last, so a second launch during the waits above still wins. It counts as an open even
        // when it opens nothing (a folder without a README), so a boot render landing late
        // stays quiet.
        if let Some(request) = self.opens.ready() {
            self.next_seq();
            if let Some(doc) = self.resolve_target(Path::new(&request.path)) {
                initial = Some(self.open_document(&path_string(&doc)));
            }
        }
        let recent = lock(&self.state).reading.recent.clone();
        let payload = StartupPayload {
            settings: self.settings(),
            library: lock(&self.library).payload(),
            recent,
            initial,
            version: lectern_core::version().to_owned(),
            portable: false,
            startup_notice: lock(&self.notice).take(),
        };
        self.perf.mark("startup-ready", None);
        payload
    }

    pub(super) fn adopt_early(
        self: &Arc<Self>,
        early: Early,
        superseded: bool,
    ) -> Option<OpenResult> {
        if let Some(folder) = &early.folder {
            self.add_folder(folder);
        }
        let doc = early.doc.filter(|_| !superseded)?;
        if doc.from_args {
            write(&self.trust).opened_by_user(&doc.path);
        }
        match doc.outcome {
            Err(error) if !doc.from_args => {
                log::info!("didn't restore the last document: {}", error.message);
                None
            }
            outcome => Some(self.finish_open(self.next_seq(), &doc.path, outcome)),
        }
    }

    /// The boot render missed startup. The UI shows the welcome screen meanwhile, so a document
    /// that rendered is opened through an `open-request`, unless something newer was opened.
    pub(super) fn early_landed_late(self: &Arc<Self>, early: Early, seq_at_startup: u64) {
        if let Some(folder) = &early.folder {
            self.add_folder(folder);
        }
        let Some(doc) = early.doc else {
            return;
        };
        if doc.outcome.is_err() || self.open_seq.load(Ordering::SeqCst) != seq_at_startup {
            return;
        }
        if doc.from_args {
            write(&self.trust).opened_by_user(&doc.path);
        }
        log::info!("the boot render missed startup; asking the UI to open it");
        self.host.emit(UiEvent::OpenRequest(OpenRequest {
            path: path_string(&doc.path),
            t0_ms: None,
        }));
    }

    /// What a launch argument opens: a folder becomes a library root (unless it is in one or
    /// holds one) and opens its README, if it has one; anything else is opened as given. The user
    /// chose the path, so its network host is trusted. Touches the file system.
    pub(super) fn resolve_target(self: &Arc<Self>, path: &Path) -> Option<PathBuf> {
        write(&self.trust).opened_by_user(path);
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                self.add_folder(path);
                let readme = path.join("README.md");
                readme.is_file().then_some(readme)
            }
            _ => Some(path.to_path_buf()),
        }
    }

    /// Hands a second launch's request, arriving after startup, to the forwarding thread, which
    /// checks for a folder off the main thread and then asks the UI to open the file.
    pub fn forward(&self, request: OpenRequest) {
        let _ = self.forwards.send(request);
    }

    pub(super) fn forward_now(self: &Arc<Self>, request: OpenRequest) {
        // Counts as an open, so a boot render landing late never overrides it.
        self.next_seq();
        if let Some(doc) = self.resolve_target(Path::new(&request.path)) {
            self.host.emit(UiEvent::OpenRequest(OpenRequest {
                path: path_string(&doc),
                t0_ms: request.t0_ms,
            }));
        }
    }
}

/// The thread behind `AppState::forward`: requests are resolved in order, one at a time, so the
/// last launch still wins when several arrive together.
pub(super) fn spawn_forwarder(state: Weak<AppState>) -> Sender<OpenRequest> {
    let (tx, rx) = mpsc::channel::<OpenRequest>();
    let spawned = thread::Builder::new()
        .name("lectern-forward".to_owned())
        .spawn(move || {
            while let Ok(request) = rx.recv() {
                let Some(state) = state.upgrade() else {
                    break;
                };
                state.forward_now(request);
            }
        });
    if let Err(e) = spawned {
        log::error!("couldn't start the forwarding thread; second launches won't open: {e}");
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::*;
    use std::time::Duration;

    #[test]
    fn a_launch_queued_during_startup_wins_over_the_boot_doc() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let b = f.dir.file("b.md", "# B");
        let (early, opens, a2, b2) = (Arc::clone(&f.early), Arc::clone(&f.opens), a.clone(), b);
        let late = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            let queued = opens.offer(OpenRequest {
                path: path_string(&b2),
                t0_ms: None,
            });
            assert!(queued.is_none(), "the UI isn't ready yet");
            thread::sleep(Duration::from_millis(40));
            early.fill(early_doc(&a2));
            b2
        });
        let payload = f.state.startup();
        let b = late.join().unwrap();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), b);
        assert_eq!(current(&f).0, b);
        assert!(f.host.open_requests().is_empty());
    }

    #[test]
    fn a_boot_render_that_misses_startup_is_opened_when_it_lands() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let payload = f.state.startup();
        assert!(payload.initial.is_none());
        assert!(payload.startup_notice.is_none());
        f.early.fill(early_doc(&a));
        assert_eq!(f.host.open_requests(), [a]);
    }

    #[test]
    fn a_late_boot_render_gives_way_to_a_newer_open() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let b = f.dir.file("b.md", "# B");
        f.state.startup();
        f.state.open_document(&path_string(&b));
        f.early.fill(early_doc(&a));
        assert!(f.host.open_requests().is_empty());
        assert_eq!(current(&f).0, b);
    }

    #[test]
    fn a_late_boot_render_that_failed_opens_nothing() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.state.startup();
        f.early.fill(early_doc(&f.dir.0.join("gone.md")));
        assert!(f.host.open_requests().is_empty());
    }

    #[test]
    fn a_folder_argument_becomes_a_root_and_opens_its_readme() {
        let f = fixture(profile(&[]), FakeHost::default());
        let with = f.dir.folder("with");
        let readme = f.dir.file("with/README.md", "# With");
        let without = f.dir.folder("without");
        let file = f.dir.file("plain/c.md", "# C");
        assert_eq!(f.state.resolve_target(&with), Some(readme));
        assert_eq!(f.state.resolve_target(&without), None);
        assert_eq!(f.state.resolve_target(&file), Some(file));
        let roots = f.state.settings().library_roots;
        assert_eq!(roots, [path_string(&with), path_string(&without)]);
    }

    #[test]
    fn a_second_launch_with_a_folder_asks_the_ui_to_open_its_readme() {
        let f = fixture(profile(&[]), FakeHost::default());
        let folder = f.dir.folder("vault");
        let readme = f.dir.file("vault/README.md", "# Vault");
        f.state.forward(OpenRequest {
            path: path_string(&folder),
            t0_ms: Some(5.0),
        });
        wait_until("the README is requested", || {
            f.host.open_requests() == [readme.clone()]
        });
        assert_eq!(f.state.settings().library_roots, [path_string(&folder)]);
    }

    #[test]
    fn a_folder_inside_a_root_opens_its_readme_without_becoming_a_root() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let inner_readme = dir.file("vault/work/a/README.md", "# A");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let inner = inner_readme.parent().unwrap().to_owned();
        assert_eq!(f.state.resolve_target(&inner), Some(inner_readme));
        let outer = f.dir.0.clone();
        assert_eq!(f.state.resolve_target(&outer), None);
        assert_eq!(f.state.settings().library_roots, [path_string(&root)]);
    }

    #[test]
    fn a_forwarded_launch_beats_a_late_boot_render() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let b = f.dir.file("b.md", "# B");
        f.state.startup();
        f.state.forward(OpenRequest {
            path: path_string(&b),
            t0_ms: None,
        });
        wait_until("B is requested", || f.host.open_requests() == [b.clone()]);
        f.early.fill(early_doc(&a));
        assert_eq!(f.host.open_requests(), [b]);
    }

    #[test]
    fn a_queued_folder_without_a_readme_still_beats_a_late_boot_render() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let folder = f.dir.folder("plain");
        assert!(f
            .opens
            .offer(OpenRequest {
                path: path_string(&folder),
                t0_ms: None,
            })
            .is_none());
        // The boot render is still running when startup gives up waiting for it.
        let payload = f.state.startup();
        assert!(payload.initial.is_none());
        assert_eq!(f.state.settings().library_roots, [path_string(&folder)]);
        f.early.fill(early_doc(&a));
        assert!(
            f.host.open_requests().is_empty(),
            "the folder launch is newer than the boot document"
        );
    }

    #[test]
    fn a_superseded_boot_doc_is_never_opened() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let b = f.dir.file("b.md", "# B");
        assert!(f
            .opens
            .offer(OpenRequest {
                path: path_string(&b),
                t0_ms: None,
            })
            .is_none());
        f.early.fill(early_doc(&a));
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), b);
        let recent: Vec<String> = payload.recent.iter().map(|r| r.path.clone()).collect();
        assert_eq!(recent, [path_string(&b)]);
    }
}
