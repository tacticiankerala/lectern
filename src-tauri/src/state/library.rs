//! The library: the user's roots and the ad-hoc root, their index, and keeping them in line with
//! the settings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use lectern_core::cache::RenderCache;
use lectern_core::ipc::{Candidate, LibraryPayload, RootState, RootView};
use lectern_core::library::snapshot::load_snapshot;
use lectern_core::library::tree::TreeNode;
use lectern_core::library::{path_key, LibraryIndex};
use lectern_core::search::FileHits;

use super::paths::{is_under, normalize_root, path_string, root_name, same_path};
use super::sync::{lock, read, write};
use super::{AppState, CACHE_CAP};
use crate::events::UiEvent;

/// A library root as the app tracks it.
pub(super) struct Root {
    pub(super) path: PathBuf,
    /// The folder of a document opened outside every root; indexed, but not in the sidebar.
    pub(super) adhoc: bool,
    /// Unique per root added, so a scan of a root since removed is ignored.
    pub(super) gen: u64,
    pub(super) state: RootState,
    pub(super) tree: Option<TreeNode>,
    /// Its index stopped at the scan's file cap.
    pub(super) truncated: bool,
    pub(super) scanning: bool,
    /// A change came in during the scan, so it runs again.
    pub(super) rescan: bool,
    /// Its OS watch may not be registered (it was unavailable when the roots were watched), so
    /// the next scan that reaches it registers the watches again.
    pub(super) needs_watch: bool,
    /// Callers (`add_root`, `retry_root`) waiting for the next scan to give it a tree or find it
    /// unavailable.
    pub(super) waiters: Vec<Sender<()>>,
}

#[derive(Default)]
pub(super) struct Library {
    /// The user's roots in settings order, then the ad-hoc root.
    pub(super) roots: Vec<Root>,
    /// Every root with an index, swapped whole on each change so renders never wait.
    pub(super) index: Arc<LibraryIndex>,
    pub(super) next_gen: u64,
}

impl Library {
    pub(super) fn push_root(&mut self, path: PathBuf, adhoc: bool) -> u64 {
        self.next_gen += 1;
        self.roots.push(Root {
            path,
            adhoc,
            gen: self.next_gen,
            state: RootState::Scanning,
            tree: None,
            truncated: false,
            scanning: false,
            rescan: false,
            needs_watch: false,
            waiters: Vec::new(),
        });
        self.next_gen
    }

    pub(super) fn find_mut(&mut self, path: &Path) -> Option<&mut Root> {
        self.roots.iter_mut().find(|r| same_path(&r.path, path))
    }

    pub(super) fn find_gen(&self, path: &Path, gen: u64) -> Option<&Root> {
        self.roots
            .iter()
            .find(|r| r.gen == gen && same_path(&r.path, path))
    }

    pub(super) fn find_gen_mut(&mut self, path: &Path, gen: u64) -> Option<&mut Root> {
        self.roots
            .iter_mut()
            .find(|r| r.gen == gen && same_path(&r.path, path))
    }

    /// Removes the roots `remove` picks, with their indexes. True when any went.
    pub(super) fn remove_where(&mut self, remove: impl Fn(&Root) -> bool) -> bool {
        let gone: Vec<String> = self
            .roots
            .iter()
            .filter(|r| remove(r))
            .map(|r| path_key(&r.path))
            .collect();
        if gone.is_empty() {
            return false;
        }
        self.roots.retain(|r| !gone.contains(&path_key(&r.path)));
        let mut index = (*self.index).clone();
        index.roots.retain(|r| !gone.contains(&path_key(&r.root)));
        self.index = Arc::new(index);
        true
    }

    pub(super) fn payload(&self) -> LibraryPayload {
        LibraryPayload {
            roots: self
                .roots
                .iter()
                .filter(|r| !r.adhoc)
                .map(|r| RootView {
                    path: path_string(&r.path),
                    name: root_name(&r.path),
                    state: r.state.clone(),
                    tree: r.tree.clone(),
                    truncated: r.truncated,
                })
                .collect(),
        }
    }

