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
use lectern_core::library::pathmap::page_url;
use lectern_core::library::{path_key, DocKind, LibraryIndex};
use lectern_core::page::PageScope;
use lectern_core::render::RENDER_VERSION;
use lectern_core::store::State;

use super::doc::{
    breadcrumbs, check_size, framed, now_ms, opened_kind, read_text, readme_in, render_doc,
    stat_doc, Rendered,
};
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

    /// Reads and renders `path`, or takes its render from the cache. An HTML page or a PDF is
    /// only stated: it shows in a frame. Anything over its kind's size cap is refused unread.
    pub(super) fn load(&self, path: &Path) -> Result<Rendered, OpenError> {
        // The generation is read before the index, so a render against an index replaced
        // meanwhile is never cached.
        let gen = self.index_gen.load(Ordering::SeqCst);
        let index = self.index();
        let kind = opened_kind(path);
        let stamp = stat_doc(path)?;
        let covered = index.root_for(path).is_some();
        check_size(path, kind, stamp.size)?;
        if kind.framed() {
            return Ok(Rendered {
                covered,
                gen,
                ..framed(path, kind, &stamp)?
            });
        }
        let key = CacheKey {
            path: path_key(path),
            mtime_ms: stamp.mtime_ms,
            size: stamp.size,
            version: RENDER_VERSION,
        };
        if let Some(doc) = lock(&self.cache).get(&key) {
            return Ok(Rendered {
                kind,
                doc,
                framed: None,
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
        let doc = Arc::new(render_doc(
            path, kind, &text.text, with_index, &mapper, &hosts,
        ));
        // Lossy renders aren't cached, so a cache hit is never lossy.
        if !text.lossy {
            let mut cache = lock(&self.cache);
            if self.index_gen.load(Ordering::SeqCst) == gen {
                cache.put(key, Arc::clone(&doc));
            }
        }
        Ok(Rendered {
            kind,
            doc,
            framed: None,
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
        // Each open of a page or a PDF has a URL of its own, which reaches the page's folder and
        // below, which its reports name, and whose bridge has a key of its own.
        let open = rendered
            .framed
            .as_deref()
            .map(|page| self.app.open_page(&self.label, page, folder_of(page)));
        let doc = self.doc_payload(path, &rendered, &self.index(), open.as_ref());
        let current = self.make_current(seq, path, Some(&rendered));
        if current {
            self.pin_open(seq, open.as_ref().map(|o| o.token.as_str()));
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

    /// The payload of `path`, rendered as `rendered`, under `open` when it is a page or a PDF.
    pub(super) fn doc_payload(
        &self,
        path: &Path,
        rendered: &Rendered,
        index: &LibraryIndex,
        open: Option<&PageScope>,
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
        let frame_url = open.and_then(|o| page_url(&o.token, Path::new(&o.page)));
        let bridge_key = open
            .filter(|_| rendered.kind == DocKind::Html)
            .map(|o| o.key.clone());
        DocPayload {
            position: lock(&self.app.state).reading.position(&path_text).cloned(),
            path: path_text,
            kind: rendered.kind,
            frame_url,
            bridge_key,
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

    /// Makes `path` the current document unless a newer open (a higher `seq`) already did, or the
    /// UI left its document since this one began (`leave_document`); true when it did. The watcher
    /// is told under the same lock, so it always polls the current one.
    pub(super) fn make_current(&self, seq: u64, path: &Path, rendered: Option<&Rendered>) -> bool {
        let mut current = lock(&self.current);
        if current.as_ref().is_some_and(|c| c.seq > seq)
            || self.left_at.load(Ordering::SeqCst) > seq
        {
            return false;
        }
        let refreshed_at = current
            .as_ref()
            .filter(|c| same_path(&c.path, path))
            .and_then(|c| c.refreshed_at);
        *current = Some(Current {
            path: path.to_path_buf(),
            seq,
            stale: follows_the_index(path)
                && rendered.is_some_and(|r| !r.covered || r.doc.has_unresolved_wikilinks),
            rendered_gen: rendered.map_or(0, |r| r.gen),
            refreshed_at,
            doc: rendered.map(|r| Arc::clone(&r.doc)),
        });
        if let Some(watch) = &*lock(&self.watch) {
            watch.doc(Some(path.to_path_buf()));
        }
        true
    }

    /// The UI left its document for none (the welcome page, or a file deleted while open): there
    /// is no current document, so nothing acts on the one it showed (an agent's read, highlight or
    /// comment, a review command), and an open begun before now never becomes current. Numbered as
    /// an open is, so an open begun after it does. The watcher stops watching it.
    pub fn leave_document(&self) {
        let seq = self.next_seq();
        let mut current = lock(&self.current);
        self.left_at.store(seq, Ordering::SeqCst);
        *current = None;
        if let Some(watch) = &*lock(&self.watch) {
            watch.doc(None);
        }
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
    /// are gone. At most once per index generation, only once the window has shown its first
    /// document, and only for a document that follows the index (`follows_the_index`).
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
                        && follows_the_index(&c.path)
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
    /// in a document the index already covers. A document that failed to open is left alone, and
    /// so is one that doesn't follow the index (`follows_the_index`).
    pub(super) fn refresh_current_now(&self) {
        if !self.ui_shown.is_open() {
            return;
        }
        let gen = self.index_gen.load(Ordering::SeqCst);
        let path = lock(&self.current)
            .as_mut()
            .filter(|c| c.doc.is_some() && follows_the_index(&c.path))
            .map(|c| {
                c.refreshed_at = Some(gen);
                c.path.clone()
            });
        if let Some(path) = path {
            self.emit(UiEvent::DocChanged(path));
        }
    }

    /// The open numbered `seq` is on screen, with the page or PDF open `token` (none for any other
    /// document): that open is kept however many come after it. Under the lock that made it
    /// current, so a newer open's pin is never undone by an older one's.
    fn pin_open(&self, seq: u64, token: Option<&str>) {
        let current = lock(&self.current);
        if current.as_ref().is_some_and(|c| c.seq == seq) {
            lock(&self.app.page_opens).pin(&self.label, token);
        }
    }

    /// Asks the UI to load the HTML page on screen again, once the window has shown it: the *HTML
    /// pages* setting changed, and a page already loaded keeps the policy it was served under. A
    /// PDF, which the setting doesn't touch, a note, and a page that failed to open are left alone.
    pub(super) fn reload_page(&self) {
        if !self.ui_shown.is_open() {
            return;
        }
        let path = lock(&self.current)
            .as_ref()
            .filter(|c| c.doc.is_some() && opened_kind(&c.path) == DocKind::Html)
            .map(|c| c.path.clone());
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

/// The folder of `page`, a normalised path (`X:\a.html`, `\\server\share\a.html`): a drive's
/// root keeps its separator.
fn folder_of(page: &str) -> &str {
    match page.rsplit_once('\\') {
        Some((folder, _)) if folder.ends_with(':') => &page[..=folder.len()],
        Some((folder, _)) => folder,
        None => page,
    }
}

/// Whether the document at `path` shows anything the index or the path mappings decide: a note's
/// links and images do. A page, a PDF or a text file shows the same whatever they hold, so it is
/// never reloaded for them.
fn follows_the_index(path: &Path) -> bool {
    opened_kind(path) == DocKind::Markdown
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
    use lectern_core::library::pathmap::{PathMapper, PAGE_ORIGIN};
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

    /// The path of an open's frame URL, and the token in it: `/o/<token>/<file name>`.
    fn open_url(url: &str) -> (&str, &str) {
        let path = url.strip_prefix(PAGE_ORIGIN).expect("on the page origin");
        let token = path
            .strip_prefix("/o/")
            .and_then(|rest| rest.split_once('/'))
            .map(|(token, _)| token)
            .expect("an open's URL");
        (path, token)
    }

    /// An HTML page or a PDF opens with its kind, nothing of Lectern's own to show, and a frame URL
    /// naming an open of its own, which serves it; a note opens as it always has.
    #[test]
    fn a_page_opens_with_its_kind_and_a_frame_url() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("one/report.html", "<h1>Report</h1><p>Rose.</p>");
        let pdf = f.dir.file("one/brief.pdf", "%PDF-1.4\n");
        let note = f.dir.file("one/plan.md", "# Plan\n");
        let mut tokens = Vec::new();
        for (path, kind) in [(&page, DocKind::Html), (&pdf, DocKind::Pdf)] {
            let OpenResult::Ok { doc } = f.state.open_document(&path_string(path)) else {
                panic!("{} didn't open", path.display());
            };
            assert_eq!(doc.kind, kind);
            let url = doc.frame_url.expect("a frame URL");
            let (file, token) = open_url(&url);
            let name = path.file_name().unwrap().to_string_lossy();
            assert_eq!(file, format!("/o/{token}/{name}"));
            tokens.push(token.to_owned());
            let served = f.app.serve_page(file);
            assert_eq!(served.status, 200, "{url}");
            assert!(served.body.ends_with(&fs::read(path).unwrap()), "{url}");
            assert_eq!(doc.html, "");
            assert!(doc.outline.is_empty());
            assert!(doc.frontmatter.is_none());
            assert_eq!(doc.title, name);
        }
        assert_ne!(tokens[0], tokens[1]);
        let OpenResult::Ok { doc } = f.state.open_document(&path_string(&note)) else {
            panic!("the note didn't open");
        };
        assert_eq!(doc.kind, DocKind::Markdown);
        assert_eq!(doc.frame_url, None);
        assert!(doc.html.contains("Plan"));
        assert_eq!(last_doc(&f), Some(path_string(&note)));
    }

    /// A page or a PDF opened through `..` shows from the folder it is in, which its open reaches.
    #[test]
    fn a_page_opened_through_dot_dot_shows_from_where_it_is() {
        let f = fixture(profile(&[]), FakeHost::default());
        let pdf = f.dir.file("one/brief.pdf", "%PDF-1.4\n");
        f.dir.file("one/style.css", "p {}");
        f.dir.folder("one/sub");
        let through = pdf.with_file_name("sub").join("..").join("brief.pdf");
        let OpenResult::Ok { doc } = f.state.open_document(&path_string(&through)) else {
            panic!("{} didn't open", through.display());
        };
        let url = doc.frame_url.expect("a frame URL");
        let (file, token) = open_url(&url);
        assert_eq!(file, format!("/o/{token}/brief.pdf"));
        assert_eq!(f.app.serve_page(file).status, 200, "{url}");
        assert_eq!(
            f.app.serve_page(&format!("/o/{token}/style.css")).status,
            200
        );
    }

    /// A page or a PDF at a path the page protocol can't name is an open error, never an empty
    /// pane.
    #[test]
    fn a_page_without_a_frame_url_is_an_open_error() {
        let f = fixture(profile(&[]), FakeHost::default());
        let pdf = f.dir.file("one/brief.pdf", "%PDF-1.4\n");
        let verbatim = PathBuf::from(format!(r"\\?\{}", pdf.display()).replace('/', r"\"));
        assert!(fs::metadata(&verbatim).is_ok(), "{}", verbatim.display());
        for result in [
            f.state.load(&verbatim),
            render_file(&verbatim, &PathMapper::default(), &[]),
        ] {
            let Err(error) = result else {
                panic!("{} loaded", verbatim.display());
            };
            assert!(matches!(error.kind, OpenErrorKind::Io), "{error:?}");
        }
    }

    /// A PDF over 100 MiB or a page over 20 MiB, the page protocol's caps, doesn't open, at boot or
    /// later; one at the cap does.
    #[test]
    fn a_pdf_or_a_page_over_its_cap_is_too_large() {
        const MIB: u64 = 1024 * 1024;
        let f = fixture(profile(&[]), FakeHost::default());
        let sized = |rel: &str, len: u64| {
            let path = f.dir.file(rel, "");
            fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_len(len))
                .unwrap();
            path
        };
        let pdf = "This PDF is too large to show (over 100 MB).";
        let page = "This page is too large to show (over 20 MB).";
        for (path, message) in [
            (sized("big/brief.pdf", 100 * MIB + 1), pdf),
            (sized("big/report.html", 20 * MIB + 1), page),
            (sized("big/old.HTM", 20 * MIB + 1), page),
        ] {
            let OpenResult::Err { error } = f.state.open_document(&path_string(&path)) else {
                panic!("{} opened", path.display());
            };
            assert!(matches!(error.kind, OpenErrorKind::TooLarge), "{error:?}");
            assert_eq!(error.message, message);
            let Err(error) = render_file(&path, &PathMapper::default(), &[]) else {
                panic!("{} rendered at boot", path.display());
            };
            assert_eq!(error.message, message);
        }
        for path in [
            sized("ok/brief.pdf", 100 * MIB),
            sized("ok/report.html", 20 * MIB),
        ] {
            let opened = f.state.open_document(&path_string(&path));
            assert!(
                matches!(opened, OpenResult::Ok { .. }),
                "{}",
                path.display()
            );
        }
    }

    /// A text file opens as written, in paragraphs, never as a note: read fresh and from the
    /// cache alike.
    #[test]
    fn a_text_file_opens_as_its_paragraphs() {
        let f = fixture(profile(&[]), FakeHost::default());
        let text = f
            .dir
            .file("one/readme.txt", "# Not a heading\n\n<b>bold</b>\n");
        for _ in 0..2 {
            let OpenResult::Ok { doc } = f.state.open_document(&path_string(&text)) else {
                panic!("the text file didn't open");
            };
            assert_eq!(doc.kind, DocKind::Text);
            assert_eq!(
                doc.html,
                concat!(
                    "<p class=\"lx-plain\" data-sourcepos=\"1:1-1:15\"># Not a heading</p>\n",
                    "<p class=\"lx-plain\" data-sourcepos=\"3:1-3:11\">&lt;b&gt;bold&lt;/b&gt;</p>\n",
                )
            );
            assert!(doc.outline.is_empty());
            assert_eq!(doc.title, "readme.txt");
        }
    }

    /// Nothing in a page depends on the index, so it is never stale, and a rescan that moves
    /// files never reloads it.
    #[test]
    fn a_page_is_never_refreshed_for_the_index() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let outside = dir.file("elsewhere/report.html", "<h1>Report</h1>");
        let page = dir.file("vault/report.html", "<h1>Report</h1>");
        let b = dir.file("vault/work/b.md", "# B");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        wait_until("the root is indexed", || settled(&f, &root));
        f.state.window_shown();
        f.state.open_document(&path_string(&outside));
        assert_eq!(current(&f), (outside, false));
        f.state.open_document(&path_string(&page));
        assert_eq!(current(&f), (page, false));
        fs::create_dir_all(root.join("archive")).unwrap();
        fs::rename(&b, root.join("archive").join("b.md")).unwrap();
        rescan(&f, &root);
        f.state.refresh_current_now();
        assert_eq!(f.host.doc_changes(), 0);
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
