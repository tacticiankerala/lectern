//! Fakes and fixtures for the app-state tests: a host that records events, a watch that records
//! requests, temp folders, and an `AppState` wired to them.

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

use super::doc::{now_ms, render_file, Early, EarlyDoc};
use super::open_queue::OpenQueue;
use super::paths::{path_string, same_path};
use super::profile::{Profile, StateFile};
use super::sync::{lock, Slot};
use super::watch_control::Watch;
use super::{AppState, Boot, Timings};
use crate::events::{Host, UiEvent};

#[derive(Default)]
pub(super) struct FakeHost {
    pub(super) events: Mutex<Vec<UiEvent>>,
    /// An `index-ready` for this root waits (after it is recorded) until `release` is called,
    /// which holds that root's scan open.
    pub(super) hold_index_ready: Mutex<Option<PathBuf>>,
    pub(super) released: Mutex<bool>,
    pub(super) release_cv: Condvar,
}

impl Host for FakeHost {
    fn emit(&self, event: UiEvent) {
        let held = matches!(&event, UiEvent::IndexReady(root)
            if lock(&self.hold_index_ready).as_ref() == Some(root));
        lock(&self.events).push(event);
        if held {
            let released = lock(&self.released);
            drop(
                self.release_cv
                    .wait_while(released, |released| !*released)
                    .unwrap(),
            );
        }
    }

    fn exit(&self) {}
}

impl FakeHost {
    pub(super) fn open_requests(&self) -> Vec<PathBuf> {
        lock(&self.events)
            .iter()
            .filter_map(|e| match e {
                UiEvent::OpenRequest(r) => Some(PathBuf::from(&r.path)),
                _ => None,
            })
            .collect()
    }

    pub(super) fn doc_changes(&self) -> usize {
        lock(&self.events)
            .iter()
            .filter(|e| matches!(e, UiEvent::DocChanged(_)))
            .count()
    }

    pub(super) fn indexed(&self, root: &Path) -> bool {
        lock(&self.events)
            .iter()
            .any(|e| matches!(e, UiEvent::IndexReady(r) if r == root))
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
    pub(super) state: Arc<AppState>,
    pub(super) host: Arc<FakeHost>,
    pub(super) watched: Arc<Mutex<Vec<Watched>>>,
    pub(super) early: Arc<Slot<Early>>,
    pub(super) opens: Arc<OpenQueue>,
    pub(super) config: PathBuf,
    pub(super) dir: TempDir,
}

pub(super) fn profile(roots: &[&Path]) -> Profile {
    Profile {
        settings: Settings {
            library_roots: roots.iter().map(|r| path_string(r)).collect(),
            ..Settings::default()
        },
        state: StateFile::default(),
        notice: None,
        wsl_distro: None,
        persist: true,
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
    let state = AppState::new(boot, Arc::clone(&host) as Arc<dyn Host>, move |_| {
        Box::new(FakeWatch(fake_watch))
    });
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
    lock(&f.state.library)
        .roots
        .iter()
        .any(|r| same_path(&r.path, root) && matches!(r.state, RootState::Ready) && !r.scanning)
}

pub(super) fn opened_path(result: &OpenResult) -> PathBuf {
    match result {
        OpenResult::Ok { doc } => PathBuf::from(&doc.path),
        OpenResult::Err { error } => panic!("open failed: {}", error.message),
    }
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
