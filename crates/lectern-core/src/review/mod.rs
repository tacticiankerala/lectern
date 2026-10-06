//! Review comments: a sidecar file beside a note holds the reader's comments on it and Claude's
//! replies, as a Markdown thread both can edit.
//!
//! The sidecar of `plan.md` is `plan.review.md`. [`format`] reads and writes it. This module holds
//! the types, the sidecar naming rules, and the fingerprint and timestamp helpers the anchors use.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod format;

/// What every sidecar's file name ends with.
pub const SIDECAR_SUFFIX: &str = ".review.md";
/// A sidecar larger than this is shown read-only.
pub const MAX_SIDECAR_BYTES: u64 = 2 * 1024 * 1024;
/// Lectern never reads more of a sidecar than this.
pub const HARD_READ_CAP: u64 = 16 * 1024 * 1024;
/// A quote longer than this many characters is cut and ends in `…`.
pub const QUOTE_CAP: usize = 500;
/// The characters of context kept on each side of a quote, to find it again after an edit.
pub const CONTEXT_CHARS: usize = 32;
/// The longest comment text Lectern accepts, in characters.
pub const MAX_COMMENT_CHARS: usize = 20_000;

/// Where a comment stands. Written lowercase in the sidecar's section headers.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum CommentStatus {
    Open,
    Replied,
    Question,
    Pushback,
    Resolved,
    Dismissed,
}

impl CommentStatus {
    const ALL: [Self; 6] = [
        Self::Open,
        Self::Replied,
        Self::Question,
        Self::Pushback,
        Self::Resolved,
        Self::Dismissed,
    ];

    /// The name the sidecar uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Replied => "replied",
            Self::Question => "question",
            Self::Pushback => "pushback",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }

    /// Reads a status name, ignoring ASCII case.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str().eq_ignore_ascii_case(s))
    }

    /// True until the comment is resolved or dismissed.
    pub fn is_open(self) -> bool {
        !matches!(self, Self::Resolved | Self::Dismissed)
    }
}

/// What a Claude entry says about the comment: `**Claude (question):**` and so on.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ClaudeKind {
    Reply,
    Question,
    Pushback,
    Resolved,
}

impl ClaudeKind {
    const ALL: [Self; 4] = [Self::Reply, Self::Question, Self::Pushback, Self::Resolved];

    /// The name the sidecar uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Question => "question",
            Self::Pushback => "pushback",
            Self::Resolved => "resolved",
        }
    }

    /// Reads a kind name, ignoring ASCII case.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(s))
    }

    /// The status a comment takes when this is its newest entry.
    pub fn status(self) -> CommentStatus {
        match self {
            Self::Reply => CommentStatus::Replied,
            Self::Question => CommentStatus::Question,
            Self::Pushback => CommentStatus::Pushback,
            Self::Resolved => CommentStatus::Resolved,
        }
    }
}

/// Who wrote a thread entry.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum EntryAuthor {
    You,
    Claude,
}

/// One paragraph of a comment's thread, from `**You:**` or `**Claude[ (kind)]:**` to the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub author: EntryAuthor,
    /// Only Claude entries have a kind; a bare `**Claude:**` has none and counts as a reply.
    pub kind: Option<ClaudeKind>,
    pub text: String,
}

/// The `<!-- anchor … -->` line under a comment's header: what Lectern needs to find the quote
/// again, and how many thread entries the comment had when Lectern last wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AnchorMeta {
    /// The text just before the quote in the note.
    pub prefix: String,
    /// The text just after the quote in the note.
    pub suffix: String,
    /// The [`fingerprint`] of the note when the comment was anchored.
    pub fp: String,
    /// The number of thread entries when Lectern last wrote the comment.
    pub n: u32,
    /// When the comment was made, as [`iso_utc`] writes it.
    pub created: String,
}

/// One `## C<n> · <status> · L<a>–L<b> · <heading path>` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: u32,
    /// The status in the header. [`Comment::effective_status`] is the one to show.
    pub status: CommentStatus,
    pub start_line: u32,
    pub end_line: u32,
    pub heading_path: Vec<String>,
    pub anchor: Option<AnchorMeta>,
    pub quote: String,
    /// Paragraphs between the quote and the first entry.
    pub notes: Vec<String>,
    pub entries: Vec<Entry>,
    /// The section exactly as read, line endings included; written back verbatim unless `dirty`.
    pub raw: Option<String>,
    /// Set when Lectern changed the comment, so the writer regenerates it.
    pub dirty: bool,
}

impl Comment {
    /// The status to show: the kind of a Claude entry added since Lectern last wrote the comment,
    /// otherwise the header's. A user's resolve or reopen isn't undone by an older Claude entry.
    pub fn effective_status(&self) -> CommentStatus {
        let seen = self.anchor.as_ref().map_or(0, |a| a.n) as usize;
        match self.entries.last() {
            Some(e) if self.entries.len() > seen && e.author == EntryAuthor::Claude => {
                e.kind.unwrap_or(ClaudeKind::Reply).status()
            }
            _ => self.status,
        }
    }

    /// Before Lectern changes a comment: fold newer Claude entries into the header.
    pub fn settle(&mut self) {
        self.status = self.effective_status();
    }

