//! The library: roots, the file index and path resolution.

pub mod ignore;
pub mod pathmap;
pub mod resolve;
pub mod scan;
pub mod snapshot;
pub mod tree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::review;

/// The extensions of the files Lectern treats as Markdown notes, the ones its installer
/// registers. Scanning, link routing and the app's file kinds read this list, and the UI's file
/// picker gets it from `ui/src/generated/markdown-extensions.ts`, which the core tests write.
pub const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

/// Whether `path` names a Markdown file: its extension is one of `MARKDOWN_EXTENSIONS`, in any
/// letter case.
pub fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|md| ext.eq_ignore_ascii_case(md))
        })
}

/// One file under a library root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// The path relative to the root, always `/`-separated.
    pub rel: String,
    /// A Markdown file (see `MARKDOWN_EXTENSIONS`).
    pub is_md: bool,
    /// Last modified, in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    pub size: u64,
    /// The frontmatter `name:`, filled in by `scan::read_heads`.
    pub fm_name: Option<String>,
    /// The frontmatter `status:`, filled in by `scan::read_heads`.
    pub fm_status: Option<String>,
    /// For a file named like a review sidecar (`*.review.md`) with sidecar frontmatter: the file
    /// name of the note it names, filled in by `scan::read_heads`.
    #[serde(default)]
    pub review_of: Option<String>,
    /// For a file with `review_of`: its comments still open. `None` when it's too large to count.
    #[serde(default)]
    pub review_open: Option<u32>,
    /// The file is the review sidecar of a note beside it, so it isn't a note itself. Set by
    /// `finalize`.
    #[serde(skip)]
    pub is_sidecar: bool,
}

/// Every file under one library root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootIndex {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    /// When the walk ran, in milliseconds since the Unix epoch.
    pub scanned_at_ms: i64,
    /// The walk stopped at its file cap, so files past it are missing from the index.
    #[serde(default)]
    pub truncated: bool,
    /// `scan::read_heads` has filled in the frontmatter fields, so sidecars are known by their
    /// frontmatter. Until then, `finalize` tells them by their names alone. False for a fresh
    /// walk and for a snapshot saved before Lectern had sidecars.
    #[serde(default)]
    pub heads_read: bool,
    /// Built by `finalize`; never persisted.
    #[serde(skip)]
    lookup: Lookup,
}

/// Case-insensitive indexes into `RootIndex::files`.
#[derive(Debug, Clone, Default)]
struct Lookup {
    by_rel: HashMap<String, usize>,
    /// Notes only (see `md_files`), here and in `by_fm_name`.
    by_stem: HashMap<String, Vec<usize>>,
    by_fm_name: HashMap<String, Vec<usize>>,
}

/// All library roots.
#[derive(Debug, Default, Clone)]
pub struct LibraryIndex {
    pub roots: Vec<RootIndex>,
}

impl RootIndex {
    /// An index of `files` under `root`, finalized and ready for lookups.
    pub fn new(root: PathBuf, files: Vec<FileEntry>, scanned_at_ms: i64) -> Self {
        let mut index = Self {
            root,
            files,
            scanned_at_ms,
            truncated: false,
            heads_read: false,
            lookup: Lookup::default(),
        };
        index.finalize();
        index
    }

    /// The absolute path of `rel`, joined with the platform's separators.
    pub fn abs(&self, rel: &str) -> PathBuf {
        join_rel(&self.root, rel)
    }

    /// The file at `rel`, compared case-insensitively. Needs `finalize` after `files` changes.
    pub fn get(&self, rel: &str) -> Option<&FileEntry> {
        let idx = *self.lookup.by_rel.get(&rel_key(rel))?;
        self.files.get(idx)
    }

    /// Markdown files whose name without the extension is `stem`, compared case-insensitively.
    pub fn with_stem<'a>(&'a self, stem: &str) -> impl Iterator<Item = &'a FileEntry> + 'a {
        self.indexed(self.lookup.by_stem.get(&stem.to_lowercase()))
    }

    /// Files whose frontmatter `name:` is `name`, compared case-insensitively.
    pub fn with_fm_name<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a FileEntry> + 'a {
        self.indexed(self.lookup.by_fm_name.get(&name.to_lowercase()))
    }

    fn indexed<'a>(&'a self, idxs: Option<&'a Vec<usize>>) -> impl Iterator<Item = &'a FileEntry> {
        idxs.into_iter()
            .flatten()
            .filter_map(|&idx| self.files.get(idx))
    }

    /// Marks the sidecars and rebuilds the lookup maps from `files`. Call after changing `files`
    /// or their heads. Until the heads are read, a file named as the sidecar of a Markdown note
    /// beside it is taken for one, so a sidecar never shows while a slow share is being read.
    pub fn finalize(&mut self) {
        let mut lookup = Lookup::default();
        for (idx, file) in self.files.iter().enumerate() {
            lookup.by_rel.entry(rel_key(&file.rel)).or_insert(idx);
        }
        let classify = if self.heads_read {
            is_sidecar
        } else {
            named_as_sidecar
        };
        let sidecars: Vec<bool> = self
            .files
            .iter()
            .map(|file| classify(file, &self.files, &lookup.by_rel))
            .collect();
        for (file, sidecar) in self.files.iter_mut().zip(sidecars) {
            file.is_sidecar = sidecar;
        }
        for (idx, file) in self.files.iter().enumerate() {
            if !file.is_md || file.is_sidecar {
                continue;
            }
            if let Some(stem) = Path::new(&file.rel).file_stem() {
                let stem = stem.to_string_lossy().to_lowercase();
                lookup.by_stem.entry(stem).or_default().push(idx);
            }
            if let Some(name) = &file.fm_name {
                lookup
                    .by_fm_name
                    .entry(name.to_lowercase())
                    .or_default()
                    .push(idx);
            }
        }
        self.lookup = lookup;
    }

    /// The notes: Markdown files that aren't review sidecars.
    pub fn md_files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.iter().filter(|f| f.is_md && !f.is_sidecar)
    }

    /// The open comments in the review sidecar of the note at `note_rel`. `None` when the note has
    /// no sidecar, or it was too large to count.
    pub fn comment_count(&self, note_rel: &str) -> Option<u32> {
        let (dir, note) = split_rel(note_rel);
        let sidecar = self.get(&sibling_rel(dir, &sidecar_name(note)))?;
        let names_note = sidecar
            .review_of
            .as_deref()
            .is_some_and(|of| same_name(of, note));
        if sidecar.is_sidecar && names_note {
            sidecar.review_open
        } else {
            None
        }
    }
}

