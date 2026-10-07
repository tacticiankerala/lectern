//! Opening a document: the render cache, the payload, which open becomes current, and the
//! silent re-render once the index can do better. Also the recent files, which are the
//! workspace's, and the reading positions, which every window shares.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lectern_core::cache::CacheKey;
use lectern_core::ipc::{
    DocPayload, OpenError, OpenErrorKind, OpenResult, RecentEntry, SavedPosition,
};
use lectern_core::library::{path_key, LibraryIndex};
use lectern_core::render::RENDER_VERSION;
use lectern_core::store::State;

use super::doc::{breadcrumbs, now_ms, read_text, readme_in, render_text, stat_doc, Rendered};
use super::paths::{is_under, path_string, same_path};
use super::sync::{lock, read, write};
use super::{trust, Current, WindowState};
use crate::events::UiEvent;

impl WindowState {
    /// Opens `path`. One on a network host the user hasn't chosen is refused before anything
    /// touches it, and doesn't become the current document.
    pub fn open_document(self: &Arc<Self>, path: &str) -> OpenResult {
        if !self.trusts(path) {
            return refused(path);
        }
        let seq = self.next_seq();
        self.open_numbered(seq, path)
    }

    /// Opens `path` as the open numbered `seq`, which was numbered when it was asked for: a newer
    /// open, numbered while this one waited, stays the current document.
    pub(super) fn open_numbered(self: &Arc<Self>, seq: u64, path: &str) -> OpenResult {
        if !self.trusts(path) {
            return refused(path);
        }
        let path = PathBuf::from(path);
        let result = self.load(&path);
        self.finish_open(seq, &path, result)
    }

    pub(super) fn load(&self, path: &Path) -> Result<Rendered, OpenError> {
        // The generation is read before the index, so a render against an index replaced
        // meanwhile is never cached.
        let gen = self.index_gen.load(Ordering::SeqCst);
        let index = self.index();
        let stamp = stat_doc(path)?;
        let covered = index.root_for(path).is_some();
        let key = CacheKey {
            path: path_key(path),
            mtime_ms: stamp.mtime_ms,
            size: stamp.size,
            version: RENDER_VERSION,
        };
        if let Some(doc) = lock(&self.cache).get(&key) {
            return Ok(Rendered {
                doc,
                mtime_ms: stamp.mtime_ms,
                lossy: false,
                covered,
                gen,
            });
        }
        let text = read_text(path)?;
        let mapper = self.mapper();
        let hosts = read(&self.app.trust).hosts();
        let with_index = (!index.roots.is_empty()).then_some(&*index);
        let doc = Arc::new(render_text(path, &text.text, with_index, &mapper, &hosts));
        // Lossy renders aren't cached, so a cache hit is never lossy.
        if !text.lossy {
            let mut cache = lock(&self.cache);
            if self.index_gen.load(Ordering::SeqCst) == gen {
                cache.put(key, Arc::clone(&doc));
            }
        }
        Ok(Rendered {
            doc,
            mtime_ms: stamp.mtime_ms,
            lossy: text.lossy,
            covered,
            gen,
        })
    }

    pub(super) fn finish_open(
        self: &Arc<Self>,
        seq: u64,
        path: &Path,
        result: Result<Rendered, OpenError>,
    ) -> OpenResult {
        let rendered = match result {
            Ok(rendered) => rendered,
            Err(error) => {
                // Watched even while missing, so its return is noticed.
                self.make_current(seq, path, None);
                return OpenResult::Err { error };
            }
        };
        let doc = self.doc_payload(path, &rendered, &self.index());
        let current = self.make_current(seq, path, Some(&rendered));
        if current {
            // An index installed while this rendered has already looked for stale documents and
            // found the previous one; this one gets its look now.
            self.refresh_if_stale(None);
        }
        let entry = RecentEntry {
            path: doc.path.clone(),
            title: doc.title.clone(),
            opened_ms: now_ms(),
        };
        self.change_own_workspace(|ws| {
            as_reading(&mut ws.recent, |reading| reading.push_recent(entry));
            if current {
                ws.last_doc = Some(doc.path.clone());
            }
            true
        });
        // The open succeeded, so the image protocol may serve from the document's folder.
        if let Some(dir) = path.parent() {
            write(&self.app.assets).add_folder(dir);
        }
        if current {
            self.ensure_root_for(path, seq);
        }
        OpenResult::Ok { doc }
    }

