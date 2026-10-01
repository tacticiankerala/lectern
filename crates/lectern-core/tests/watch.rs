//! The watcher runs real threads against real files. Every wait is a `recv_timeout` on the
//! callback's channel with a generous limit, so a slow machine only makes a test slower.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::watch::{DocWatcher, WatchEvent};

const POLL: Duration = Duration::from_millis(100);
/// Long enough that only a notification, never the poll, can report a change within `WAIT`.
const NO_POLL: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_millis(150);
/// The most any test waits for an event it expects.
const WAIT: Duration = Duration::from_secs(3);
/// How long a test listens to show that no further event arrives: more than a poll and a
/// debounce together.
const QUIET: Duration = Duration::from_millis(600);

fn watcher(poll: Duration) -> (DocWatcher, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let watcher = DocWatcher::new(
        move |event| {
            let _ = tx.send(event);
        },
        poll,
        DEBOUNCE,
    );
    (watcher, rx)
}

/// A temporary folder holding `doc.md`.
fn doc_in_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("doc.md");
    fs::write(&doc, "v1\n").unwrap();
    (tmp, doc)
}

/// The next event that `keep` accepts, skipping others; panics after `WAIT`.
fn next_matching(rx: &Receiver<WatchEvent>, keep: impl Fn(&WatchEvent) -> bool) -> WatchEvent {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) if keep(&event) => return event,
            Ok(_) => {}
            Err(e) => panic!("no matching event within {WAIT:?}: {e:?}"),
        }
    }
}

fn is_doc_event(event: &WatchEvent) -> bool {
    matches!(event, WatchEvent::DocChanged(_) | WatchEvent::DocRemoved(_))
}

fn assert_quiet(rx: &Receiver<WatchEvent>, keep: impl Fn(&WatchEvent) -> bool) {
    let deadline = Instant::now() + QUIET;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) if keep(&event) => panic!("unexpected {event:?}"),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => panic!("the watcher stopped"),
        }
    }
}

#[test]
fn poll_detects_change() {
    let (_tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(POLL);
    watcher.set_current_doc(Some(doc.clone()));
    fs::write(&doc, "version 2\n").unwrap();
    assert_eq!(next_matching(&rx, |_| true), WatchEvent::DocChanged(doc));
}

#[test]
fn poll_detects_removal() {
    let (_tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(POLL);
    watcher.set_current_doc(Some(doc.clone()));
    fs::remove_file(&doc).unwrap();
    assert_eq!(
        next_matching(&rx, |_| true),
        WatchEvent::DocRemoved(doc.clone())
    );

    // Putting it back is a change.
    fs::write(&doc, "restored\n").unwrap();
    assert_eq!(next_matching(&rx, |_| true), WatchEvent::DocChanged(doc));
}

#[test]
fn debounce_collapses_bursts() {
    let (tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(POLL);
    // Notifications report every write as it happens; without the debounce each would be an event.
    watcher.watch_roots(&[tmp.path().to_path_buf()]);
    watcher.set_current_doc(Some(doc.clone()));
    // Five writes within 50 ms, each a different size so every one is a visible change.
    for i in 1..=5 {
        fs::write(&doc, "x".repeat(i * 10)).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        next_matching(&rx, is_doc_event),
        WatchEvent::DocChanged(doc)
    );
    assert_quiet(&rx, is_doc_event);
}

#[test]
fn switching_docs_stops_watching_the_old_one() {
    let (tmp, old) = doc_in_tmp();
    let new = tmp.path().join("new.md");
    fs::write(&new, "new\n").unwrap();
    let (watcher, rx) = watcher(POLL);
    watcher.set_current_doc(Some(old.clone()));
    watcher.set_current_doc(Some(new.clone()));
    fs::write(&old, "old, changed\n").unwrap();
    fs::write(&new, "new, changed\n").unwrap();
    assert_eq!(next_matching(&rx, |_| true), WatchEvent::DocChanged(new));
    assert_quiet(&rx, |_| true);

    watcher.set_current_doc(None);
    fs::remove_file(&old).unwrap();
    assert_quiet(&rx, |_| true);
}

#[test]
fn notify_reports_library_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (watcher, rx) = watcher(NO_POLL);
    watcher.watch_roots(&[root.to_path_buf()]);
    fs::create_dir(root.join("work")).unwrap();
    fs::write(root.join("work").join("new.md"), "new\n").unwrap();
    assert_eq!(
        next_matching(&rx, |_| true),
        WatchEvent::LibraryChanged(root.to_path_buf())
    );
    assert_quiet(&rx, |_| true);
}

#[test]
fn notify_reports_doc_changes_before_the_poll() {
    let (tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(NO_POLL);
    watcher.watch_roots(&[tmp.path().to_path_buf()]);
    watcher.set_current_doc(Some(doc.clone()));

    fs::write(&doc, "changed by an editor\n").unwrap();
    assert_eq!(
        next_matching(&rx, is_doc_event),
        WatchEvent::DocChanged(doc.clone())
    );

    // An atomic save: write a temporary file, then rename it over the document.
    let tmp_file = tmp.path().join("doc.md.tmp");
    fs::write(&tmp_file, "saved atomically, longer\n").unwrap();
    fs::rename(&tmp_file, &doc).unwrap();
    assert_eq!(
        next_matching(&rx, is_doc_event),
        WatchEvent::DocChanged(doc.clone())
    );

    // Moved away, as when `work/x` becomes `archive/x`.
    fs::rename(&doc, tmp.path().join("moved.md")).unwrap();
    assert_eq!(
        next_matching(&rx, is_doc_event),
        WatchEvent::DocRemoved(doc)
    );
}

#[test]
fn watch_roots_replaces_the_set() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (watcher, rx) = watcher(NO_POLL);
    watcher.watch_roots(&[a.path().to_path_buf()]);
    watcher.watch_roots(&[b.path().to_path_buf()]);
    fs::write(a.path().join("a.md"), "a\n").unwrap();
    fs::write(b.path().join("b.md"), "b\n").unwrap();
    assert_eq!(
        next_matching(&rx, |_| true),
        WatchEvent::LibraryChanged(b.path().to_path_buf())
    );
    assert_quiet(&rx, |_| true);
}

#[test]
fn a_root_that_cannot_be_watched_leaves_polling_working() {
    let (tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(POLL);
    watcher.watch_roots(&[tmp.path().join("missing")]);
    watcher.set_current_doc(Some(doc.clone()));
    fs::write(&doc, "still polled\n").unwrap();
    assert_eq!(next_matching(&rx, |_| true), WatchEvent::DocChanged(doc));
}

#[test]
fn dropping_stops_the_threads() {
    let (tmp, doc) = doc_in_tmp();
    let (watcher, rx) = watcher(NO_POLL);
    watcher.watch_roots(&[tmp.path().to_path_buf()]);
    watcher.set_current_doc(Some(doc));

    let started = Instant::now();
    drop(watcher);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "drop took {:?}",
        started.elapsed()
    );
    // The callback, and with it the sender, is dropped once every thread holding it has exited.
    assert_disconnects(&rx);
}

fn assert_disconnects(rx: &Receiver<WatchEvent>) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(_) => {}
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => panic!("a watcher thread outlived the watcher"),
        }
    }
}