/// Whether `file` is the review sidecar of a note beside it: its frontmatter names a Markdown file
/// in its own folder that the index holds, and that file's sidecar name is this file's name. A
/// file that is merely named like a sidecar (`code.review.md`) stays a note.
fn is_sidecar(file: &FileEntry, files: &[FileEntry], by_rel: &HashMap<String, usize>) -> bool {
    let Some(note) = file.review_of.as_deref() else {
        return false;
    };
    // The note is a file name, so it can't lead out of the sidecar's folder.
    if note.is_empty() || note.contains(['/', '\\']) {
        return false;
    }
    let (dir, name) = split_rel(&file.rel);
    let note_is_md = sibling(files, by_rel, dir, note).is_some_and(|note| note.is_md);
    note_is_md && same_name(&sidecar_name(note), name)
}

/// Whether `file` is named as the sidecar of a Markdown note beside it, frontmatter unseen: what
/// stands in for `is_sidecar` until the heads are read. An ordinary `code.review.md` beside a
/// `code.md` is taken for a sidecar until then.
fn named_as_sidecar(
    file: &FileEntry,
    files: &[FileEntry],
    by_rel: &HashMap<String, usize>,
) -> bool {
    let (dir, name) = split_rel(&file.rel);
    if !review::is_sidecar_name(name) {
        return false;
    }
    // The suffix is ASCII, so this cut falls on a character boundary.
    let base = &name[..name.len() - review::SIDECAR_SUFFIX.len()];
    // `<base>.review.md` is the sidecar name of `<base>.md`, and of `<base>` itself when that is a
    // Markdown file with another extension (`x.markdown`).
    [format!("{base}.md"), base.to_owned()]
        .iter()
        .any(|candidate| {
            sibling(files, by_rel, dir, candidate).is_some_and(|note| {
                let (_, note_name) = split_rel(&note.rel);
                note.is_md && same_name(&sidecar_name(note_name), name)
            })
        })
}

/// The file called `name` in the folder `dir`, compared case-insensitively.
fn sibling<'a>(
    files: &'a [FileEntry],
    by_rel: &HashMap<String, usize>,
    dir: &str,
    name: &str,
) -> Option<&'a FileEntry> {
    let idx = *by_rel.get(&rel_key(&sibling_rel(dir, name)))?;
    files.get(idx)
}

/// The folder part and the file name of the `/`-separated `rel`; the folder is empty at the root.
fn split_rel(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

/// The `/`-separated path of the file called `name` in the folder `dir`.
fn sibling_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The file name of the review sidecar of the note called `note`.
fn sidecar_name(note: &str) -> String {
    review::sidecar_path(Path::new(note))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// File names compared ignoring case, as Windows compares them and as `review::store` compares a
/// sidecar's `note:` with its note.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

impl LibraryIndex {
    /// The root holding `abs`; the deepest one when roots are nested. Case-insensitive.
    pub fn root_for(&self, abs: &Path) -> Option<&RootIndex> {
        let key = path_key(abs);
        self.roots
            .iter()
            .filter_map(|r| {
                let root = path_key(&r.root);
                key_under(&key, &root).map(|_| (root.len(), r))
            })
            .max_by_key(|&(root_len, _)| root_len)
            .map(|(_, r)| r)
    }

    /// Whether `abs` is an indexed file in any root that holds it. Nested roots are all checked,
    /// because a capped or stale inner index can miss a file its parent has.
    pub fn contains(&self, abs: &Path) -> bool {
        let key = path_key(abs);
        self.roots
            .iter()
            .any(|r| key_under(&key, &path_key(&r.root)).is_some_and(|rel| r.get(rel).is_some()))
    }

    /// Adds `r`, replacing any root with the same path (compared case-insensitively).
    pub fn upsert_root(&mut self, r: RootIndex) {
        let key = path_key(&r.root);
        match self.roots.iter_mut().find(|old| path_key(&old.root) == key) {
            Some(old) => *old = r,
            None => self.roots.push(r),
        }
    }
}

/// A path as a comparison key: lowercase, `/`-separated, without a trailing separator. Lectern
/// runs on Windows, so paths compare case-insensitively on every platform.
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .to_lowercase()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

/// `root` joined with the `/`-separated `rel`, using the platform's separators.
pub(crate) fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|c| !c.is_empty()));
    path
}

/// The part of path key `key` below path key `root`: empty for the root itself, `None` outside it.
pub(crate) fn key_under<'a>(key: &'a str, root: &str) -> Option<&'a str> {
    if key == root {
        return Some("");
    }
    key.strip_prefix(root)?.strip_prefix('/')
}

fn rel_key(rel: &str) -> String {
    path_key(Path::new(rel))
}
