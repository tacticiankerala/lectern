//! The review tools: `comment`, `comments`, `reply` and `resolve` work a file's review threads, in
//! its sidecar, under the agent's own name.
//!
//! - **Where a comment goes:** in a note or a text file, core finds the target in the text as shown
//!   ([`comment_anchor`]): a quote is the comment's quote, and a heading or a line gives the text of
//!   its block. An HTML page's window finds it in what the page reported.
//! - **What is written:** the agent's entry, `**Claude (question):**`, through the sidecar store,
//!   as the agent writing the sidecar by hand would write it ([`ReviewOp`]'s `author`). It is never
//!   yours, and the thread's status comes from it, as the `n=` rule has it for any agent entry.
//! - **What is listed:** each thread as the Comments tab shows it ([`list_threads`]), found in the
//!   note's text; a page's, when no window shows it, at its stored place, in an unknown state. A
//!   long list comes in parts small enough for an agent's tool output ([`threads_page`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::read::READ_BYTES;
use super::request::{EntryKind, Target, ThreadFilter};
use super::target::find_target;
use super::tools::listed;
use crate::ipc::PathForms;
use crate::review::anchor::{resolve_all, AnchorState};
use crate::review::ops::{AgentAuthor, NewAnchor, ReviewOp};
use crate::review::text::{NoteText, TextMap};
use crate::review::view::{build_payload, utf16_offsets, CommentView, PayloadInput};
use crate::review::{ClaudeKind, CommentStatus, EntryAuthor, Review, CONTEXT_CHARS, QUOTE_CAP};

/// The closing note of a `resolve` that gives none.
pub const RESOLVED: &str = "Resolved.";

/// The most bytes a `comments` answer takes as JSON: as a `read`'s, as MCP carries it twice.
pub const COMMENTS_BYTES: usize = READ_BYTES;

/// The most characters of a thread's quote, and of each of its headings, and the most headings,
/// the innermost, an answer gives of a thread too long for it (see [`threads_page`]).
const QUOTE_CHARS: usize = QUOTE_CAP;
const HEADING_CHARS: usize = 200;
const HEADINGS_KEPT: usize = 12;

impl From<EntryKind> for ClaudeKind {
    fn from(kind: EntryKind) -> Self {
        match kind {
            EntryKind::Question => Self::Question,
            EntryKind::Reply => Self::Reply,
            EntryKind::Pushback => Self::Pushback,
        }
    }
}

/// A thread's id as an agent writes it: `C3`.
pub fn thread_id(n: u32) -> String {
    format!("C{n}")
}

/// The number of the thread `id` names (`C3`); none for anything else.
pub fn thread_number(id: &str) -> Option<u32> {
    id.strip_prefix('C')
        .filter(|digits| !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

/// The passage of the note or text file named `file`, read as `source`, whose text as shown is
/// `text`, that a comment on `target` is about, as the window would send a selection of it: its
/// lines, its text as shown and the text just before it.
///
/// A quote is the passage it names. A heading or a line gives the text of its block, and a quote
/// found only in the source (with Markdown marks) the text of the blocks on its lines. An error, a
/// sentence for the agent, when the target isn't there, or a line has no text.
pub fn comment_anchor(
    text: &TextMap,
    source: &str,
    file: &str,
    target: &Target,
) -> Result<NewAnchor, String> {
    let found = find_target(text, source, file, target)?;
    let (start, end) = match found.span {
        Some(span) => span,
        None => blocks_on(text, found.start_line, found.end_line).ok_or_else(|| match target {
            Target::Line { line } => format!("Line {line} of {file} has no text to comment on."),
            _ => format!("That passage isn't in {file}."),
        })?,
    };
    let (start_line, end_line) = text.lines_of(start, end);
    // Where it starts too, so the occurrence chosen here is the one anchored, whatever the text
    // before it shares with another.
    let text_start = utf16_offsets(text.text(), &[start]).first().copied();
    Ok(NewAnchor {
        start_line,
        end_line,
        quote: text.text()[start..end].to_owned(),
        prefix: text.before(start, CONTEXT_CHARS),
        text_start,
    })
}

/// Where the text of the blocks on lines `start..=end` is, from the first's start to the last's
/// end; none when no block is on them.
fn blocks_on(text: &TextMap, start: u32, end: u32) -> Option<(usize, usize)> {
    let mut on = text
        .blocks()
        .iter()
        .filter(|b| b.start_line <= end && b.end_line >= start);
    let first = on.next()?;
    let last = on.next_back().unwrap_or(first);
    Some((first.start, last.end))
}

/// The agent `agent` (its name as the reader sees it), writing an entry of `kind`.
fn author(agent: &str, kind: ClaudeKind) -> Option<AgentAuthor> {
    Some(AgentAuthor {
        name: agent.to_owned(),
        kind,
    })
}

/// `comment`'s operation: a thread on `anchor`, started by `agent` saying `text`, a question
/// unless `kind` says otherwise.
pub fn comment_op(
    anchor: NewAnchor,
    text: String,
    agent: &str,
    kind: Option<EntryKind>,
) -> ReviewOp {
    ReviewOp::Add {
        anchor,
        text,
        author: author(agent, kind.map_or(ClaudeKind::Question, Into::into)),
    }
}

/// `reply`'s operation: `agent` adds `text` to thread `id`, a reply unless `kind` says otherwise.
pub fn reply_op(id: u32, text: String, agent: &str, kind: Option<EntryKind>) -> ReviewOp {
    ReviewOp::Reply {
        id,
        text,
        author: author(agent, kind.map_or(ClaudeKind::Reply, Into::into)),
    }
}

/// `resolve`'s operation: `agent` closes thread `id` with `text`, [`RESOLVED`] when it gives none.
pub fn resolve_op(id: u32, text: Option<String>, agent: &str) -> ReviewOp {
    ReviewOp::Reply {
        id,
        text: text.unwrap_or_else(|| RESOLVED.to_owned()),
        author: author(agent, ClaudeKind::Resolved),
    }
}

/// The error for thread `n` of `file`, which isn't there, naming the threads that are (`ids`).
pub fn no_such_thread(n: u32, file: &str, ids: &[u32]) -> String {
    let there: Vec<String> = ids.iter().map(|&id| thread_id(id)).collect();
    let there: Vec<&str> = there.iter().map(String::as_str).collect();
    let missing = thread_id(n);
    match there[..] {
        [] => format!("There's no thread {missing} in {file}: it has no comments yet."),
        [only] => format!("There's no thread {missing} in {file}. Its only thread is {only}."),
        _ => format!(
            "There's no thread {missing} in {file}. Its threads are {}.",
            listed(&there, "and")
        ),
    }
}

/// Where a thread's quote is in its file now: as the Comments tab has it, or `unknown` for an HTML
/// page no window shows, whose text only its window has.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Anchored,
    Moved,
    Detached,
    Unknown,
}

/// One entry of a thread, as written: who wrote it (you, or an agent by name), what it is (an
/// agent's kind), and its Markdown.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEntry {
    pub author: EntryAuthor,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ClaudeKind>,
    pub text: String,
}