#[test]
fn the_watcher_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DocWatcher>();
}

/// `path` relative to the current directory, climbing out with `..` as far as needed.
fn relative_to_cwd(path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let common = cwd.ancestors().find(|a| path.starts_with(a)).unwrap();
    let mut rel: PathBuf = cwd
        .strip_prefix(common)
        .unwrap()
        .components()
        .map(|_| "..")
        .collect();
    rel.push(path.strip_prefix(common).unwrap());
    rel
}

#[test]
fn relative_paths_match_notifications() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let doc = tmp.path().join("doc.md");
    fs::write(&doc, "v1\n").unwrap();
    let root = relative_to_cwd(tmp.path());
    assert!(root.is_relative(), "{}", root.display());
    let rel_doc = root.join("doc.md");

    let (watcher, rx) = watcher(NO_POLL);
    watcher.watch_roots(std::slice::from_ref(&root));
    watcher.set_current_doc(Some(rel_doc.clone()));
    fs::write(&doc, "changed through a relative path\n").unwrap();

    // Both events come from notifications, and carry the paths as they were passed in.
    let expected = [
        WatchEvent::DocChanged(rel_doc),
        WatchEvent::LibraryChanged(root),
    ];
    let mut seen = Vec::new();
    while !expected.iter().all(|e| seen.contains(e)) {
        seen.push(next_matching(&rx, |_| true));
    }
}

#[test]
fn a_doc_switched_away_from_mid_batch_gets_no_event() {
    let lib = tempfile::tempdir().unwrap();
    let lib_root = lib.path().to_path_buf();
    let (docs, doc) = doc_in_tmp();
    let other = docs.path().join("other.md");
    fs::write(&other, "other\n").unwrap();

    // Each `LibraryChanged` for `lib` holds the debounce thread until the test releases it.
    let (tx, rx) = mpsc::channel();
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let held = WatchEvent::LibraryChanged(lib_root.clone());
    let watcher = DocWatcher::new(
        move |event| {
            let hold = event == held;
            let _ = tx.send(event);
            if hold {
                let _ = gate.lock().unwrap().recv();
            }
        },
        NO_POLL,
        DEBOUNCE,
    );
    watcher.watch_roots(&[lib_root.clone(), docs.path().to_path_buf()]);
    watcher.set_current_doc(Some(doc.clone()));

    fs::write(lib.path().join("a.md"), "a\n").unwrap();
    assert_eq!(
        next_matching(&rx, |_| true),
        WatchEvent::LibraryChanged(lib_root.clone())
    );
    // While that callback runs, another library change and then a document change come in.
    fs::write(lib.path().join("b.md"), "b\n").unwrap();
    fs::write(&doc, "changed while the thread was held\n").unwrap();
    // Let both fall due, so they are taken in one batch, library event first, once released.
    thread::sleep(DEBOUNCE * 3);
    release.send(()).unwrap();
    assert_eq!(
        next_matching(&rx, |_| true),
        WatchEvent::LibraryChanged(lib_root)
    );
    // The document is switched while the batch's library callback runs.
    watcher.set_current_doc(Some(other));
    release.send(()).unwrap();
    assert_quiet(&rx, is_doc_event);
}
