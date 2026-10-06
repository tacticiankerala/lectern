//! Walking a root, reading frontmatter heads, and probing a root that may be offline.
//!
//! Library roots can live on a NAS that stalls for seconds at a time, so the walk and the head
//! reads run on thread pools of their own rather than rayon's global pool, which rendering uses.
//! A stalled share then never holds up highlighting.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jwalk::{DirEntry, Parallelism, WalkDirGeneric};
use rayon::prelude::*;

use super::ignore::is_ignored;
use super::{is_markdown, join_rel, FileEntry, RootIndex};
use crate::frontmatter::{parse_frontmatter, Frontmatter, PropValue};
use crate::review::{self, format};

/// How much of each Markdown file `read_heads` reads.
const HEAD_BYTES: usize = 4096;
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// The file cap for an ad-hoc root: the folder of a document opened outside every library root,
/// which may be any folder at all (Downloads, a drive's root).
pub const ADHOC_MAX_FILES: usize = 20_000;
/// The file cap for a library root the user added: only a safety net. Every extension is indexed,
/// so a root holding code needs room well past its notes.
pub const ROOT_MAX_FILES: usize = 200_000;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// The walk stops after recording this many files, and marks the index truncated.
    pub max_files: usize,
}

impl ScanOptions {
    /// The options for a root: the ad-hoc cap for an ad-hoc root, else the library root cap.
    pub fn for_root(adhoc: bool) -> Self {
        let max_files = if adhoc {
            ADHOC_MAX_FILES
        } else {
            ROOT_MAX_FILES
        };
        Self { max_files }
    }
}

/// A library root's options.
impl Default for ScanOptions {
    fn default() -> Self {
        Self::for_root(false)
    }
}

/// Size and modification time, taken on the walk's worker threads.
#[derive(Debug, Clone, Copy)]
struct Stat {
    mtime_ms: i64,
    size: u64,
}

/// jwalk client state: nothing per directory, a `Stat` per regular file.
type Walk = WalkDirGeneric<((), Option<Stat>)>;
type WalkEntry = DirEntry<((), Option<Stat>)>;

/// Walks `root` and records every file that isn't ignored, up to `opts.max_files`; reaching the cap
/// marks the index `truncated`. Reads no file contents. Fails when the root is missing, isn't a
/// directory or can't be listed; errors below the root (an unreadable folder, a file deleted
/// mid-walk) skip that entry.
pub fn scan_root(root: &Path, opts: &ScanOptions) -> io::Result<RootIndex> {
    // jwalk's own read of the root is the only one, so a root that vanishes or disconnects is
    // reported by the walk itself rather than slipping in after a separate check.
    let walk = Walk::new(root)
        .skip_hidden(false)
        .sort(true)
        .parallelism(Parallelism::RayonNewPool(0))
        .process_read_dir(|depth, _dir, _state, children| {
            // `None` is the root entry itself, which is never filtered.
            if depth.is_none() {
                return;
            }
            children.retain(|child| {
                child.as_ref().is_ok_and(|e| {
                    !is_ignored(&e.file_name.to_string_lossy(), e.file_type.is_dir())
                })
            });
            for e in children.iter_mut().flatten() {
                if !e.file_type.is_dir() {
                    e.client_state = stat(&e.path());
                }
            }
        });
    let entries = walk.into_iter().map(|entry| file_of(root, entry));
    let (files, truncated) = collect_files(entries, opts.max_files)?;
    let mut index = RootIndex::new(root.to_path_buf(), files, unix_millis(SystemTime::now()));
    index.truncated = truncated;
    Ok(index)
}

/// The file an entry records, `None` for the root, directories and skipped entries. Errors for
/// the root itself fail the scan; errors below it skip the entry.
fn file_of(root: &Path, entry: jwalk::Result<WalkEntry>) -> io::Result<Option<FileEntry>> {
    let entry = match entry {
        Ok(entry) => entry,
        // Only the root itself fails at depth 0 (missing, disconnected or a broken link); the
        // walk's pool is its own, so jwalk never reports a busy thread pool.
        Err(e) if e.depth() == 0 => return Err(io_error(&e)),
        Err(_) => return Ok(None),
    };
    if entry.depth == 0 {
        return match &entry.read_children {
            None => Err(not_a_folder(root)),
            Some(children) => children.error().map_or(Ok(None), |e| Err(io_error(e))),
        };
    }
    // Directories, symlinks to directories, and files that couldn't be read have no `Stat`. A
    // name that isn't valid Unicode can't be addressed by a `/`-separated `rel`.
    let (Some(stat), Some(rel)) = (entry.client_state, rel_path(root, &entry)) else {
        return Ok(None);
    };
    Ok(Some(FileEntry {
        is_md: is_markdown(&rel),
        rel,
        mtime_ms: stat.mtime_ms,
        size: stat.size,
        fm_name: None,
        fm_status: None,
        review_of: None,
        review_open: None,
        is_sidecar: false,
    }))
}