    pub(super) fn user_roots(&self) -> Vec<(PathBuf, u64)> {
        self.roots
            .iter()
            .filter(|r| !r.adhoc)
            .map(|r| (r.path.clone(), r.gen))
            .collect()
    }
}

impl AppState {
    /// Loads the library snapshots, then (once the first document is on screen) starts the
    /// watcher and scans every root. Runs on a thread of its own.
    pub fn start_library(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("lectern-library".to_owned())
            .spawn(move || this.boot_library());
        if let Err(e) = spawned {
            log::error!("couldn't start the library thread: {e}");
            self.snapshots_loaded.open();
        }
    }

    pub(super) fn boot_library(self: &Arc<Self>) {
        let roots = lock(&self.library).user_roots();
        let started = Instant::now();
        // Snapshots are local files; a root itself is touched only after its probe answers.
        for (root, gen) in &roots {
            if let Some(index) = load_snapshot(&self.snapshot_dir, root) {
                self.install(root, *gen, index, RootState::Scanning);
            }
        }
        log::info!(
            "loaded {} library snapshots in {:?}",
            roots.len(),
            started.elapsed()
        );
        self.snapshots_loaded.open();
        // Like the scans, the watcher starts once the first document is on screen.
        self.ui_shown.wait(self.timings.scan_delay);
        self.watch_user_roots();
        for (root, _) in &roots {
            self.request_scan(root, None);
        }
    }

    pub fn library_payload(&self) -> LibraryPayload {
        lock(&self.library).payload()
    }

    pub fn quick_open_candidates(&self) -> Vec<Candidate> {
        let index = self.index();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for root in &index.roots {
            let root_text = path_string(&root.root);
            for file in root.md_files() {
                let abs = root.abs(&file.rel);
                if !seen.insert(path_key(&abs)) {
                    continue;
                }
                candidates.push(Candidate {
                    path: path_string(&abs),
                    name: file.rel.rsplit('/').next().unwrap_or(&file.rel).to_owned(),
                    rel: file.rel.clone(),
                    root: root_text.clone(),
                });
            }
        }
        candidates
    }

    pub fn search(&self, query: &str) -> Vec<FileHits> {
        self.content.search(&self.index(), query)
    }

    /// Adds `path` as a library root and waits a few seconds for its tree. An unreachable root
    /// comes back unavailable within the probe timeout.
    pub fn add_root(self: &Arc<Self>, path: &str) -> Result<LibraryPayload, String> {
        self.insert_root(path, true)?;
        Ok(self.library_payload())
    }

    /// Adds a folder from the command line or a second launch as a library root, without
    /// waiting for its scan, unless it is already a root, sits in one or holds one.
    pub(super) fn add_folder(self: &Arc<Self>, folder: &Path) {
        let roots: Vec<PathBuf> = lock(&self.library)
            .user_roots()
            .into_iter()
            .map(|(root, _)| root)
            .collect();
        if !folder_becomes_root(folder, &roots) {
            return;
        }
        if let Err(e) = self.insert_root(&path_string(folder), false) {
            log::warn!("couldn't add {} to the library: {e}", folder.display());
        }
    }

    pub(super) fn insert_root(self: &Arc<Self>, path: &str, wait: bool) -> Result<(), String> {
        let root = normalize_root(path)?;
        let added = {
            let mut settings = write(&self.settings);
            let known = settings
                .library_roots
                .iter()
                .any(|r| same_path(Path::new(r), &root));
            if !known {
                settings.library_roots.push(path_string(&root));
                self.saver.settings(&settings);
            }
            !known
        };
        if added {
            for (root, gen) in self.sync_roots() {
                self.start_root(&root, gen, wait);
            }
            self.watch_user_roots();
        }
        Ok(())
    }