    pub(super) fn doc_payload(
        &self,
        path: &Path,
        rendered: &Rendered,
        index: &LibraryIndex,
    ) -> DocPayload {
        let path_text = path_string(path);
        let (user_root, crumb_root) = {
            let lib = lock(&self.library);
            let deepest = |adhoc: bool| {
                lib.roots
                    .iter()
                    .filter(|r| r.adhoc == adhoc && is_under(path, &r.path))
                    .max_by_key(|r| path_key(&r.path).len())
                    .map(|r| r.path.clone())
            };
            let user_root = deepest(false);
            let crumb_root = user_root.clone().or_else(|| deepest(true));
            (user_root, crumb_root)
        };
        let parent = path.parent().unwrap_or(path).to_path_buf();
        let breadcrumbs = breadcrumbs(path, crumb_root.as_deref().unwrap_or(&parent), |dir| {
            readme_in(index, dir)
        });
        let doc = &rendered.doc;
        DocPayload {
            position: lock(&self.app.state).reading.position(&path_text).cloned(),
            path: path_text,
            title: doc.title.clone(),
            html: doc.html.clone(),
            outline: doc.outline.clone(),
            frontmatter: doc.frontmatter.clone(),
            tasks: doc.tasks.clone(),
            word_count: doc.word_count,
            mtime_ms: rendered.mtime_ms,
            lossy: rendered.lossy,
            breadcrumbs,
            root_path: user_root.as_deref().map(path_string),
        }
    }

    /// Makes `path` the current document unless a newer open (a higher `seq`) already did; true
    /// when it did. The watcher is told under the same lock, so it always polls the current one.
    pub(super) fn make_current(&self, seq: u64, path: &Path, rendered: Option<&Rendered>) -> bool {
        let mut current = lock(&self.current);
        if current.as_ref().is_some_and(|c| c.seq > seq) {
            return false;
        }
        let refreshed_at = current
            .as_ref()
            .filter(|c| same_path(&c.path, path))
            .and_then(|c| c.refreshed_at);
        *current = Some(Current {
            path: path.to_path_buf(),
            seq,
            stale: rendered.is_some_and(|r| !r.covered || r.doc.has_unresolved_wikilinks),
            rendered_gen: rendered.map_or(0, |r| r.gen),
            refreshed_at,
            doc: rendered.map(|r| Arc::clone(&r.doc)),
        });
        if let Some(watch) = &*lock(&self.watch) {
            watch.doc(Some(path.to_path_buf()));
        }
        true
    }

    /// Asks the UI to reload the current document when it is stale and an index newer than its
    /// render covers it (below `root`, when given), at most once per index generation. Only once
    /// the window has shown its first document.
    pub(super) fn refresh_if_stale(&self, root: Option<&Path>) {
        self.refresh_current(root, false);
    }

    /// Asks the UI to reload the current document when an index newer than its render covers it
    /// (below `root`, when given) and either it is stale or, with `files_changed`, files below
    /// `root` were added, removed or moved, so the links it resolved may point at paths that
    /// are gone. At most once per index generation, and only once the window has shown its
    /// first document.
    pub(super) fn refresh_current(&self, root: Option<&Path>, files_changed: bool) {
        if !self.ui_shown.is_open() {
            return;
        }
        let gen = self.index_gen.load(Ordering::SeqCst);
        let index = self.index();
        let path = {
            let mut current = lock(&self.current);
            match current.as_mut() {
                Some(c)
                    if (c.stale || files_changed)
                        && c.rendered_gen < gen
                        && c.refreshed_at != Some(gen)
                        && root.is_none_or(|root| is_under(&c.path, root))
                        && index.root_for(&c.path).is_some() =>
                {
                    c.refreshed_at = Some(gen);
                    Some(c.path.clone())
                }
                _ => None,
            }
        };
        if let Some(path) = path {
            self.emit(UiEvent::DocChanged(path));
        }
    }

