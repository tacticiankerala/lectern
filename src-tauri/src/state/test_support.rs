//! Fakes and fixtures for the app-state tests: a host that records events, a watch that records
//! requests, temp folders, and an `App` wired to them, with its first window's `WindowState`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::ipc::{OpenResult, RootState, Settings};
use lectern_core::library::pathmap::PathMapper;
use lectern_core::perf::PerfLog;
use lectern_core::render::highlight::StartupWarmUp;
use lectern_core::workspace::{Workspaces, WORKSPACES_FILE};

use super::doc::{now_ms, render_file, Early, EarlyDoc};
use super::open_queue::OpenQueue;
use super::paths::{path_string, same_path};
use super::profile::{Profile, StateFile};
use super::sync::{lock, write, Slot};
use super::watch_control::Watch;
use super::{App, Boot, Timings, WindowState};
use crate::app::MAIN_WINDOW;
use crate::events::{Host, Target, UiEvent};

#[derive(Default)]
pub(super) struct FakeHost {
    /// Every event sent, with the windows it was for, in order.
    pub(super) events: Mutex<Vec<(Target, UiEvent)>>,
    /// An `index-ready` for this root waits (after it is recorded) until `release` is called,
    /// which holds that root's scan open.
    pub(super) hold_index_ready: Mutex<Option<PathBuf>>,
    pub(super) released: Mutex<bool>,
    pub(super) release_cv: Condvar,
    /// The windows built, in order, and whether each took the focus.
    pub(super) opened: Mutex<Vec<(String, bool)>>,
    /// The windows brought forward, in order.
    pub(super) focused: Mutex<Vec<String>>,
    /// The app asked to exit.
    pub(super) exited: Mutex<bool>,
}

impl Host for FakeHost {
    fn emit(&self, target: Target, event: UiEvent) {
        let held = matches!(&event, UiEvent::IndexReady(root)
            if lock(&self.hold_index_ready).as_ref() == Some(root));
        lock(&self.events).push((target, event));
        if held {
            let released = lock(&self.released);
            drop(
                self.release_cv
                    .wait_while(released, |released| !*released)
                    .unwrap(),
            );
        }
    }

    fn exit(&self) {
        *lock(&self.exited) = true;
    }

    fn open_window(&self, label: &str, focus: bool) -> Result<(), String> {
        lock(&self.opened).push((label.to_owned(), focus));
        Ok(())
    }

    fn focus_window(&self, label: &str) {
        lock(&self.focused).push(label.to_owned());
    }
}

impl FakeHost {
    pub(super) fn open_requests(&self) -> Vec<PathBuf> {
        lock(&self.events)
            .iter()
            .filter_map(|(_, e)| match e {
                UiEvent::OpenRequest(r) => Some(PathBuf::from(&r.path)),
                _ => None,
            })
            .collect()
    }

    pub(super) fn doc_changes(&self) -> usize {
        lock(&self.events)
            .iter()
            .filter(|(_, e)| matches!(e, UiEvent::DocChanged(_)))
            .count()
    }