    pub fn remove_root(&self, path: &str) -> LibraryPayload {
        {
            let mut settings = write(&self.settings);
            settings
                .library_roots
                .retain(|r| !same_path(Path::new(r), Path::new(path)));
            self.saver.settings(&settings);
        }
        self.sync_roots();
        self.watch_user_roots();
        self.library_payload()
    }

    /// Probes and scans `path` again, waiting a few seconds for the outcome.
    pub fn retry_root(self: &Arc<Self>, path: &str) -> LibraryPayload {
        let root = PathBuf::from(path);
        let payload = {
            let mut lib = lock(&self.library);
            match lib.find_mut(&root) {
                Some(slot) if !slot.adhoc => {
                    slot.state = RootState::Scanning;
                    Some(lib.payload())
                }
                _ => None,
            }
        };
        if let Some(payload) = payload {
            self.host.emit(UiEvent::LibraryUpdated(payload));
            let (done, finished) = mpsc::channel();
            self.request_scan(&root, Some(done));
            let _ = finished.recv_timeout(self.timings.root);
        }
        self.library_payload()
    }

    /// Brings the tracked roots in line with the settings: new ones are added (and returned, to
    /// be started), removed ones dropped with their indexes. An ad-hoc root that becomes a user
    /// root is promoted, keeping its index; one that a user root now covers is dropped.
    pub(super) fn sync_roots(&self) -> Vec<(PathBuf, u64)> {
        let wanted: Vec<PathBuf> = read(&self.settings)
            .library_roots
            .iter()
            .map(PathBuf::from)
            .collect();
        let mut lib = lock(&self.library);
        let mut changed =
            lib.remove_where(|r| !r.adhoc && !wanted.iter().any(|w| same_path(w, &r.path)));
        let mut added = Vec::new();
        for path in &wanted {
            if lib
                .roots
                .iter()
                .any(|r| !r.adhoc && same_path(&r.path, path))
            {
                continue;
            }
            if let Some(adhoc) = lib.roots.iter_mut().find(|r| same_path(&r.path, path)) {
                adhoc.adhoc = false;
                adhoc.path = path.clone();
                added.push((path.clone(), adhoc.gen));
                continue;
            }
            added.push((path.clone(), lib.push_root(path.clone(), false)));
        }
        changed |= lib.remove_where(|r| r.adhoc && wanted.iter().any(|w| is_under(&r.path, w)));
        let order = |r: &Root| {
            if r.adhoc {
                usize::MAX
            } else {
                wanted
                    .iter()
                    .position(|w| same_path(w, &r.path))
                    .unwrap_or(usize::MAX)
            }
        };
        lib.roots.sort_by_key(order);
        drop(lib);
        // The roots' hosts are trusted; images on them render once the cache is cleared.
        changed |= self.reconfigure_trust();
        write(&self.assets).set_roots(&wanted);
        if changed {
            self.index_changed(None);
        }
        added
    }

    /// Shows a new root's snapshot, if it has one, then scans it; with `wait`, until its tree is
    /// ready or it turns out unavailable, for a few seconds at most.
    pub(super) fn start_root(self: &Arc<Self>, root: &Path, gen: u64, wait: bool) {
        if let Some(index) = load_snapshot(&self.snapshot_dir, root) {
            self.install(root, gen, index, RootState::Scanning);
        }
        let (done, finished) = mpsc::channel();
        self.request_scan(root, wait.then_some(done));
        if wait {
            let _ = finished.recv_timeout(self.timings.root);
        }
    }