    /// Asks the UI to reload the current document, stale or not, once the window has shown it.
    /// After the path mappings change, any link or image it resolved may point elsewhere, even
    /// in a document the index already covers. A document that failed to open is left alone.
    pub(super) fn refresh_current_now(&self) {
        if !self.ui_shown.is_open() {
            return;
        }
        let gen = self.index_gen.load(Ordering::SeqCst);
        let path = lock(&self.current)
            .as_mut()
            .filter(|c| c.doc.is_some())
            .map(|c| {
                c.refreshed_at = Some(gen);
                c.path.clone()
            });
        if let Some(path) = path {
            self.emit(UiEvent::DocChanged(path));
        }
    }

    /// The recent files of the window's workspace, newest first.
    pub(super) fn recent(&self) -> Vec<RecentEntry> {
        self.workspace_id()
            .and_then(|id| self.app.read_workspace(&id, |ws| ws.recent.clone()))
            .unwrap_or_default()
    }

    /// Drops `path` from the workspace's recent files, for good; returns the recent files left.
    pub fn remove_recent(&self, path: &str) -> Vec<RecentEntry> {
        self.change_own_workspace(|ws| {
            as_reading(&mut ws.recent, |reading| reading.remove_recent(path))
        });
        self.recent()
    }

    /// Saves the reading position in `path`, which every window shares.
    pub fn save_position(&self, path: &str, position: SavedPosition) {
        let mut state = lock(&self.app.state);
        state.reading.set_position(path, position, now_ms());
        self.app.saver.state(&state);
    }
}

/// Runs `f` on `recent` as a reading state's recent files, so a workspace's keep the same rules:
/// newest first, each path once, 20 at most.
fn as_reading<R>(recent: &mut Vec<RecentEntry>, f: impl FnOnce(&mut State) -> R) -> R {
    let mut reading = State {
        recent: std::mem::take(recent),
        ..State::default()
    };
    let result = f(&mut reading);
    *recent = reading.recent;
    result
}

