//! Live reload. The library roots are watched with the OS's change notifications
//! (ReadDirectoryChangesW on Windows, inotify on Linux), and the open document and its review
//! sidecar are polled as well, because notifications from a NAS are unreliable. Events are
//! debounced per document, per sidecar and per root.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::library::ignore::is_ignored;
use crate::library::{key_under, path_key};
use crate::review::sidecar_path;

/// How long dropping a watcher waits for its threads to finish.
const STOP_GRACE: Duration = Duration::from_millis(500);

/// A change the app reacts to. Paths are reported as they were passed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// The open document changed on disk, or came back after it was gone.
    DocChanged(PathBuf),
    /// The open document was deleted or moved away, or can no longer be read.
    DocRemoved(PathBuf),
    /// Something changed below this library root.
    LibraryChanged(PathBuf),
    /// The open document's review sidecar was created, changed or deleted. Carries the
    /// document's path, not the sidecar's.
    ReviewChanged(PathBuf),
}

/// Watches the library roots, the open document and its review sidecar, and reports changes
/// through a callback.
///
/// It runs two threads of its own: one polls the open document and its sidecar, the other reports
/// debounced events, so a stat stalled on a NAS never holds up library events. Dropping the
/// watcher stops both, and notify's thread with them.
pub struct DocWatcher {
    shared: Arc<Shared>,
    /// Replaced by each `watch_roots`; `None` without roots, or when the watcher couldn't start.
    notify: Mutex<Option<RecommendedWatcher>>,
    threads: Vec<JoinHandle<()>>,
    /// Disconnects once every thread has exited.
    exited: Mutex<Receiver<()>>,
}

/// What the threads and notify's handler share.
struct Shared {
    state: Mutex<State>,
    /// Signalled whenever `state` changes in a way a waiting thread may care about.
    wake: Condvar,
    on_event: Box<dyn Fn(WatchEvent) + Send + Sync>,
    debounce: Duration,
}

#[derive(Default)]
struct State {
    stop: bool,
    roots: Vec<Root>,
    doc: Option<Doc>,
    /// Bumped by every `set_current_doc`, so a stat of the previous document is thrown away.
    generation: u64,
    /// A notification touched the document or its sidecar, so the poll thread checks both
    /// straight away.
    doc_touched: bool,
    /// Events waiting out their debounce.
    pending: HashMap<Key, Pending>,
}

struct Root {
    path: PathBuf,
    key: String,
}

struct Doc {
    /// As passed in, for events.
    path: PathBuf,
    /// Absolute, for stats and the key.
    abs: PathBuf,
    key: String,
    generation: u64,
    /// The stamp last reported, or seen when the document was set. `None` while it is missing.
    seen: Option<Stamp>,
    sidecar: Sidecar,
}

/// The open document's review sidecar, at the path `review::sidecar_path` gives. It may not exist.
struct Sidecar {
    /// Absolute, for stats and the key.
    abs: PathBuf,
    key: String,
    /// The stamp last reported, or seen when the document was set. `None` while it is missing.
    seen: Option<Stamp>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    size: u64,
}

/// Events debounce per key: the open document, its sidecar, or a root by its path key.
#[derive(PartialEq, Eq, Hash)]
enum Key {
    Doc,
    Review,
    Root(String),
}

impl Key {
    /// Whether events for this key are about the open document, so they're dropped once another
    /// document is opened.
    fn is_doc(&self) -> bool {
        matches!(self, Key::Doc | Key::Review)
    }
}

struct Pending {
    event: WatchEvent,
    due: Instant,
    /// For a document or sidecar event, the generation of the document it is about.
    generation: Option<u64>,
}