    /// Indexes the folder of `doc` when no root covers it, replacing the previous ad-hoc root.
    /// Only while the open numbered `seq` is still the current document, checked under the
    /// library lock: an open that stalled (on the asset scope, say) must not replace the ad-hoc
    /// root of a document opened since.
    pub(super) fn ensure_root_for(self: &Arc<Self>, doc: &Path, seq: u64) {
        let Some(folder) = doc.parent().map(Path::to_path_buf) else {
            return;
        };
        let dropped = {
            let mut lib = lock(&self.library);
            let still_current = lock(&self.current).as_ref().is_some_and(|c| c.seq == seq);
            if !still_current || lib.roots.iter().any(|r| is_under(doc, &r.path)) {
                return;
            }
            let dropped = lib.remove_where(|r| r.adhoc);
            lib.push_root(folder.clone(), true);
            dropped
        };
        if dropped {
            self.index_changed(None);
        }
        self.request_scan(&folder, None);
    }

    /// The index changed: cached renders may resolve links differently now.
    pub(super) fn index_changed(&self, root: Option<&Path>) {
        {
            let mut cache = lock(&self.cache);
            self.index_gen.fetch_add(1, Ordering::SeqCst);
            *cache = RenderCache::new(CACHE_CAP);
        }
        if let Some(root) = root {
            self.refresh_if_stale(Some(root));
        }
    }

    /// The user's library roots holding `path`: more than one when roots nest, none for a file
    /// outside them all. The ad-hoc root of a file opened from elsewhere isn't one.
    pub(super) fn user_roots_holding(&self, path: &Path) -> Vec<PathBuf> {
        lock(&self.library)
            .roots
            .iter()
            .filter(|r| !r.adhoc && is_under(path, &r.path))
            .map(|r| r.path.clone())
            .collect()
    }

    pub(super) fn watch_user_roots(&self) {
        let roots = lock(&self.library)
            .user_roots()
            .into_iter()
            .map(|(root, _)| root)
            .collect();
        self.watch.roots(roots);
    }

    /// Takes the root's "needs watching" mark; true when it had one.
    pub(super) fn take_needs_watch(&self, root: &Path, gen: u64) -> bool {
        lock(&self.library)
            .find_gen_mut(root, gen)
            .is_some_and(|r| std::mem::take(&mut r.needs_watch))
    }
}