    /// The documents of the `review-changed` events sent, in order.
    pub(super) fn review_changes(&self) -> Vec<PathBuf> {
        lock(&self.events)
            .iter()
            .filter_map(|(_, e)| match e {
                UiEvent::ReviewChanged(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    pub(super) fn indexed(&self, root: &Path) -> bool {
        lock(&self.events)
            .iter()
            .any(|(_, e)| matches!(e, UiEvent::IndexReady(r) if r == root))
    }

    /// Where each event that `is` picks out was sent, in order.
    pub(super) fn targets(&self, is: impl Fn(&UiEvent) -> bool) -> Vec<Target> {
        lock(&self.events)
            .iter()
            .filter(|(_, e)| is(e))
            .map(|(target, _)| target.clone())
            .collect()
    }

    /// The `settings-changed` events sent, in order, with the windows they were for.
    pub(super) fn settings_changes(&self) -> Vec<(Target, Settings)> {
        lock(&self.events)
            .iter()
            .filter_map(|(target, e)| match e {
                UiEvent::SettingsChanged(snapshot) => {
                    Some((target.clone(), snapshot.settings.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The revision each `settings-changed` sent carried, in order.
    pub(super) fn settings_revs(&self) -> Vec<u64> {
        lock(&self.events)
            .iter()
            .filter_map(|(_, e)| match e {
                UiEvent::SettingsChanged(snapshot) => Some(snapshot.rev),
                _ => None,
            })
            .collect()
    }

    /// Where each `workspaces-changed` was sent, in order.
    pub(super) fn workspace_changes(&self) -> Vec<Target> {
        self.targets(|e| matches!(e, UiEvent::WorkspacesChanged))
    }

    /// The windows built, in order.
    pub(super) fn opened_windows(&self) -> Vec<String> {
        lock(&self.opened)
            .iter()
            .map(|(label, _)| label.clone())
            .collect()
    }

    /// Lets a held `index-ready` go on.
    pub(super) fn release(&self) {
        *lock(&self.released) = true;
        self.release_cv.notify_all();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Watched {
    Roots(Vec<PathBuf>),
    Doc(Option<PathBuf>),
}

pub(super) struct FakeWatch(pub(super) Arc<Mutex<Vec<Watched>>>);

impl Watch for FakeWatch {
    fn roots(&self, roots: Vec<PathBuf>) {
        lock(&self.0).push(Watched::Roots(roots));
    }

    fn doc(&self, doc: Option<PathBuf>) {
        lock(&self.0).push(Watched::Doc(doc));
    }
}

/// A folder under the system temp folder, removed when dropped.
pub(super) struct TempDir(pub(super) PathBuf);

impl TempDir {
    pub(super) fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lectern-state-test-{}-{n}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(super) fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    pub(super) fn folder(&self, rel: &str) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Fixture {
    /// The first window's state.
    pub(super) state: Arc<WindowState>,
    pub(super) app: Arc<App>,
    pub(super) host: Arc<FakeHost>,
    pub(super) watched: Arc<Mutex<Vec<Watched>>>,
    pub(super) early: Arc<Slot<Early>>,
    pub(super) opens: Arc<OpenQueue>,
    pub(super) config: PathBuf,
    /// The last field, so the folder goes only after the state.
    pub(super) dir: TempDir,
}

impl Drop for Fixture {
    /// Scans and the save thread write into the temp folder from threads of their own, and a
    /// write after it is removed makes it again. So, a failed test included, this lets a held
    /// scan go on, lets go of every window's state (they hold the app), waits until no other
    /// thread holds one, and writes what is waiting to be saved. Dropping the fixture then drops
    /// the app and ends the save thread with nothing left to write.
    fn drop(&mut self) {
        self.host.release();
        let mut states: Vec<Arc<WindowState>> =
            write(&self.app.windows).drain().map(|(_, s)| s).collect();
        if !states.iter().any(|s| Arc::ptr_eq(s, &self.state)) {
            states.push(Arc::clone(&self.state));
        }
        // Held here only by `states`, and by the fixture for its first window.
        let ours = |s: &Arc<WindowState>| 1 + usize::from(Arc::ptr_eq(s, &self.state));
        let started = Instant::now();
        while states.iter().any(|s| Arc::strong_count(s) > ours(s))
            && started.elapsed() < Duration::from_secs(10)
        {
            thread::sleep(Duration::from_millis(5));
        }
        self.app.saver.flush(Duration::from_secs(10));
    }
}

/// A profile whose one workspace, "Main" (`w1`), has the libraries `roots` and is open, as an
/// older Lectern's profile becomes; its `workspaces.json` is taken as saved already.
pub(super) fn profile(roots: &[&Path]) -> Profile {
    let settings = Settings {
        library_roots: roots.iter().map(|r| path_string(r)).collect(),
        ..Settings::default()
    };
    Profile {
        workspaces: Workspaces::migrate(&settings, None, Vec::new(), None),
        migrated: false,
        settings,
        state: StateFile::default(),
        notice: None,
        wsl_distro: None,
        persist: true,
        launch: None,
    }
}

pub(super) fn fixture_in(dir: TempDir, profile: Profile, host: FakeHost) -> Fixture {
    fixture_full(dir, profile, host, Arc::new(StartupWarmUp::new(|| {})))
}

/// A fixture whose start-up warm-up is `warm`.
pub(super) fn fixture_with_warm(
    profile: Profile,
    host: FakeHost,
    warm: Arc<StartupWarmUp>,
) -> Fixture {
    fixture_full(TempDir::new(), profile, host, warm)
}

fn fixture_full(
    dir: TempDir,
    profile: Profile,
    host: FakeHost,
    warm: Arc<StartupWarmUp>,
) -> Fixture {
    let host = Arc::new(host);
    let watched = Arc::new(Mutex::new(Vec::new()));
    let early = Arc::new(Slot::default());
    let opens = Arc::new(OpenQueue::default());
    let config = dir.folder("config");
    let boot = Boot {
        config_dir: config.clone(),
        snapshot_dir: dir.0.join("snapshots"),
        perf: Arc::new(PerfLog::new(None, 0.0)),
        exit_after_paint: false,
        profile,
        early: Arc::clone(&early),
        warm,
        opens: Arc::clone(&opens),
        timings: Timings {
            probe: Duration::from_secs(2),
            exists: Duration::from_secs(1),
            early: Duration::from_millis(300),
            snapshots: Duration::from_millis(300),
            root: Duration::from_secs(5),
            scan_delay: Duration::ZERO,
            // Never in a test: releasing would race the other tests' renders.
            release_after: Duration::from_secs(24 * 60 * 60),
        },
        portable: false,
    };
    let fake_watch = Arc::clone(&watched);
    let app = App::new(boot, Arc::clone(&host) as Arc<dyn Host>, move |_| {
        Box::new(FakeWatch(Arc::clone(&fake_watch)))
    });
    let state = app.window(MAIN_WINDOW).expect("the first window's state");
    state.start_library();
    // The boot thread takes the roots once and then watches them: past that, nothing it does
    // races what a test does to the roots.
    wait_until("the boot thread watches the roots", || {
        lock(&watched)
            .iter()
            .any(|w| matches!(w, Watched::Roots(_)))
    });
    Fixture {
        state,
        app,
        host,
        watched,
        early,
        opens,
        config,
        dir,
    }
}

pub(super) fn fixture(profile: Profile, host: FakeHost) -> Fixture {
    fixture_in(TempDir::new(), profile, host)
}

pub(super) fn wait_until(what: &str, f: impl Fn() -> bool) {
    let started = Instant::now();
    while !f() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn indexed(f: &Fixture, path: &Path) -> bool {
    f.state.index().root_for(path).is_some()
}

pub(super) fn settled(f: &Fixture, root: &Path) -> bool {
    settled_in(&f.state, root)
}

/// Whether `window` has indexed `root`, with no scan of it running.
pub(super) fn settled_in(window: &WindowState, root: &Path) -> bool {
    lock(&window.library)
        .roots
        .iter()
        .any(|r| same_path(&r.path, root) && matches!(r.state, RootState::Ready) && !r.scanning)
}

/// What `workspaces.json` holds once everything waiting to be saved is written.
pub(super) fn saved_workspaces(f: &Fixture) -> Workspaces {
    f.app.flush();
    let text = fs::read_to_string(f.config.join(WORKSPACES_FILE)).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub(super) fn opened_path(result: &OpenResult) -> PathBuf {
    match result {
        OpenResult::Ok { doc } => PathBuf::from(&doc.path),
        OpenResult::Err { error } => panic!("open failed: {}", error.message),
    }
}

/// The last document of the workspace the fixture's window shows.
pub(super) fn last_doc(f: &Fixture) -> Option<String> {
    let id = f.state.workspace_id()?;
    f.app
        .read_workspace(&id, |ws| ws.last_doc.clone())
        .flatten()
}

pub(super) fn current(f: &Fixture) -> (PathBuf, bool) {
    let current = lock(&f.state.current);
    let c = current.as_ref().expect("a current document");
    (c.path.clone(), c.stale)
}

pub(super) fn early_doc(path: &Path) -> Early {
    Early {
        doc: Some(EarlyDoc {
            path: path.to_path_buf(),
            from_args: true,
            outcome: render_file(path, &PathMapper::default(), &[]),
        }),
        folder: None,
    }
}
