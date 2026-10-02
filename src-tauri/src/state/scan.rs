//! Probing, walking and indexing a library root on a thread of its own, and installing the
//! result.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use lectern_core::ipc::RootState;
use lectern_core::library::scan::{probe_root, read_heads, scan_root, ScanOptions};
use lectern_core::library::snapshot::save_snapshot;
use lectern_core::library::tree::build_tree;
use lectern_core::library::{path_key, RootIndex};

use super::sync::{lock, write};
use super::AppState;
use crate::events::UiEvent;

impl AppState {
    /// Scans `root` on a thread of its own once the first document is on screen; when a scan is
    /// already running, it runs once more after. `done` hears when the root next has a tree or
    /// turns out unavailable, whichever scan gets there.
    pub fn request_scan(self: &Arc<Self>, root: &Path, done: Option<Sender<()>>) {
        let gen = {
            let mut lib = lock(&self.library);
            let Some(slot) = lib.find_mut(root) else {
                return;
            };
            slot.waiters.extend(done);
            if slot.scanning {
                slot.rescan = true;
                return;
            }
            slot.scanning = true;
            slot.gen
        };
        let this = Arc::clone(self);
        let path = root.to_path_buf();
        let spawned = thread::Builder::new()
            .name("lectern-scan".to_owned())
            .spawn(move || {
                // Walking competes with WebView2 for the CPU, so scans wait for first paint.
                this.ui_shown.wait(this.timings.scan_delay);
                loop {
                    this.scan_once(&path, gen);
                    if this.scan_finished(&path, gen) {
                        break;
                    }
                }
            });
        if let Err(e) = spawned {
            log::error!("couldn't start a scan of {}: {e}", root.display());
            if let Some(slot) = lock(&self.library).find_gen_mut(root, gen) {
                slot.scanning = false;
                slot.waiters.clear();
            }
        }
    }

    pub(super) fn scan_once(&self, root: &Path, gen: u64) {
        let Some(adhoc) = self.root_is_adhoc(root, gen) else {
            return;
        };
        if let Err(reason) = probe_root(root, self.timings.probe) {
            log::warn!("library root {} is unavailable: {reason}", root.display());
            self.set_root_state(root, gen, RootState::Unavailable { reason });
            self.notify_waiters(root, gen);
            return;
        }
        if !adhoc {
            // A root that was unavailable couldn't be watched; now that it answers, it can.
            if self.take_needs_watch(root, gen) {
                self.watch_user_roots();
            }
            // Its canonical host (`S:\…` is `\\nas\share\…`) is the user's too. The probe
            // proved the share answers, so this won't stall.
            if let Ok(canonical) = fs::canonicalize(root) {
                if write(&self.trust).learn_root(root, &canonical) {
                    self.index_changed(None);
                }
            }
        }
        let started = Instant::now();
        let opts = ScanOptions::for_root(adhoc);
        let mut index = match scan_root(root, &opts) {
            Ok(index) => index,
            Err(e) => {
                log::warn!("couldn't scan {}: {e}", root.display());
                let reason = e.to_string();
                self.set_root_state(root, gen, RootState::Unavailable { reason });
                self.notify_waiters(root, gen);
                return;
            }
        };
        let walked = started.elapsed();
        if index.truncated {
            log::warn!(
                "only the first {} files of {} were indexed",
                opts.max_files,
                root.display()
            );
        }
        // A root showing no tree yet gets one now; a snapshot's tree stays until the heads are in.
        if !self.has_tree(root, gen) {
            self.install(root, gen, index.clone(), RootState::Scanning);
        }
        self.notify_waiters(root, gen);
        read_heads(&mut index);
        log::info!(
            "indexed {} ({} files): walk {walked:?}, heads {:?}",
            root.display(),
            index.files.len(),
            started.elapsed() - walked
        );
        if !adhoc {
            if let Err(e) = save_snapshot(&self.snapshot_dir, &index) {
                log::warn!("couldn't save the snapshot of {}: {e}", root.display());
            }
        }
        if self.install(root, gen, index, RootState::Ready) {
            self.host.emit(UiEvent::IndexReady(root.to_path_buf()));
        }
    }

    /// Ends a scan unless another was asked for meanwhile; true when the scan thread can stop.
    pub(super) fn scan_finished(&self, root: &Path, gen: u64) -> bool {
        let mut lib = lock(&self.library);
        match lib.find_gen_mut(root, gen) {
            Some(slot) if slot.rescan => {
                slot.rescan = false;
                false
            }
            Some(slot) => {
                slot.scanning = false;
                true
            }
            None => true,
        }
    }

    /// Tells everyone waiting on the root that it has a tree or turned out unavailable.
    fn notify_waiters(&self, root: &Path, gen: u64) {
        let waiters = lock(&self.library)
            .find_gen_mut(root, gen)
            .map(|r| std::mem::take(&mut r.waiters))
            .unwrap_or_default();
        for waiter in waiters {
            let _ = waiter.send(());
        }
    }

    /// Whether the root is the ad-hoc root; `None` once it has been removed.
    pub(super) fn root_is_adhoc(&self, root: &Path, gen: u64) -> Option<bool> {
        lock(&self.library).find_gen(root, gen).map(|r| r.adhoc)
    }

    pub(super) fn has_tree(&self, root: &Path, gen: u64) -> bool {
        lock(&self.library)
            .find_gen(root, gen)
            .is_some_and(|r| r.tree.is_some())
    }

    /// Makes `index` the index of `root`, unless the root was removed meanwhile, and tells the UI.
    /// The current document is rendered again when it is stale, or when files under `root` were
    /// added, removed or moved since the last index (a first index counts as a change).
    pub(super) fn install(
        &self,
        root: &Path,
        gen: u64,
        index: RootIndex,
        state: RootState,
    ) -> bool {
        let tree = build_tree(&index);
        let (payload, files_changed) = {
            let mut lib = lock(&self.library);
            let Some(slot) = lib.find_gen_mut(root, gen) else {
                return false;
            };
            slot.tree = Some(tree);
            slot.state = state;
            slot.truncated = index.truncated;
            let adhoc = slot.adhoc;
            let mut next = (*lib.index).clone();
            let key = path_key(root);
            let files_changed = next
                .roots
                .iter()
                .find(|old| path_key(&old.root) == key)
                .is_none_or(|old| !same_files(old, &index));
            next.upsert_root(index);
            lib.index = Arc::new(next);
            ((!adhoc).then(|| lib.payload()), files_changed)
        };
        self.index_changed(None);
        self.refresh_current(Some(root), files_changed);
        if let Some(payload) = payload {
            self.host.emit(UiEvent::LibraryUpdated(payload));
        }
        true
    }

    pub(super) fn set_root_state(&self, root: &Path, gen: u64, state: RootState) {
        let payload = {
            let mut lib = lock(&self.library);
            let Some(slot) = lib.find_gen_mut(root, gen) else {
                return;
            };
            if matches!(state, RootState::Unavailable { .. }) && !slot.adhoc {
                slot.needs_watch = true;
            }
            slot.state = state;
            let adhoc = slot.adhoc;
            (!adhoc).then(|| lib.payload())
        };
        if let Some(payload) = payload {
            self.host.emit(UiEvent::LibraryUpdated(payload));
        }
    }
}

/// Whether two indexes of one root hold the same files, by path; their heads and timestamps may
/// differ.
fn same_files(a: &RootIndex, b: &RootIndex) -> bool {
    a.files.len() == b.files.len() && rels(a) == rels(b)
}

fn rels(index: &RootIndex) -> HashSet<&str> {
    index.files.iter().map(|f| f.rel.as_str()).collect()
}