/// Gathers files until `max_files`, pulling nothing more once the cap is reached. Dropping the
/// walk then stops jwalk's workers, which may have read only a few folders ahead. True alongside
/// the files when the cap was reached: there may be more that weren't recorded.
fn collect_files(
    entries: impl IntoIterator<Item = io::Result<Option<FileEntry>>>,
    max_files: usize,
) -> io::Result<(Vec<FileEntry>, bool)> {
    let mut files = Vec::new();
    for entry in entries {
        files.extend(entry?);
        if files.len() >= max_files {
            return Ok((files, true));
        }
    }
    Ok((files, false))
}

fn io_error(e: &jwalk::Error) -> io::Error {
    let kind = e.io_error().map_or(io::ErrorKind::Other, io::Error::kind);
    io::Error::new(kind, e.to_string())
}

fn not_a_folder(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotADirectory,
        format!("{} is not a folder", path.display()),
    )
}

/// Metadata for a regular file, following symlinks. `None` for directories and unreadable entries.
fn stat(path: &Path) -> Option<Stat> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then(|| Stat {
        mtime_ms: meta.modified().map_or(0, unix_millis),
        size: meta.len(),
    })
}

/// The entry's path below `root`, `/`-separated.
fn rel_path(root: &Path, entry: &WalkEntry) -> Option<String> {
    let mut rel = String::new();
    for part in entry.parent_path().strip_prefix(root).ok()?.components() {
        rel.push_str(part.as_os_str().to_str()?);
        rel.push('/');
    }
    rel.push_str(entry.file_name.to_str()?);
    Some(rel)
}

fn unix_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// Fills `fm_name` and `fm_status` for every Markdown file from the frontmatter in its first
/// 4 KiB, and `review_of` and `review_open` for review sidecars, in parallel, then refreshes the
/// lookup maps. A file that can't be read keeps what it had; a NAS can fail single reads.
pub fn read_heads(root: &mut RootIndex) {
    let base = root.root.clone();
    let read = |file: &mut FileEntry| {
        if !file.is_md {
            return;
        }
        let path = join_rel(&base, &file.rel);
        if let Ok(head) = read_head(&path) {
            let (name, status) = head_fields(&head);
            file.fm_name = name;
            file.fm_status = status;
            (file.review_of, file.review_open) = review_fields(&path, &file.rel, &head);
        }
    };
    match rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("lectern-heads-{i}"))
        .build()
    {
        Ok(pool) => pool.install(|| root.files.par_iter_mut().for_each(read)),
        Err(_) => root.files.iter_mut().for_each(read),
    }
    root.heads_read = true;
    root.finalize();
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD_BYTES);
    File::open(path)?
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(head)
}

/// For a file named like a review sidecar whose head has sidecar frontmatter: the note it names,
/// and its open comments. Counting reads the whole file, unless the head already holds it, and
/// gives `None` for one over `MAX_SIDECAR_BYTES` or that can't be read.
fn review_fields(path: &Path, rel: &str, head: &[u8]) -> (Option<String>, Option<u32>) {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if !review::is_sidecar_name(name) {
        return (None, None);
    }
    let Some(note) = review::sidecar_note(&String::from_utf8_lossy(head)) else {
        return (None, None);
    };
    let open = if head.len() < HEAD_BYTES {
        Some(open_comments(head))
    } else {
        read_sidecar(path).map(|text| open_comments(&text))
    };
    (Some(note), open)
}

/// The whole sidecar at `path`, `None` when it's over `MAX_SIDECAR_BYTES` or can't be read.
fn read_sidecar(path: &Path) -> Option<Vec<u8>> {
    let mut text = Vec::new();
    File::open(path)
        .ok()?
        .take(review::MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut text)
        .ok()?;
    let len = u64::try_from(text.len()).unwrap_or(u64::MAX);
    (len <= review::MAX_SIDECAR_BYTES).then_some(text)
}

fn open_comments(text: &[u8]) -> u32 {
    format::parse(&String::from_utf8_lossy(text)).open_count()
}

/// `name:` and `status:` from the frontmatter at the start of `head`.
fn head_fields(head: &[u8]) -> (Option<String>, Option<String>) {
    let complete = head.len() < HEAD_BYTES;
    let Some(yaml) = frontmatter_yaml(head, complete) else {
        return (None, None);
    };
    let Frontmatter::Parsed { entries } = parse_frontmatter(&String::from_utf8_lossy(yaml)) else {
        return (None, None);
    };
    let field = |key: &str| {
        entries
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| scalar_text(&p.value))
    };
    (field("name"), field("status"))
}

