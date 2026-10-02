//! The file watcher's worker thread, and the `Watch` seam tests replace.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use lectern_core::watch::{DocWatcher, WatchEvent};

pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(super) const DEBOUNCE: Duration = Duration::from_millis(150);

/// Where watch requests go: the real `WatchControl`, or a fake in tests.
pub trait Watch: Send + Sync {
    fn roots(&self, roots: Vec<PathBuf>);
    fn doc(&self, doc: Option<PathBuf>);
}

/// Drives the `DocWatcher` from a thread of its own: watching a root or statting the document
/// can block on a stalled share. Only the latest roots and document matter, so a backlog is
/// collapsed.
pub struct WatchControl {
    tx: Sender<WatchCommand>,
}

pub(super) enum WatchCommand {
    Roots(Vec<PathBuf>),
    Doc(Option<PathBuf>),
}

impl WatchControl {
    /// Starts the watch worker. `on_event` runs on the watcher's thread and must never call back
    /// into the watcher.
    pub fn new(on_event: impl Fn(WatchEvent) + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<WatchCommand>();
        let spawned = thread::Builder::new()
            .name("lectern-watch-control".to_owned())
            .spawn(move || {
                let watcher = DocWatcher::new(on_event, POLL_INTERVAL, DEBOUNCE);
                while let Ok(first) = rx.recv() {
                    let (mut roots, mut doc) = (None, None);
                    for command in std::iter::once(first).chain(rx.try_iter()) {
                        match command {
                            WatchCommand::Roots(r) => roots = Some(r),
                            WatchCommand::Doc(d) => doc = Some(d),
                        }
                    }
                    if let Some(roots) = roots {
                        watcher.watch_roots(&roots);
                    }
                    if let Some(doc) = doc {
                        watcher.set_current_doc(doc);
                    }
                }
            });
        if let Err(e) = spawned {
            log::error!("couldn't start the watcher thread; live reload is off: {e}");
        }
        Self { tx }
    }
}

impl Watch for WatchControl {
    fn roots(&self, roots: Vec<PathBuf>) {
        let _ = self.tx.send(WatchCommand::Roots(roots));
    }

    fn doc(&self, doc: Option<PathBuf>) {
        let _ = self.tx.send(WatchCommand::Doc(doc));
    }
}