/// Whether a folder from a launch becomes a library root: not when it is a root already, sits
/// inside one, or holds one.
pub(super) fn folder_becomes_root(folder: &Path, roots: &[PathBuf]) -> bool {
    !roots
        .iter()
        .any(|root| is_under(folder, root) || is_under(root, folder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::*;
    use lectern_core::library::RootIndex;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn promoting_the_adhoc_folder_keeps_one_root_with_its_index() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = f.dir.file("notes/x.md", "# X");
        let folder = doc.parent().unwrap().to_path_buf();
        f.state.open_document(&path_string(&doc));
        wait_until("the ad-hoc folder is indexed", || indexed(&f, &doc));
        f.state.add_root(&path_string(&folder)).unwrap();
        let lib = lock(&f.state.library);
        assert_eq!(lib.roots.len(), 1);
        assert!(!lib.roots[0].adhoc);
        assert!(same_path(&lib.roots[0].path, &folder));
        drop(lib);
        assert!(indexed(&f, &doc));
        assert_eq!(f.state.library_payload().roots.len(), 1);
    }

    #[test]
    fn retrying_a_root_that_came_back_watches_it_again() {
        let f = fixture(profile(&[]), FakeHost::default());
        let root = f.dir.0.join("later");
        let library = f.state.add_root(&path_string(&root)).unwrap();
        assert!(matches!(
            library.roots[0].state,
            RootState::Unavailable { .. }
        ));
        let watches = || {
            lock(&f.watched)
                .iter()
                .filter(|w| matches!(w, Watched::Roots(r) if r.contains(&root)))
                .count()
        };
        let before = watches();
        f.dir.file("later/a.md", "# A");
        f.state.retry_root(&path_string(&root));
        wait_until("the root is watched again", || watches() > before);
    }

    #[test]
    fn replacing_the_adhoc_root_drops_its_index() {
        let f = fixture(profile(&[]), FakeHost::default());
        let x = f.dir.file("one/x.md", "# X");
        let y = f.dir.file("two/y.md", "# Y");
        f.state.open_document(&path_string(&x));
        wait_until("the first folder is indexed", || indexed(&f, &x));
        let gen = f.state.index_gen.load(Ordering::SeqCst);
        f.state.open_document(&path_string(&y));
        assert!(f.state.index_gen.load(Ordering::SeqCst) > gen);
        assert!(!indexed(&f, &x));
    }

    #[test]
    fn launch_folders_never_nest_roots() {
        let roots = [PathBuf::from(r"S:\Notes\My Vault\dev")];
        assert!(folder_becomes_root(Path::new(r"C:\Notes"), &roots));
        assert!(folder_becomes_root(
            Path::new(r"S:\Notes\My Vault\devices"),
            &roots
        ));
        // Inside a root, the root itself (any case or trailing separator), or holding one.
        assert!(!folder_becomes_root(
            Path::new(r"S:\Notes\My Vault\dev\work\a"),
            &roots
        ));
        assert!(!folder_becomes_root(
            Path::new(r"s:\notes\my vault\DEV\"),
            &roots
        ));
        assert!(!folder_becomes_root(Path::new(r"S:\Notes"), &roots));
        assert!(folder_becomes_root(Path::new(r"S:\Notes"), &[]));
    }

    #[test]
    fn a_retry_during_a_running_scan_waits_for_its_result() {
        let f = fixture(profile(&[]), FakeHost::default());
        let root = f.dir.folder("vault");
        f.dir.file("vault/a.md", "# A");
        // The first scan gives the tree, then stalls telling the UI its index is ready, still
        // running.
        *lock(&f.host.hold_index_ready) = Some(root.clone());
        f.state.add_root(&path_string(&root)).unwrap();
        wait_until("the scan stalls", || f.host.indexed(&root));
        let (state, path) = (Arc::clone(&f.state), path_string(&root));
        let started = Instant::now();
        let retry = thread::spawn(move || state.retry_root(&path));
        thread::sleep(Duration::from_millis(300));
        f.host.release();
        let library = retry.join().unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "answered before the scan did"
        );
        // The rescan answered: the root has its tree and answers; `Ready` follows by event.
        assert!(library.roots[0].tree.is_some());
        assert!(!matches!(
            library.roots[0].state,
            RootState::Unavailable { .. }
        ));
    }

    #[test]
    fn adding_an_unreachable_root_answers_unavailable_quickly() {
        let f = fixture(profile(&[]), FakeHost::default());
        let missing = f.dir.0.join("not-there");
        let started = Instant::now();
        let library = f.state.add_root(&path_string(&missing)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(3500));
        assert!(matches!(
            library.roots[0].state,
            RootState::Unavailable { .. }
        ));
    }

    #[test]
    fn a_root_indexed_only_up_to_the_cap_says_so() {
        let f = fixture(profile(&[]), FakeHost::default());
        let root = f.dir.folder("vault");
        f.dir.file("vault/a.md", "# A");
        f.state.add_root(&path_string(&root)).unwrap();
        wait_until("the root is indexed", || settled(&f, &root));
        assert!(!f.state.library_payload().roots[0].truncated);
        let gen = lock(&f.state.library).find_mut(&root).unwrap().gen;
        let mut capped = RootIndex::new(root.clone(), Vec::new(), 0);
        capped.truncated = true;
        f.state.install(&root, gen, capped, RootState::Ready);
        assert!(f.state.library_payload().roots[0].truncated);
        let told = lock(&f.host.events)
            .iter()
            .any(|e| matches!(e, UiEvent::LibraryUpdated(library) if library.roots[0].truncated));
        assert!(told, "the UI heard nothing");
    }

    #[test]
    fn snapshots_load_even_when_a_root_is_missing() {
        let dir = TempDir::new();
        let missing = dir.0.join("not-there");
        let f = fixture_in(dir, profile(&[&missing]), FakeHost::default());
        assert!(f.state.snapshots_loaded.wait(Duration::from_secs(1)));
    }
}