impl DocWatcher {
    /// Starts the poll and debounce threads. `on_event` runs on the debounce thread, one event at
    /// a time, once `debounce` has passed with no newer event for the same document or root.
    /// Nothing is watched until `watch_roots` or `set_current_doc`.
    pub fn new(
        on_event: impl Fn(WatchEvent) + Send + Sync + 'static,
        poll_interval: Duration,
        debounce: Duration,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            wake: Condvar::new(),
            on_event: Box::new(on_event),
            debounce,
        });
        let (alive, exited) = mpsc::channel();
        let poll = {
            let shared = Arc::clone(&shared);
            spawn("lectern-watch-poll", &alive, move || {
                shared.poll(poll_interval);
            })
        };
        let debouncer = {
            let shared = Arc::clone(&shared);
            spawn("lectern-watch-debounce", &alive, move || {
                shared.report_when_due()
            })
        };
        Self {
            shared,
            notify: Mutex::new(None),
            threads: poll.into_iter().chain(debouncer).collect(),
            exited: Mutex::new(exited),
        }
    }

    /// Watches `roots` recursively, in place of the roots watched before. Errors are logged: a
    /// root that can't be watched gets no library events, and the open document is still polled.
    pub fn watch_roots(&self, roots: &[PathBuf]) {
        let mut notify = self.notify.lock().unwrap_or_else(PoisonError::into_inner);
        // The old watcher goes first, so the two never hold OS resources at the same time.
        *notify = None;
        let resolved: Vec<(&PathBuf, PathBuf)> =
            roots.iter().map(|path| (path, absolute(path))).collect();
        {
            let mut state = self.shared.lock();
            let State {
                roots: watched,
                pending,
                ..
            } = &mut *state;
            *watched = resolved
                .iter()
                .map(|(path, abs)| Root {
                    path: (*path).clone(),
                    key: path_key(abs),
                })
                .collect();
            pending.retain(|key, _| match key {
                Key::Doc | Key::Review => true,
                Key::Root(root) => watched.iter().any(|r| r.key == *root),
            });
        }
        if roots.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let handler = move |result: notify::Result<Event>| match result {
            Ok(event) => shared.on_notify(&event),
            Err(e) => log::warn!("file watcher error: {e}"),
        };
        let mut watcher = match notify::recommended_watcher(handler) {
            Ok(watcher) => watcher,
            Err(e) => {
                log::error!("can't start the file watcher; polling the open document only: {e}");
                return;
            }
        };
        // Event paths start with the path watched, so it is watched in the form keys are made from.
        for (path, abs) in &resolved {
            if let Err(e) = watcher.watch(abs, RecursiveMode::Recursive) {
                log::warn!("can't watch {}: {e}", path.display());
            }
        }
        *notify = Some(watcher);
    }

    /// Polls `doc` and its review sidecar from now on, every poll interval, in place of the
    /// document polled before; `None` stops polling. Their stamps are taken here, so only later
    /// changes are reported. Events still pending for the previous document are dropped.
    pub fn set_current_doc(&self, doc: Option<PathBuf>) {
        // Outside the lock. The caller has just read the file, so the stats are quick.
        let doc = doc.map(|path| {
            let abs = absolute(&path);
            let sidecar = sidecar_path(&abs);
            let sidecar = Sidecar {
                key: path_key(&sidecar),
                seen: stamp_of(&sidecar),
                abs: sidecar,
            };
            (stamp_of(&abs), abs, path, sidecar)
        });
        let mut state = self.shared.lock();
        state.generation += 1;
        let generation = state.generation;
        state.doc = doc.map(|(seen, abs, path, sidecar)| Doc {
            key: path_key(&abs),
            path,
            abs,
            generation,
            seen,
            sidecar,
        });
        state.doc_touched = false;
        state.pending.retain(|key, _| !key.is_doc());
        drop(state);
        self.shared.wake.notify_all();
    }
}

impl Drop for DocWatcher {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        // Stops notify's thread, which then drops its handler.
        drop(
            self.notify
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        // The threads exit as soon as they see `stop`, unless one is inside a stat of a stalled
        // share. Wait a moment for them, but never hang: a thread left behind exits when its call
        // returns, and reports nothing more.
        let exited = self
            .exited
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(RecvTimeoutError::Disconnected) = exited.recv_timeout(STOP_GRACE) {
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
        }
    }
}

