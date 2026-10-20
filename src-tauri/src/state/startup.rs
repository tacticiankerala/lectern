//! Startup and launches: the payload the UI asks for first, the document rendered during boot
//! (or the launch that superseded it), second launches forwarded once the UI is up, and paths the
//! user opens in the running app, which follow the same rules as a launch.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Weak};
use std::thread;

use lectern_core::ipc::{OpenRequest, OpenResult, SettingsSnapshot, StartupPayload, UserOpen};

use super::doc::Early;
use super::open_queue::HeldOpen;
use super::paths::path_string;
use super::sync::{lock, write};
use super::WindowState;
use crate::events::UiEvent;

impl WindowState {
    /// The window's startup payload, with every workspace for its title. The document rendered as
    /// the window started (during boot, for the first window) becomes the initial document, unless
    /// a second launch, or an agent's `show`, asked for another one meanwhile: the agent's comes
    /// with its target, found again in the file as it opened, and its `show` hears whether it
    /// opened, or that its target isn't there now. An agent's never takes the place
    /// of the user's launch (the command line's file, or a second launch's): its request goes to
    /// the UI now, which answers it once it has shown the user's file. The payloads of the window
    /// that asked first are the primary ones, until the automatic update check has run
    /// (`App::claims_update_check`). The payload tells the highest generation the UI has numbered
    /// with, so a page loaded over this same state (a reload) numbers from above it, and an open
    /// still on its way from the page before never becomes current.
    pub fn startup(self: &Arc<Self>) -> StartupPayload {
        self.app.perf.mark("webview-ready", None);
        if !self.snapshots_loaded.wait(self.app.timings.snapshots) {
            log::warn!("startup went ahead before the library snapshots loaded");
        }
        let seq = self.open_seq.load(Ordering::SeqCst);
        let early = self.early.as_ref().and_then(|slot| {
            let weak = Arc::downgrade(self);
            slot.take_or_later(self.app.timings.early, move |early| {
                if let Some(state) = weak.upgrade() {
                    state.early_landed_late(early, seq);
                }
            })
        });
        // A launch already queued supersedes the boot document, which is then never opened.
        let superseded = self.opens.has_pending();
        let mut initial = early.and_then(|early| self.adopt_early(early, superseded));
        let mut agent_target = None;
        // Last, so a second launch during the waits above still wins. It counts as an open even
        // when it opens nothing (a folder without a README), so a boot render landing late
        // stays quiet. The UI's listeners are in place before it asks for this payload.
        if let Some(HeldOpen {
            request,
            by_agent,
            at,
            settle,
        }) = self
            .opens
            .ready(|call| self.emit(UiEvent::AgentRequest(call)))
        {
            self.count_open();
            let path = PathBuf::from(&request.path);
            if by_agent {
                // A file, its host checked already (`state::agent`): opened as it is, trusting
                // nothing new, then taken to the agent's target, found again in the file as it
                // opened, which may have changed since the show found it. Its `show` hears how it
                // went: shown, the open's error, or that the target isn't in the file now.
                let opened = self.open_document(&request.path);
                match at.map(|at| self.found_again(&path, at)).transpose() {
                    Ok(at) => {
                        self.opens.opened(&request.path, &opened, settle);
                        agent_target = at;
                    }
                    Err(missing) => self.opens.missed(&request.path, missing, settle),
                }
                initial = Some(opened);
            } else if self.blank_window_folder(&path) {
                // The UI's listeners are in place before it asks for this payload.
                self.emit(UiEvent::OpenRequest(OpenRequest {
                    folder: true,
                    ..request
                }));
            } else if let Some(doc) = self.resolve_target(&path) {
                initial = Some(self.open_document(&path_string(&doc)));
            }
        }
        let recent = self.recent();
        let workspace = self.workspace_summary();
        let workspaces = self.app.list_workspaces(&self.label);
        let SettingsSnapshot { settings, rev } = self.snapshot();
        let payload = StartupPayload {
            settings,
            settings_rev: rev,
            library: lock(&self.library).payload(),
            recent,
            initial,
            version: lectern_core::version().to_owned(),
            portable: self.app.portable,
            startup_notice: lock(&self.app.notice).take(),
            workspace,
            workspaces,
            primary: self.app.claims_update_check(&self.label),
            generation: self.ui_generation().max(self.cutoff.load(Ordering::SeqCst)),
            agent_target,
        };
        self.app.perf.mark("startup-ready", None);
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
            write(&self.app.trust).opened_by_user(&doc.path);
        }
        match doc.outcome {
            Err(error) if !doc.from_args => {
                log::info!("didn't restore the last document: {}", error.message);
                None
            }
            outcome => {
                self.count_open();
                Some(self.finish_open(self.ui_generation(), &doc.path, outcome))
            }
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
            write(&self.app.trust).opened_by_user(&doc.path);
        }
        log::info!("the boot render missed startup; asking the UI to open it");
        self.emit(UiEvent::OpenRequest(OpenRequest {
            path: path_string(&doc.path),
            t0_ms: None,
            folder: false,
        }));
    }

    /// What a launch argument opens: a folder becomes a library root (unless it is in one or
    /// holds one) and opens its README, if it has one; anything else is opened as given. The user
    /// chose the path, so its network host is trusted. Touches the file system.
    pub(super) fn resolve_target(self: &Arc<Self>, path: &Path) -> Option<PathBuf> {
        write(&self.app.trust).opened_by_user(path);
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                self.add_folder(path);
                let readme = path.join("README.md");
                readme.is_file().then_some(readme)
            }
            _ => Some(path.to_path_buf()),
        }
    }

    /// Opens a path the user chose in the running app at the UI's latest generation
    /// (`open_user_path_numbered`), for tests: `commands::open_user_path` carries the UI's number.
    #[cfg(test)]
    pub fn open_user_path(self: &Arc<Self>, path: &str) -> UserOpen {
        self.open_user_path_numbered(self.ui_generation(), path)
    }

    /// Opens a path the user chose in the running app (the file dialog, a drop, Add folder) as
    /// the open the UI numbered `generation` (`open_numbered`), with the same decision as a launch
    /// argument (`resolve_target`): its network host is trusted, a file opens, and a folder joins
    /// the library unless it nests with a root, opening its README when it has one. A blank
    /// window has no library: a folder there opens and joins nothing, and the answer says it was
    /// one (`folder`), for the UI to make a workspace of it. An open the UI numbered after it,
    /// while the path resolves (a share can stall), is newer, and stays current; it counts as an
    /// open, like a launch, even when it opens nothing, so a boot render landing late never
    /// overrides it. Touches the file system.
    pub fn open_user_path_numbered(self: &Arc<Self>, generation: u64, path: &str) -> UserOpen {
        self.generation.fetch_max(generation, Ordering::SeqCst);
        // Counted before the path resolves (a share can stall), and whether or not it opens
        // anything, so a boot render landing meanwhile stays quiet (`early_landed_late`).
        self.count_open();
        if self.blank_window_folder(Path::new(path)) {
            return UserOpen {
                doc: None,
                library: self.library_payload(),
                folder: true,
            };
        }
        let target = self.resolve_target(Path::new(path));
        self.finish_user_open(generation, target)
    }

    /// Opens what a user path resolved to, as the open numbered `generation`.
    pub(super) fn finish_user_open(
        self: &Arc<Self>,
        generation: u64,
        target: Option<PathBuf>,
    ) -> UserOpen {
        UserOpen {
            doc: target.map(|doc| self.open_numbered(generation, &path_string(&doc))),
            library: self.library_payload(),
            folder: false,
        }
    }

    /// Hands a second launch's request, arriving after startup, to the forwarding thread, which
    /// checks for a folder off the main thread and then asks the UI to open the file.
    pub fn forward(&self, request: OpenRequest) {
        let _ = self.forwards.send(request);
    }

    pub(super) fn forward_now(self: &Arc<Self>, request: OpenRequest) {
        // Counts as an open, so a boot render landing late never overrides it.
        self.count_open();
        let path = PathBuf::from(&request.path);
        if self.blank_window_folder(&path) {
            self.emit(UiEvent::OpenRequest(OpenRequest {
                folder: true,
                ..request
            }));
        } else if let Some(doc) = self.resolve_target(&path) {
            self.emit(UiEvent::OpenRequest(OpenRequest {
                path: path_string(&doc),
                t0_ms: request.t0_ms,
                folder: false,
            }));
        }
    }

    /// Whether a launch's or a user's `path` is a folder for a blank window. Such a window has no
    /// workspace to add the folder to, and none is made without a name, so the UI is told it is a
    /// folder (`OpenRequest::folder`, `UserOpen::folder`) to ask for one, instead of the folder
    /// becoming a root. The user chose the path, so its network host is trusted first. Touches
    /// the file system.
    fn blank_window_folder(&self, path: &Path) -> bool {
        if self.workspace_id().is_some() {
            return false;
        }
        write(&self.app.trust).opened_by_user(path);
        fs::metadata(path).is_ok_and(|meta| meta.is_dir())
    }
}

