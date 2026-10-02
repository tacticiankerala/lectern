//! Full-text search across the library (Ctrl+Shift+F): smart-case substring search over the text
//! of every Markdown file.
//!
//! File contents are cached by path and checked against a fresh stat, so only new and changed
//! files are read again. A search within 3 s of the last full check trusts the cache without
//! statting, so a burst of keystrokes costs one pass over the files. Reads run on a small thread
//! pool of the cache's own: a NAS that stalls holds up that pool, never rayon's global one, which
//! rendering uses.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::frontmatter::{parse_frontmatter, Frontmatter, PropValue};
use crate::library::scan::frontmatter_yaml;
use crate::library::{path_key, LibraryIndex};
use crate::text::decode;

/// Matching lines returned for one file.
const MAX_HITS_PER_FILE: usize = 5;
/// Matching lines returned for one search.
const MAX_HITS: usize = 500;
/// Characters of context kept on each side of a line's first hit.
const CONTEXT_CHARS: usize = 60;
const ELLIPSIS: &str = "…";
/// The most threads reading files for a search. Reads mostly wait on the disk or the network.
const MAX_THREADS: usize = 8;
/// Larger Markdown files are not searched, nor read.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// How long after a search has checked every file's stamp the cache is trusted without a stat.
const VALIDATION_WINDOW: Duration = Duration::from_secs(3);

/// A run of snippet text, marked when it is a hit.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Segment {
    pub text: String,
    pub hit: bool,
}

/// One matching line.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    /// 1-based.
    pub line: u32,
    /// Up to 60 characters either side of the line's first hit, cut at character boundaries, with
    /// `…` where the line goes on. Every hit inside is marked; hits and the text between them
    /// alternate.
    pub segments: Vec<Segment>,
}

/// The matching lines of one file.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileHits {
    /// The absolute path.
    pub path: String,
    /// The frontmatter `title`, else the first `# ` heading, else the file name without its
    /// extension.
    pub title: String,
    /// The path below its root, `/`-separated.
    pub rel: String,
    /// The file name holds the query; for a README, its folder's name does.
    pub name_match: bool,
    /// The first matching lines, at most 5.
    pub hits: Vec<SearchHit>,
    /// Every matching line in the file, including those not returned.
    pub total: u32,
}

/// File contents kept between searches.
pub struct ContentCache {
    entries: Mutex<HashMap<PathBuf, Cached>>,
    /// When the last search that checked every file's stamp started.
    validated: Mutex<Option<Instant>>,
    /// How long after `validated` cached files are trusted without a stat.
    window: Duration,
    /// `None` when the pool couldn't be built; files are then read one at a time.
    pool: Option<rayon::ThreadPool>,
}

struct Cached {
    stamp: Stamp,
    /// `None` for a binary file, or one too large to search.
    doc: Option<Arc<Doc>>,
}

/// What cached contents are checked against: a file is read again when either part changes. The
/// full-precision modification time catches two saves within the same millisecond.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    size: u64,
}

struct Doc {
    text: String,
    /// Worked out the first time the file appears in results.
    title: OnceLock<String>,
}

/// A Markdown file to search.
struct Candidate<'a> {
    path: PathBuf,
    rel: &'a str,
    root: &'a Path,
}

/// A file with at least one matching line.
struct Matched<'a> {
    file: &'a Candidate<'a>,
    /// `file.path` as text, for sorting and the result.
    path: String,
    doc: Arc<Doc>,
    name_match: bool,
    total: u32,
    /// The first `MAX_HITS_PER_FILE` matching lines.
    lines: Vec<LineAt>,
}

/// A matching line within a file's text.
struct LineAt {
    number: u32,
    /// The line's bytes, without its line break.
    span: Range<usize>,
}

impl Default for ContentCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentCache {
    /// A cache trusted for 3 s after each search that checks every file.
    pub fn new() -> Self {
        Self::with_validation_window(VALIDATION_WINDOW)
    }