    /// After Lectern changes a comment: record the entry count and mark it for rewriting.
    pub fn touch(&mut self) {
        let n = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        self.anchor.get_or_insert_with(AnchorMeta::default).n = n;
        self.dirty = true;
    }
}

/// One `## ` section of the sidecar.
// Most items are comments, so boxing them would only add an allocation each.
#[expect(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Comment(Comment),
    /// Any other section, or a comment that couldn't be read: kept exactly as read and written
    /// back byte for byte.
    Raw(String),
}

/// A parsed sidecar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// The file name of the note the sidecar belongs to, from its frontmatter; empty if missing.
    pub note: String,
    /// Everything before the first section, exactly as read: frontmatter, title and the
    /// instructions comment.
    pub preamble: String,
    pub items: Vec<Item>,
    /// The file has CRLF line endings (any at all), so freshly written comments use them too.
    pub crlf: bool,
    /// The file started with a UTF-8 byte order mark, so it's written back with one.
    pub bom: bool,
}

impl Review {
    /// The comments, in file order.
    pub fn comments(&self) -> impl Iterator<Item = &Comment> {
        self.items.iter().filter_map(|item| match item {
            Item::Comment(c) => Some(c),
            Item::Raw(_) => None,
        })
    }

    pub fn comment_mut(&mut self, id: u32) -> Option<&mut Comment> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Comment(c) if c.id == id => Some(c),
            _ => None,
        })
    }

    /// The id for a new comment: one more than any in the file, counting `## C<n>` sections that
    /// are kept raw, so an id is never reused.
    pub fn next_id(&self) -> u32 {
        let raw_ids = self.items.iter().filter_map(|item| match item {
            Item::Raw(section) => raw_section_id(section),
            Item::Comment(_) => None,
        });
        self.comments()
            .map(|c| c.id)
            .chain(raw_ids)
            .max()
            .map_or(1, |max| max.saturating_add(1))
    }

    /// The comments still open, by their effective status.
    pub fn open_count(&self) -> u32 {
        let open = self
            .comments()
            .filter(|c| c.effective_status().is_open())
            .count();
        u32::try_from(open).unwrap_or(u32::MAX)
    }
}

/// The `<n>` of a raw section that starts `## C<n>`.
fn raw_section_id(section: &str) -> Option<u32> {
    let rest = section.strip_prefix("## C")?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..digits].parse().ok()
}

/// The sidecar beside `note`: `plan.md` → `plan.review.md`, `x.markdown` → `x.markdown.review.md`.
pub fn sidecar_path(note: &Path) -> PathBuf {
    let name = note
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("md") && !stem.is_empty() => stem.to_owned(),
        _ => name,
    };
    note.with_file_name(format!("{base}{SIDECAR_SUFFIX}"))
}

/// True if a file name could be a sidecar's: something, then `.review.md` in any case.
pub fn is_sidecar_name(name: &str) -> bool {
    name.len() > SIDECAR_SUFFIX.len() && name.to_ascii_lowercase().ends_with(SIDECAR_SUFFIX)
}

/// The `note:` of a sidecar's frontmatter, or `None` if `head` isn't a sidecar's.
///
/// Line-based on purpose: the scan calls it on every 4 KiB head and it must never fail loudly. A
/// BOM and CRLF are accepted. The first line must be `---`; `key: value` lines are read up to the
/// next `---`, and the note is returned only if `lectern-review:` holds a positive integer.
pub fn sidecar_note(head: &str) -> Option<String> {
    let head = head.strip_prefix('\u{feff}').unwrap_or(head);
    let mut lines = head.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut marked = false;
    let mut note = None;
    for line in lines {
        if line.trim_end() == "---" {
            return note.filter(|_| marked);
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim_end() {
            "lectern-review" => marked = value.trim().parse::<u32>().is_ok_and(|v| v > 0),
            "note" => note = read_frontmatter_value(value),
            _ => {}
        }
    }
    None
}

/// A frontmatter value: a JSON string if it starts with `"`, a YAML single-quoted string (where
/// `''` stands for `'`) if it's wrapped in single quotes, otherwise the trimmed text.
fn read_frontmatter_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with('"') {
        return serde_json::from_str(value).ok();
    }
    let single_quoted = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''));
    Some(match single_quoted {
        Some(inner) => inner.replace("''", "'"),
        None => value.to_owned(),
    })
}

/// 64-bit FNV-1a: stable across runs and platforms, unlike `std`'s hasher.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

/// A note's fingerprint: `fnv1a64:` and 16 lowercase hex digits.
pub fn fingerprint(source: &str) -> String {
    format!("fnv1a64:{:016x}", fnv1a64(source.as_bytes()))
}

/// `t` as `YYYY-MM-DDTHH:MM:SSZ`, in UTC to the second. Times before 1970 give the epoch.
pub fn iso_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (year, month, day) = civil_from_days(secs / 86_400);
    let time = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01: Howard Hinnant's `civil_from_days`,
/// for days on or after the epoch.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365], from 1 March
    let mp = (5 * doy + 2) / 153; // [0, 11], from March
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}