/// A thread, as `comments` gives it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// `C3`.
    pub id: String,
    /// The status the Comments tab shows.
    pub status: CommentStatus,
    pub state: ThreadState,
    /// Its quote's lines now; the stored ones when it is detached or its state unknown.
    pub start_line: u32,
    pub end_line: u32,
    /// The headings it is under, outermost first.
    pub heading_path: Vec<String>,
    pub quote: String,
    pub entries: Vec<ThreadEntry>,
    /// Too long for an answer of its own, its entries were cut short (see [`threads_page`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `comments`' answer: a part of the list of threads, how many there are in all, where the next
/// part starts, and why Lectern won't change the sidecar, when it won't.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommentsAnswer {
    pub threads: Vec<Thread>,
    /// How many threads the list holds.
    pub total: u32,
    /// Where the next part starts, the `from` to ask for it with; none at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

/// `comment`'s answer: the new thread, and the sidecar it is in, in both forms.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CommentAnswer {
    pub id: String,
    pub sidecar: PathForms,
}

/// `reply`'s and `resolve`'s answer: the thread, its status now, and the sidecar, in both forms.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ThreadAnswer {
    pub id: String,
    pub status: CommentStatus,
    pub sidecar: PathForms,
}

/// The threads of `review` that `filter` lists, in file order, as the Comments tab shows them:
/// found in `note`'s text, or at their stored places, their state unknown, without it.
///
/// `filter` lists the threads of one status (`open`, `question`, `pushback`, `resolved`), or `all`;
/// none lists every thread that isn't resolved or dismissed.
pub fn list_threads(
    review: &Review,
    note: Option<&NoteText>,
    filter: Option<ThreadFilter>,
) -> Vec<Thread> {
    let resolved = note.map_or_else(Vec::new, |note| {
        resolve_all(review, &note.text, &note.fingerprint)
    });
    let payload = build_payload(
        PayloadInput {
            note: Path::new(""),
            sidecar: Path::new(""),
            review: Some(review),
            resolved: &resolved,
            text: note.map(|note| &note.text),
            read_only: None,
            note_wsl: None,
            sidecar_wsl: None,
        },
        &|_| String::new(),
    );
    payload
        .comments
        .into_iter()
        .filter(|c| lists(filter, c.status))
        .map(|c| thread(c, note.is_some()))
        .collect()
}