    /// A cache trusted for `window` after each search that checks every file. With
    /// `Duration::ZERO`, every search stats every file.
    pub fn with_validation_window(window: Duration) -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(MAX_THREADS));
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("lectern-search-{i}"))
            .build()
            .ok();
        Self {
            entries: Mutex::default(),
            validated: Mutex::default(),
            window,
            pool,
        }
    }

    /// The Markdown files in `index` whose text holds `query`, best first: file name matches, then
    /// the most matching lines, then by path. At most 5 lines are returned per file and 500 in
    /// all; files past that are left out.
    ///
    /// Smart case: a query without uppercase letters ignores case. A blank query finds nothing,
    /// and so does one with a line break, since lines are matched one at a time. Files that can't
    /// be read, are binary, or are over 5 MiB are skipped.
    ///
    /// Each file's stamp (modification time and size) is checked with a fresh stat, except within
    /// the validation window of the last search that checked them all: cached files are then used
    /// as they are, and only files new to the cache are read.
    pub fn search(&self, index: &LibraryIndex, query: &str) -> Vec<FileHits> {
        let Some(re) = matcher(query) else {
            return Vec::new();
        };
        let started = Instant::now();
        let trusted = self
            .validated()
            .is_some_and(|at| started.duration_since(at) < self.window);
        let files = candidates(index);
        let mut matched: Vec<Matched> = match &self.pool {
            Some(pool) => pool.install(|| {
                files
                    .par_iter()
                    .filter_map(|file| self.find_in(file, &re, trusted))
                    .collect()
            }),
            None => files
                .iter()
                .filter_map(|file| self.find_in(file, &re, trusted))
                .collect(),
        };
        self.forget_all_but(&files);
        if !trusted {
            *self.validated() = Some(started);
        }

        matched.sort_by(|a, b| {
            b.name_match
                .cmp(&a.name_match)
                .then(b.total.cmp(&a.total))
                .then_with(|| a.path.cmp(&b.path))
        });
        let mut budget = MAX_HITS;
        let mut results = Vec::new();
        for m in matched {
            if budget == 0 {
                break;
            }
            let shown = &m.lines[..m.lines.len().min(budget)];
            budget -= shown.len();
            let hits = shown
                .iter()
                .map(|line| SearchHit {
                    line: line.number,
                    segments: snippet(&m.doc.text[line.span.clone()], &re),
                })
                .collect();
            let title = m
                .doc
                .title
                .get_or_init(|| title_of(&m.doc.text, &m.file.path));
            results.push(FileHits {
                path: m.path,
                title: title.clone(),
                rel: m.file.rel.to_owned(),
                name_match: m.name_match,
                hits,
                total: m.total,
            });
        }
        results
    }

    /// The matching lines of `file`; `None` when there are none, or it can't be read.
    fn find_in<'a>(
        &self,
        file: &'a Candidate<'a>,
        re: &Regex,
        trusted: bool,
    ) -> Option<Matched<'a>> {
        let doc = self.load(&file.path, trusted)?;
        let (total, lines) = matching_lines(&doc.text, re);
        (total > 0).then(|| Matched {
            file,
            path: file.path.to_string_lossy().into_owned(),
            name_match: name_matches(re, file.rel, file.root),
            doc,
            total,
            lines,
        })
    }

    /// The text of the file at `path`, read again only when its stamp has changed; when `trusted`,
    /// a cached file is used without a stat. `None` when it can't be read, is binary or is too
    /// large.
    fn load(&self, path: &Path, trusted: bool) -> Option<Arc<Doc>> {
        if trusted {
            if let Some(cached) = self.entries().get(path) {
                return cached.doc.clone();
            }
        }
        let meta = fs::metadata(path).ok().filter(fs::Metadata::is_file)?;
        let stamp = Stamp {
            modified: meta.modified().ok(),
            size: meta.len(),
        };
        if let Some(cached) = self.entries().get(path).filter(|c| c.stamp == stamp) {
            return cached.doc.clone();
        }
        // A file too large to search is skipped unread, and remembered like a binary one.
        let doc = if stamp.size > MAX_FILE_BYTES {
            None
        } else {
            decode(&fs::read(path).ok()?).ok().map(|decoded| {
                Arc::new(Doc {
                    text: decoded.text,
                    title: OnceLock::new(),
                })
            })
        };
        let cached = Cached {
            stamp,
            doc: doc.clone(),
        };
        self.entries().insert(path.to_owned(), cached);
        doc
    }

    /// Drops the cached text of files no longer in the library.
    fn forget_all_but(&self, files: &[Candidate]) {
        let keep: HashSet<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        self.entries()
            .retain(|path, _| keep.contains(path.as_path()));
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<PathBuf, Cached>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn validated(&self) -> MutexGuard<'_, Option<Instant>> {
        self.validated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The search for `query`; `None` for a blank query or one with a line break.
fn matcher(query: &str) -> Option<Regex> {
    if query.trim().is_empty() || query.contains(['\n', '\r']) {
        return None;
    }
    // Smart case, as in ripgrep: an uppercase letter anywhere makes the search exact. Otherwise
    // case is ignored through Unicode simple case folding, which the regex engine does in linear
    // time, matching on the original text so hit offsets need no mapping back.
    let ignore_case = !query.chars().any(char::is_uppercase);
    RegexBuilder::new(&regex::escape(query))
        .case_insensitive(ignore_case)
        .build()
        .ok()
}

/// Every Markdown file in `index`. A file under nested roots is searched once, under the deepest.
fn candidates(index: &LibraryIndex) -> Vec<Candidate<'_>> {
    let mut roots: Vec<_> = index.roots.iter().collect();
    roots.sort_by_cached_key(|r| Reverse(path_key(&r.root).len()));
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for root in roots {
        for file in root.md_files() {
            let path = root.abs(&file.rel);
            if seen.insert(path_key(&path)) {
                files.push(Candidate {
                    path,
                    rel: &file.rel,
                    root: &root.root,
                });
            }
        }
    }
    files
}