/// The thread behind `WindowState::forward`: requests are resolved in order, one at a time, so the
/// last launch still wins when several arrive together.
pub(super) fn spawn_forwarder(state: Weak<WindowState>) -> Sender<OpenRequest> {
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
    use crate::state::paths::same_path;
    use crate::state::test_support::*;
    use crate::state::trust;
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
                folder: false,
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

    /// The boot render misses startup, and the reader starts an open that stalls (a slow share):
    /// it counts from its start, so the boot render landing meanwhile stays quiet, never asking
    /// the UI to open it over the reader's own open.
    #[test]
    fn a_late_boot_render_gives_way_to_an_open_still_under_way() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("a.md", "# A");
        let b = f.dir.file("b.md", "# B");
        f.state.startup();
        let before = f.state.open_seq.load(Ordering::SeqCst);
        // The reader's open stalls: held at the render cache, as a slow share would hold it.
        let stalled = lock(&f.state.cache);
        let state = Arc::clone(&f.state);
        let path = path_string(&b);
        let opening = thread::spawn(move || state.open_numbered(1, &path));
        wait_until("the open is counted", || {
            f.state.open_seq.load(Ordering::SeqCst) > before
        });
        f.early.fill(early_doc(&a));
        assert!(f.host.open_requests().is_empty());
        drop(stalled);
        assert!(matches!(opening.join().unwrap(), OpenResult::Ok { .. }));
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
            folder: false,
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

    fn user_roots(f: &Fixture) -> Vec<String> {
        f.state.settings().library_roots
    }

    fn sidebar_roots(open: &UserOpen) -> Vec<String> {
        open.library.roots.iter().map(|r| r.path.clone()).collect()
    }

    #[test]
    fn a_file_the_user_chose_trusts_its_host_then_opens() {
        let f = fixture(profile(&[]), FakeHost::default());
        let chosen = r"\\lectern-chosen.invalid\share\plan.md";
        let refusal = trust::refusal(chosen);
        // Reached from a note, the host is refused untouched.
        assert!(matches!(
            f.state.open_document(chosen),
            OpenResult::Err { error } if error.message == refusal
        ));
        // Chosen by the user, the host is trusted first, so the open is attempted: the file isn't
        // there, but it isn't refused, and nothing joins the library.
        let opened = f.state.open_user_path(chosen);
        match opened.doc {
            Some(OpenResult::Err { error }) => assert_ne!(error.message, refusal),
            other => panic!("expected a failed open, got {other:?}"),
        }
        assert!(opened.library.roots.is_empty());
        // The host stays trusted for the session.
        assert!(f.state.trusts(r"\\LECTERN-CHOSEN.invalid\share\other.md"));
        // A local file opens as `open_document` would, without becoming a root.
        let doc = f.dir.file("notes/a.md", "# A");
        let opened = f.state.open_user_path(&path_string(&doc));
        assert_eq!(opened_path(opened.doc.as_ref().unwrap()), doc);
        assert!(user_roots(&f).is_empty());
        assert!(opened.library.roots.is_empty());
    }

    #[test]
    fn a_folder_the_user_chose_outside_every_root_becomes_a_root_and_opens_its_readme() {
        let f = fixture(profile(&[]), FakeHost::default());
        let vault = f.dir.folder("vault");
        let readme = f.dir.file("vault/README.md", "# Vault");
        let opened = f.state.open_user_path(&path_string(&vault));
        assert_eq!(opened_path(opened.doc.as_ref().unwrap()), readme);
        assert_eq!(user_roots(&f), [path_string(&vault)]);
        assert_eq!(sidebar_roots(&opened), [path_string(&vault)]);
        // Without a README, the folder still joins the library and nothing opens.
        let plain = f.dir.folder("plain");
        let opened = f.state.open_user_path(&path_string(&plain));
        assert!(opened.doc.is_none());
        // A window showing a workspace takes the folder itself.
        assert!(!opened.folder);
        assert_eq!(
            sidebar_roots(&opened),
            [path_string(&vault), path_string(&plain)]
        );
    }

    #[test]
    fn a_folder_the_user_chose_inside_a_root_opens_its_readme_without_nesting() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let readme = dir.file("vault/work/a/README.md", "# A");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let inner = readme.parent().unwrap().to_owned();
        let opened = f.state.open_user_path(&path_string(&inner));
        assert_eq!(opened_path(opened.doc.as_ref().unwrap()), readme);
        assert_eq!(user_roots(&f), [path_string(&root)]);
        assert_eq!(sidebar_roots(&opened), [path_string(&root)]);
    }

    #[test]
    fn a_folder_the_user_chose_that_is_or_holds_a_root_never_nests() {
        let dir = TempDir::new();
        let root = dir.folder("outer/vault");
        let readme = dir.file("outer/vault/README.md", "# Vault");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        // The root itself, named in another case: its README opens.
        let same = path_string(&root).to_uppercase();
        let opened = f.state.open_user_path(&same);
        assert!(same_path(
            &opened_path(opened.doc.as_ref().unwrap()),
            &readme
        ));
        // The folder holding it, which has no README: nothing opens, nothing is added.
        let outer = root.parent().unwrap().to_owned();
        let opened = f.state.open_user_path(&path_string(&outer));
        assert!(opened.doc.is_none());
        assert_eq!(user_roots(&f), [path_string(&root)]);
        assert_eq!(sidebar_roots(&opened), [path_string(&root)]);
    }

    /// The window's page reloaded over the same state (WebView2's own F5 or Ctrl+R, a renderer
    /// failure, a dev reload): the UI numbers from what Rust has seen, the payload's `generation`
    /// (the highest of its opens and its cutoff), so its first open is numbered above everything
    /// before, and an open still on its way from the page before never becomes current.
    #[test]
    fn startup_tells_the_generation_so_a_reloaded_page_numbers_above_it() {
        let f = fixture(profile(&[]), FakeHost::default());
        let plan = f.dir.file("notes/plan.md", "# Plan");
        let notes = f.dir.file("notes/b.md", "# B");
        // The page before numbered up to 50: an open, then a keep of its file.
        let _ = f.state.open_numbered(49, &path_string(&plan));
        assert!(f.state.keep_document(&plan, 50));
        let payload = f.state.startup();
        assert_eq!(payload.generation, 50);
        // The reloaded page's first open is numbered above it; one left over from before isn't.
        let _ = f.state.open_numbered(51, &path_string(&notes));
        assert_eq!(current(&f).0, notes);
        let _ = f.state.open_numbered(49, &path_string(&plan));
        assert_eq!(current(&f).0, notes);
    }

    #[test]
    fn a_user_path_that_resolves_slowly_never_replaces_a_newer_open() {
        let f = fixture(profile(&[]), FakeHost::default());
        let vault = f.dir.folder("vault");
        f.dir.file("vault/README.md", "# Vault");
        let newer = f.dir.file("notes/b.md", "# B");
        // The folder arrives, numbered 1; while it resolves (a stalled share), B opens, numbered 2.
        let target = f.state.resolve_target(&vault);
        f.state.open_numbered(2, &path_string(&newer));
        let opened = f.state.finish_user_open(1, target);
        // The README still renders for the UI, which drops the stale answer, but B stays current.
        assert!(matches!(opened.doc, Some(OpenResult::Ok { .. })));
        assert_eq!(current(&f).0, newer);
        assert_eq!(last_doc(&f), Some(path_string(&newer)));
        let last_watched = lock(&f.watched)
            .iter()
            .rev()
            .find_map(|w| match w {
                Watched::Doc(doc) => Some(doc.clone()),
                Watched::Roots(_) => None,
            })
            .flatten();
        assert_eq!(last_watched, Some(newer));
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
            folder: false,
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
                folder: false,
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
                folder: false,
            })
            .is_none());
        f.early.fill(early_doc(&a));
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), b);
        let recent: Vec<String> = payload.recent.iter().map(|r| r.path.clone()).collect();
        assert_eq!(recent, [path_string(&b)]);
    }
}
