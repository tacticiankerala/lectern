//! The library: roots, the file index and path resolution.

pub mod ignore;
pub mod pathmap;
pub mod scan;
pub mod snapshot;
pub mod tree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One file under a library root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// The path relative to the root, always `/`-separated.
    pub rel: String,
    /// A `.md` or `.markdown` file.
    pub is_md: bool,
    /// Last modified, in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    pub size: u64,
    /// The frontmatter `name:`, filled in by `scan::read_heads`.
    pub fm_name: Option<String>,
    /// The frontmatter `status:`, filled in by `scan::read_heads`.
    pub fm_status: Option<String>,
}

/// Every file under one library root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootIndex {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    /// When the walk ran, in milliseconds since the Unix epoch.
    pub scanned_at_ms: i64,
    /// Built by `finalize`; never persisted.
    #[serde(skip)]
    lookup: Lookup,
}

/// Case-insensitive indexes into `RootIndex::files`.
#[derive(Debug, Clone, Default)]
struct Lookup {
    by_rel: HashMap<String, usize>,
    /// Markdown files only.
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

    /// Rebuilds the lookup maps from `files`. Call after changing `files` or their heads.
    pub fn finalize(&mut self) {
        let mut lookup = Lookup::default();
        for (idx, file) in self.files.iter().enumerate() {
            lookup.by_rel.entry(rel_key(&file.rel)).or_insert(idx);
            if !file.is_md {
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

    pub fn md_files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.iter().filter(|f| f.is_md)
    }
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
pub(crate) fn path_key(path: &Path) -> String {
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
fn key_under<'a>(key: &'a str, root: &str) -> Option<&'a str> {
    if key == root {
        return Some("");
    }
    key.strip_prefix(root)?.strip_prefix('/')
}

fn rel_key(rel: &str) -> String {
    path_key(Path::new(rel))
}