/// How many lines of `text` match, and where the first `MAX_HITS_PER_FILE` of them are. A line
/// counts once however many hits it holds.
fn matching_lines(text: &str, re: &Regex) -> (u32, Vec<LineAt>) {
    let mut total = 0u32;
    let mut lines = Vec::new();
    // `from` is always the start of a line. Line breaks are counted up to `counted`, and only
    // while lines are still being collected.
    let (mut from, mut counted, mut number) = (0, 0, 1u32);
    while let Some(hit) = re.find_at(text, from) {
        let start = text[from..hit.start()]
            .rfind('\n')
            .map_or(from, |i| from + i + 1);
        let end = text[hit.start()..]
            .find('\n')
            .map_or(text.len(), |i| hit.start() + i);
        total = total.saturating_add(1);
        if lines.len() < MAX_HITS_PER_FILE {
            number = number.saturating_add(line_breaks(&text[counted..start]));
            counted = start;
            lines.push(LineAt {
                number,
                span: start..end,
            });
        }
        if end == text.len() {
            break;
        }
        from = end + 1;
    }
    (total, lines)
}

fn line_breaks(text: &str) -> u32 {
    u32::try_from(text.bytes().filter(|&b| b == b'\n').count()).unwrap_or(u32::MAX)
}

/// The segments for one matching line: up to `CONTEXT_CHARS` characters either side of the first
/// hit, with every hit inside marked. Whitespace at either end is dropped unless a hit holds it.
fn snippet(line: &str, re: &Regex) -> Vec<Segment> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    // Not reached: the line was found by this same search.
    let Some(first) = re.find(line) else {
        return Vec::new();
    };
    let start = chars_before(line, first.start(), CONTEXT_CHARS);
    let start = first.start() - line[start..first.start()].trim_start().len();
    let mut end = chars_after(line, first.end(), CONTEXT_CHARS);
    let mut hits = vec![first.range()];
    let mut last = first.end();
    while let Some(hit) = re.find_at(line, last).filter(|h| h.start() < end) {
        // A hit that runs past the window is shown whole.
        end = end.max(hit.end());
        last = hit.end();
        hits.push(hit.range());
    }
    let end = last + line[last..end].trim_end().len();

    let mut segments = Segments::default();
    if has_text(&line[..start]) {
        segments.push(ELLIPSIS, false);
    }
    let mut at = start;
    for hit in hits {
        segments.push(&line[at..hit.start], false);
        segments.push(&line[hit.start..hit.end], true);
        at = hit.end;
    }
    segments.push(&line[at..end], false);
    if has_text(&line[end..]) {
        segments.push(ELLIPSIS, false);
    }
    segments.0
}