/// The YAML between a `---` line at the very start (after an optional BOM) and the next `---`
/// line. `None` without frontmatter, or when the closing line isn't within `head`: a truncated
/// block is never parsed. `complete` says `head` holds the whole file, so a closing `---` at the
/// end needs no line break.
pub(crate) fn frontmatter_yaml(head: &[u8], complete: bool) -> Option<&[u8]> {
    let head = head.strip_prefix(UTF8_BOM).unwrap_or(head);
    let body = head
        .strip_prefix(b"---\n")
        .or_else(|| head.strip_prefix(b"---\r\n"))?;
    let mut start = 0;
    while start < body.len() {
        let (line, next, ended) = match body[start..].iter().position(|&b| b == b'\n') {
            Some(i) => (&body[start..start + i], start + i + 1, true),
            None => (&body[start..], body.len(), complete),
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line == b"---" && ended {
            return Some(&body[..start]);
        }
        start = next;
    }
    None
}

fn scalar_text(value: &PropValue) -> Option<String> {
    match value {
        PropValue::Text(s) | PropValue::Date(s) | PropValue::DateTime(s) | PropValue::Number(s) => {
            (!s.is_empty()).then(|| s.clone())
        }
        PropValue::Bool(b) => Some(b.to_string()),
        PropValue::List(_) => None,
    }
}

/// Checks that `root` is a folder that can be listed, giving up after `timeout`.
pub fn probe_root(root: &Path, timeout: Duration) -> Result<(), String> {
    let root = root.to_path_buf();
    probe_with(
        move || {
            if !fs::metadata(&root)?.is_dir() {
                return Err(not_a_folder(&root));
            }
            fs::read_dir(&root)?.next().transpose()?;
            Ok(())
        },
        timeout,
    )
}

/// Runs `f` on its own thread and waits at most `timeout` for it. A call that hangs (a stalled
/// SMB share) is left running detached; the caller gets an error straight away.
pub fn probe_with<F>(f: F, timeout: Duration) -> Result<(), String>
where
    F: FnOnce() -> io::Result<()> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("lectern-probe".to_owned())
        .spawn(move || {
            // The receiver is gone when the probe already timed out.
            let _ = tx.send(f());
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(RecvTimeoutError::Timeout) => {
            Err(format!("no response within {} ms", timeout.as_millis()))
        }
        Err(RecvTimeoutError::Disconnected) => Err("the probe failed".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel: &str) -> FileEntry {
        FileEntry {
            rel: rel.to_owned(),
            is_md: true,
            mtime_ms: 0,
            size: 0,
            fm_name: None,
            fm_status: None,
            review_of: None,
            review_open: None,
            is_sidecar: false,
        }
    }

    /// A walk that yields the root, then `files`, and panics if pulled any further.
    fn walk_of(files: &[&str]) -> impl Iterator<Item = io::Result<Option<FileEntry>>> {
        let root = std::iter::once(Ok(None));
        let files: Vec<_> = files.iter().map(|rel| Ok(Some(file(rel)))).collect();
        root.chain(files)
            .chain(std::iter::from_fn(|| panic!("pulled past the cap")))
    }

    #[test]
    fn collect_stops_pulling_once_the_cap_is_reached() {
        let (files, truncated) = collect_files(walk_of(&["a.md"]), 1).unwrap();
        assert_eq!((files.len(), truncated), (1, true));
        let (files, truncated) = collect_files(walk_of(&["a.md", "b.md", "c.md"]), 3).unwrap();
        assert_eq!((files.len(), truncated), (3, true));
    }

    #[test]
    fn collect_with_no_room_stops_after_the_root() {
        let (files, truncated) = collect_files(walk_of(&[]), 0).unwrap();
        assert!(files.is_empty());
        assert!(truncated);
    }

    #[test]
    fn collect_below_the_cap_is_not_truncated() {
        let entries = [Ok(None), Ok(Some(file("a.md"))), Ok(Some(file("b.md")))];
        let (files, truncated) = collect_files(entries, 3).unwrap();
        assert_eq!((files.len(), truncated), (2, false));
    }

    #[test]
    fn collect_fails_on_a_root_error_and_keeps_skipped_entries_out() {
        let lost = io::Error::new(io::ErrorKind::NotFound, "gone");
        let err = collect_files([Err(lost)], 10).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let (files, _) = collect_files([Ok(None), Ok(Some(file("a.md"))), Ok(None)], 10).unwrap();
        assert_eq!(files.len(), 1);
    }
}