/// Spawns a thread that holds a clone of `alive` until it ends, by panic or not.
fn spawn(
    name: &str,
    alive: &Sender<()>,
    run: impl FnOnce() + Send + 'static,
) -> Option<JoinHandle<()>> {
    let alive = alive.clone();
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _alive = alive;
            run();
        })
        .map_err(|e| log::error!("can't start the {name} thread: {e}"))
        .ok()
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for `wake`, at most `timeout` when there is one.
    fn wait<'a>(
        &self,
        state: MutexGuard<'a, State>,
        timeout: Option<Duration>,
    ) -> MutexGuard<'a, State> {
        match timeout {
            Some(timeout) => {
                let woken = self.wake.wait_timeout(state, timeout);
                woken.unwrap_or_else(PoisonError::into_inner).0
            }
            None => self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        }
    }

    /// Schedules `event` for once the debounce has passed, replacing what was pending for `key`. A
    /// document or sidecar event belongs to the current document.
    fn schedule(&self, state: &mut State, key: Key, event: WatchEvent) {
        let pending = Pending {
            event,
            due: Instant::now() + self.debounce,
            generation: key.is_doc().then_some(state.generation),
        };
        state.pending.insert(key, pending);
        self.wake.notify_all();
    }

    /// The poll thread. Stats the open document and its sidecar every `interval`, or at once when
    /// a notification touched either, and schedules an event for each whose stamp differs from the
    /// one last seen.
    fn poll(&self, interval: Duration) {
        let mut state = self.lock();
        let mut next = Instant::now() + interval;
        loop {
            if state.stop {
                return;
            }
            let now = Instant::now();
            let due = state
                .doc
                .as_ref()
                .filter(|_| state.doc_touched || now >= next)
                .map(|doc| {
                    let sidecar = doc.sidecar.abs.clone();
                    (doc.path.clone(), doc.abs.clone(), sidecar, doc.generation)
                });
            let Some((path, abs, sidecar, generation)) = due else {
                let timeout = state
                    .doc
                    .as_ref()
                    .map(|_| next.saturating_duration_since(now));
                state = self.wait(state, timeout);
                continue;
            };
            state.doc_touched = false;
            next = now + interval;
            drop(state);
            let stamp = stamp_of(&abs);
            let sidecar_stamp = stamp_of(&sidecar);
            state = self.lock();
            let Some(doc) = state
                .doc
                .as_mut()
                .filter(|doc| doc.generation == generation)
            else {
                continue;
            };
            let doc_event = (doc.seen != stamp).then(|| {
                doc.seen = stamp;
                match stamp {
                    Some(_) => WatchEvent::DocChanged(path.clone()),
                    None => WatchEvent::DocRemoved(path.clone()),
                }
            });
            let review_event = (doc.sidecar.seen != sidecar_stamp).then(|| {
                doc.sidecar.seen = sidecar_stamp;
                WatchEvent::ReviewChanged(path)
            });
            if let Some(event) = doc_event {
                self.schedule(&mut state, Key::Doc, event);
            }
            if let Some(event) = review_event {
                self.schedule(&mut state, Key::Review, event);
            }
        }
    }

    /// The debounce thread. Reports each pending event once it is due, earliest first.
    fn report_when_due(&self) {
        let mut state = self.lock();
        loop {
            if state.stop {
                return;
            }
            let now = Instant::now();
            let mut due: Vec<Pending> = state
                .pending
                .extract_if(|_, pending| pending.due <= now)
                .map(|(_, pending)| pending)
                .collect();
            if due.is_empty() {
                let next = state.pending.values().map(|pending| pending.due).min();
                let timeout = next.map(|due| due.saturating_duration_since(now));
                state = self.wait(state, timeout);
                continue;
            }
            drop(state);
            due.sort_by_key(|pending| pending.due);
            for pending in due {
                {
                    let state = self.lock();
                    if state.stop {
                        return;
                    }
                    // The document may have been switched while an earlier callback ran.
                    if pending.generation.is_some_and(|g| g != state.generation) {
                        continue;
                    }
                }
                (self.on_event)(pending.event);
            }
            state = self.lock();
        }
    }

    /// notify's handler. A change to the open document or its sidecar has the poll thread check
    /// them, which keeps one record of what was last reported; a change below a root schedules
    /// `LibraryChanged`.
    fn on_notify(&self, event: &Event) {
        if !changes_files(&event.kind) {
            return;
        }
        let mut state = self.lock();
        if state.stop {
            return;
        }
        // Lost events: anything may have changed.
        let rescan = event.need_rescan();
        let keys: Vec<String> = event.paths.iter().map(|path| path_key(path)).collect();
        let touched: Vec<(String, PathBuf)> = state
            .roots
            .iter()
            .filter(|root| {
                rescan
                    || keys
                        .iter()
                        .any(|key| key_under(key, &root.key).is_some_and(in_library))
            })
            .map(|root| (root.key.clone(), root.path.clone()))
            .collect();
        let doc_touched = state.doc.as_ref().is_some_and(|doc| {
            rescan || keys.contains(&doc.key) || keys.contains(&doc.sidecar.key)
        });
        if doc_touched {
            state.doc_touched = true;
            self.wake.notify_all();
        }
        for (key, path) in touched {
            self.schedule(&mut state, Key::Root(key), WatchEvent::LibraryChanged(path));
        }
    }
}