/// Segments that alternate: text pushed next to text of the same kind joins it, so adjacent hits
/// form one segment, and empty text is dropped.
#[derive(Default)]
struct Segments(Vec<Segment>);

impl Segments {
    fn push(&mut self, text: &str, hit: bool) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.hit == hit => last.text.push_str(text),
            _ => self.0.push(Segment {
                text: text.to_owned(),
                hit,
            }),
        }
    }
}

/// The byte index `n` characters before `at`, or 0 when there are fewer.
fn chars_before(s: &str, at: usize, n: usize) -> usize {
    s[..at]
        .char_indices()
        .rev()
        .take(n)
        .last()
        .map_or(at, |(i, _)| i)
}

/// The byte index `n` characters after `at`, or the end when there are fewer.
fn chars_after(s: &str, at: usize, n: usize) -> usize {
    s[at..]
        .char_indices()
        .nth(n)
        .map_or(s.len(), |(i, _)| at + i)
}

fn has_text(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace())
}

/// Whether the file's name without its extension holds the query, under the same smart case. A
/// README stands for its folder, as in the sidebar, so the folder's name counts for it; a README
/// at the top of a root goes by the root's name.
fn name_matches(re: &Regex, rel: &str, root: &Path) -> bool {
    let mut parts = rel.rsplit('/');
    let name = parts.next().unwrap_or_default();
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if re.is_match(stem) {
        return true;
    }
    if !stem.eq_ignore_ascii_case("readme") {
        return false;
    }
    match parts.next() {
        Some(folder) => re.is_match(folder),
        None => root
            .file_name()
            .is_some_and(|folder| re.is_match(&folder.to_string_lossy())),
    }
}

/// A cheap stand-in for the rendered title, since documents are never rendered to search them:
/// the frontmatter `title`, else the first `# ` line outside code fences, else the file name
/// without its extension. Only files in the results get a title, once per version.
fn title_of(text: &str, path: &Path) -> String {
    let (yaml, body) = split_frontmatter(text);
    yaml.and_then(frontmatter_title)
        .or_else(|| first_heading(body))
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

/// The frontmatter YAML, if there is any, and the text after it.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(yaml) = frontmatter_yaml(text.as_bytes(), true) else {
        return (None, text);
    };
    // `yaml` is a slice of `text` that ends where the closing `---` line starts.
    let start = yaml.as_ptr() as usize - text.as_ptr() as usize;
    let end = start + yaml.len();
    let body = text[end..].split_once('\n').map_or("", |(_, body)| body);
    (Some(&text[start..end]), body)
}

fn frontmatter_title(yaml: &str) -> Option<String> {
    let Frontmatter::Parsed { entries } = parse_frontmatter(yaml) else {
        return None;
    };
    let title = match &entries.iter().find(|p| p.key == "title")?.value {
        PropValue::Text(s) | PropValue::Date(s) | PropValue::DateTime(s) | PropValue::Number(s) => {
            s.trim()
        }
        PropValue::Bool(_) | PropValue::List(_) => return None,
    };
    (!title.is_empty()).then(|| title.to_owned())
}

fn first_heading(body: &str) -> Option<String> {
    // The open code fence's character and length.
    let mut open: Option<(char, usize)> = None;
    for line in body.lines() {
        match (open, fence(line)) {
            (None, Some((c, len, _))) => open = Some((c, len)),
            // A fence closes on a run of its own character at least as long, with nothing after.
            (Some((c, len)), Some((close, close_len, rest)))
                if close == c && close_len >= len && rest.trim().is_empty() =>
            {
                open = None;
            }
            (Some(_), _) => {}
            (None, None) => {
                let Some(heading) = line.strip_prefix("# ").map(str::trim) else {
                    continue;
                };
                if !heading.is_empty() {
                    return Some(heading.to_owned());
                }
            }
        }
    }
    None
}

/// A line that can open or close a code fence: three or more backticks or tildes after any
/// indentation, as (character, run length, what follows the run).
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start();
    let c = trimmed.chars().next().filter(|&c| c == '`' || c == '~')?;
    let rest = trimmed.trim_start_matches(c);
    // Both fence characters are one byte long.
    let len = trimmed.len() - rest.len();
    (len >= 3).then_some((c, len, rest))
}