/// The answer for a path on a network host the user hasn't chosen.
fn refused(path: &str) -> OpenResult {
    OpenResult::Err {
        error: OpenError {
            kind: OpenErrorKind::Permission,
            message: trust::refusal(path),
            path: path.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::*;
    use lectern_core::library::pathmap::PathMapper;
    use std::fs;

    use crate::state::doc::render_file;
    use crate::state::sync::lock;
    use lectern_core::workspace::WORKSPACES_FILE;

    #[test]
    fn an_older_open_finishing_late_never_becomes_current() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("one/a.md", "# A");
        let b = f.dir.file("two/b.md", "# B");
        let mapper = PathMapper::default();
        let (ra, rb) = (render_file(&a, &mapper, &[]), render_file(&b, &mapper, &[]));
        f.state.finish_open(2, &a, ra);
        f.state.finish_open(1, &b, rb);
        assert_eq!(current(&f).0, a);
        let docs: Vec<Watched> = lock(&f.watched)
            .iter()
            .filter(|w| matches!(w, Watched::Doc(_)))
            .cloned()
            .collect();
        assert_eq!(docs, [Watched::Doc(Some(a.clone()))]);
        let lib = lock(&f.state.library);
        assert!(lib
            .roots
            .iter()
            .any(|r| r.adhoc && same_path(&r.path, a.parent().unwrap())));
        assert!(!lib
            .roots
            .iter()
            .any(|r| same_path(&r.path, b.parent().unwrap())));
        drop(lib);
        assert_eq!(last_doc(&f), Some(path_string(&a)));
    }

    #[test]
    fn a_doc_outside_every_indexed_root_is_stale() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let inside = dir.file("vault/a.md", "# A");
        let outside = dir.file("elsewhere/b.md", "# B");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        wait_until("the root is indexed", || settled(&f, &root));
        f.state.open_document(&path_string(&outside));
        assert_eq!(current(&f), (outside, true));
        f.state.open_document(&path_string(&inside));
        assert_eq!(current(&f), (inside, false));
    }

    #[test]
    fn a_stale_doc_is_refreshed_once_per_index_generation() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/a.md", "# A\n\nSee [[nowhere]].\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        wait_until("the root is indexed", || settled(&f, &root));
        f.state.window_shown();
        f.state.open_document(&path_string(&doc));
        assert!(current(&f).1, "an unresolved wikilink keeps it stale");
        f.state.index_changed(Some(&root));
        assert_eq!(f.host.doc_changes(), 1);
        f.state.refresh_if_stale(Some(&root));
        assert_eq!(f.host.doc_changes(), 1);
        // The UI reloads it: rendered against the current generation, it isn't refreshed again.
        f.state.open_document(&path_string(&doc));
        f.state.refresh_if_stale(None);
        assert_eq!(f.host.doc_changes(), 1);
        f.state.index_changed(Some(&root));
        assert_eq!(f.host.doc_changes(), 2);
    }

    #[test]
    fn a_rescan_that_moves_a_file_refreshes_the_current_doc_once() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/a.md", "# A\n\nSee [[b]].\n");
        let b = dir.file("vault/work/b.md", "# B");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        wait_until("the root is indexed", || settled(&f, &root));
        f.state.window_shown();
        f.state.open_document(&path_string(&doc));
        assert_eq!(current(&f), (doc.clone(), false), "its wikilink resolved");
        // A rescan that finds the same files leaves the document alone.
        rescan(&f, &root);
        assert_eq!(f.host.doc_changes(), 0);
        // B moves: the document's resolved link points at a path that is gone, so it is
        // rendered again, once.
        fs::create_dir_all(root.join("archive")).unwrap();
        fs::rename(&b, root.join("archive").join("b.md")).unwrap();
        rescan(&f, &root);
        assert_eq!(f.host.doc_changes(), 1);
        rescan(&f, &root);
        assert_eq!(f.host.doc_changes(), 1);
    }

    #[test]
    fn removing_a_recent_entry_saves_the_rest() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("one/a.md", "# A");
        let b = f.dir.file("two/b.md", "# B");
        f.state.open_document(&path_string(&a));
        f.state.open_document(&path_string(&b));
        // Paths compare as on Windows.
        let left = f.state.remove_recent(&path_string(&a).to_uppercase());
        let left: Vec<PathBuf> = left.iter().map(|r| PathBuf::from(&r.path)).collect();
        assert_eq!(left, [b]);
        f.app.flush();
        let saved = fs::read_to_string(f.config.join(WORKSPACES_FILE)).unwrap();
        assert!(!saved.contains("a.md"), "{saved}");
        assert!(saved.contains("b.md"), "{saved}");
    }

    /// Scans `root` again and waits for that scan to finish.
    fn rescan(f: &Fixture, root: &Path) {
        f.state.request_scan(root, None);
        wait_until("the rescan finishes", || settled(f, root));
    }

    #[test]
    fn a_stalled_open_never_replaces_the_adhoc_root_of_a_newer_one() {
        let f = fixture(profile(&[]), FakeHost::default());
        let a = f.dir.file("one/a.md", "# A");
        let b = f.dir.file("two/b.md", "# B");
        let (folder_a, folder_b) = (
            a.parent().unwrap().to_owned(),
            b.parent().unwrap().to_owned(),
        );
        let mapper = PathMapper::default();
        let (ra, rb) = (render_file(&a, &mapper, &[]), render_file(&b, &mapper, &[]));
        // A became current, then B did, before A got to its ad-hoc root.
        let (seq_a, seq_b) = (f.state.next_seq(), f.state.next_seq());
        assert!(f.state.make_current(seq_a, &a, ra.as_ref().ok()));
        assert!(f.state.make_current(seq_b, &b, rb.as_ref().ok()));
        f.state.ensure_root_for(&b, seq_b);
        f.state.ensure_root_for(&a, seq_a);
        let lib = lock(&f.state.library);
        assert!(lib
            .roots
            .iter()
            .any(|r| r.adhoc && same_path(&r.path, &folder_b)));
        assert!(!lib.roots.iter().any(|r| same_path(&r.path, &folder_a)));
    }

    #[test]
    fn a_render_that_lands_after_the_last_install_is_refreshed() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/a.md", "# A\n\nSee [[nowhere]].\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        wait_until("the root is indexed", || settled(&f, &root));
        f.state.window_shown();
        // The render starts, an index lands while it runs, then it becomes current.
        let seq = f.state.next_seq();
        let rendered = f.state.load(&doc);
        f.state.index_changed(Some(&root));
        assert_eq!(
            f.host.doc_changes(),
            0,
            "nothing was current to refresh yet"
        );
        f.state.finish_open(seq, &doc, rendered);
        assert_eq!(f.host.doc_changes(), 1);
    }
}