/// `path` made absolute against the current directory, with `.` and `..` resolved by name. notify
/// reports absolute paths that start with the path it watches, so roots, the document and event
/// paths all compare in this form. Nothing on disk is touched: symlinks are not followed, and a UNC
/// or NAS path stays as written.
fn absolute(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for part in abs.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(part),
        }
    }
    out
}

/// `None` when the file is missing or can't be reached.
fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        size: meta.len(),
    })
}

/// Whether an event can mean changed contents. Opening and reading can't, and inotify reports
/// opens, Lectern's own reads included.
fn changes_files(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Whether a change at `rel`, the path key below a root, can touch the library index: nothing in
/// an ignored folder (`.git`, `.obsidian`, `node_modules`) and no ignored file. The last part is
/// taken for a folder when it has no extension, since a change inside a folder can be reported
/// as a change to the folder itself.
fn in_library(rel: &str) -> bool {
    let mut parts = rel.split('/').filter(|part| !part.is_empty()).peekable();
    while let Some(part) = parts.next() {
        let is_dir = parts.peek().is_some() || Path::new(part).extension().is_none();
        if is_ignored(part, is_dir) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_changes_skip_ignored_folders_and_files() {
        for rel in [
            "",
            "readme.md",
            "work/x/plan.md",
            "notes",
            ".hidden.md",
            "v1.2/a.md",
        ] {
            assert!(in_library(rel), "{rel:?} should count");
        }
        for rel in [
            ".git/index",
            ".obsidian/workspace.json",
            ".obsidian",
            "work/node_modules/x/readme.md",
            "work/._plan.md",
            ".ds_store",
            "thumbs.db",
            "work/.plan.review.md.lectern.tmp",
        ] {
            assert!(!in_library(rel), "{rel:?} should be ignored");
        }
    }

    #[test]
    fn paths_are_made_absolute_by_name() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            absolute(Path::new("a/./b/../c.md")),
            cwd.join("a").join("c.md")
        );
        let parent = cwd.parent().unwrap().join("x");
        assert_eq!(absolute(Path::new("../x")), parent);
        // An absolute path keeps every part that names something.
        let abs = cwd.join("My Vault").join("résumé.md");
        assert_eq!(absolute(&abs), abs);
    }

    #[test]
    fn reads_are_not_changes() {
        let open = EventKind::Access(AccessKind::Open(AccessMode::Any));
        let read = EventKind::Access(AccessKind::Read);
        let closed_after_reading = EventKind::Access(AccessKind::Close(AccessMode::Read));
        let closed_after_writing = EventKind::Access(AccessKind::Close(AccessMode::Write));
        assert!(!changes_files(&open));
        assert!(!changes_files(&read));
        assert!(!changes_files(&closed_after_reading));
        assert!(changes_files(&closed_after_writing));
        assert!(changes_files(&EventKind::Any));
    }
}