/// `comments`' answer for `threads`, the list [`list_threads`] gives: from the thread at `from` (0
/// by default; none past the end), as many whole threads as keep the answer within
/// [`COMMENTS_BYTES`] as JSON, and at least one. A thread too long for an answer of its own comes
/// alone, cut short to fit and marked `truncated` ([`cut_to_fit`]).
pub fn threads_page(
    threads: Vec<Thread>,
    from: Option<u32>,
    read_only: Option<String>,
) -> CommentsAnswer {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let total = threads.len();
    let start = usize::try_from(from.unwrap_or(0))
        .unwrap_or(usize::MAX)
        .min(total);
    let mut answer = CommentsAnswer {
        threads: Vec::new(),
        total: count(total),
        next: Some(count(total)),
        read_only,
    };
    // The answer without its threads, `next` at its longest.
    let mut used = json_len(&answer);
    answer.next = None;
    for (at, mut thread) in threads.into_iter().enumerate().skip(start) {
        let comma = usize::from(!answer.threads.is_empty());
        let mut len = json_len(&thread);
        if used + comma + len > COMMENTS_BYTES {
            if !answer.threads.is_empty() {
                answer.next = Some(count(at));
                break;
            }
            thread = cut_to_fit(thread, COMMENTS_BYTES.saturating_sub(used));
            len = json_len(&thread);
        }
        used += comma + len;
        answer.threads.push(thread);
    }
    answer
}

/// `thread`, cut short to take at most `budget` bytes as JSON, and marked `truncated`. Its quote
/// and headings are cut to [`QUOTE_CHARS`] and [`HEADING_CHARS`] characters, and to its innermost
/// [`HEADINGS_KEPT`] headings (a hand-edited sidecar may hold any length); then, if it still
/// doesn't fit, its entries' texts are cut to the most characters that fit, each then ending in an
/// ellipsis; should even none of their text fit, only as many of its latest entries as do. Each
/// step is checked against the size of the thread as JSON.
fn cut_to_fit(mut thread: Thread, budget: usize) -> Thread {
    let entries = std::mem::take(&mut thread.entries);
    thread.truncated = true;
    thread.quote = shortened(&thread.quote, QUOTE_CHARS);
    let outer = thread.heading_path.len().saturating_sub(HEADINGS_KEPT);
    thread.heading_path = thread.heading_path[outer..]
        .iter()
        .map(|heading| shortened(heading, HEADING_CHARS))
        .collect();
    let longest = entries
        .iter()
        .map(|e| e.text.chars().count())
        .max()
        .unwrap_or(0);
    let cut = |chars: usize, keep: usize| Thread {
        entries: entries[entries.len().saturating_sub(keep)..]
            .iter()
            .map(|e| ThreadEntry {
                author: e.author,
                name: e.name.clone(),
                kind: e.kind,
                text: shortened(&e.text, chars),
            })
            .collect(),
        ..thread.clone()
    };
    let fits = |t: &Thread| json_len(t) <= budget;
    let all = entries.len();
    let whole = cut(longest, all);
    if fits(&whole) {
        return whole;
    }
    if fits(&cut(0, all)) {
        // The most characters that fit: the size only grows with them, and all of them don't.
        let (mut fitting, mut too_many) = (0, longest);
        while too_many - fitting > 1 {
            let mid = fitting + (too_many - fitting) / 2;
            if fits(&cut(mid, all)) {
                fitting = mid;
            } else {
                too_many = mid;
            }
        }
        return cut(fitting, all);
    }
    // The most of the latest entries that fit, with none of their text.
    let (mut fitting, mut too_many) = (0, all);
    while too_many - fitting > 1 {
        let mid = fitting + (too_many - fitting) / 2;
        if fits(&cut(0, mid)) {
            fitting = mid;
        } else {
            too_many = mid;
        }
    }
    let kept = cut(0, fitting);
    if fits(&kept) {
        return kept;
    }
    // Not even its quote and headings, cut short, fit the room there is: they go too.
    Thread {
        quote: "…".to_owned(),
        heading_path: Vec::new(),
        ..thread
    }
}

/// `text`, cut to its first `chars` characters and an ellipsis when longer.
fn shortened(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// How many bytes `value` takes as JSON.
fn json_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |json| json.len())
}

/// Whether `filter` lists a thread whose status is `status`.
fn lists(filter: Option<ThreadFilter>, status: CommentStatus) -> bool {
    match filter {
        None => status.is_open(),
        Some(ThreadFilter::All) => true,
        Some(ThreadFilter::Open) => status == CommentStatus::Open,
        Some(ThreadFilter::Question) => status == CommentStatus::Question,
        Some(ThreadFilter::Pushback) => status == CommentStatus::Pushback,
        Some(ThreadFilter::Resolved) => status == CommentStatus::Resolved,
    }
}

/// The comment `c` as a thread: its state as found, when it was looked for (`placed`).
fn thread(c: CommentView, placed: bool) -> Thread {
    let state = match (placed, c.state) {
        (false, _) => ThreadState::Unknown,
        (true, AnchorState::Anchored) => ThreadState::Anchored,
        (true, AnchorState::Moved) => ThreadState::Moved,
        (true, AnchorState::Detached) => ThreadState::Detached,
    };
    Thread {
        id: thread_id(c.id),
        status: c.status,
        state,
        start_line: c.start_line,
        end_line: c.end_line,
        heading_path: c.heading_path,
        quote: c.quote,
        entries: c
            .entries
            .into_iter()
            .map(|e| ThreadEntry {
                author: e.author,
                name: e.name,
                kind: e.kind,
                text: e.text,
            })
            .collect(),
        truncated: false,
    }
}
