//! What the UI shows of a note's review: each comment with its place in the note now, its thread
//! rendered to HTML, and the sections that couldn't be read as comments.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::anchor::{stored_span, AnchorState, Resolved};
use super::text::TextMap;
use super::{raw_section_id, ClaudeKind, Comment, CommentStatus, Entry, EntryAuthor, Item, Review};

/// One entry of a comment's thread.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EntryView {
    pub author: EntryAuthor,
    /// `You` for the reader; an agent's name as written.
    pub name: String,
    pub kind: Option<ClaudeKind>,
    /// The Markdown as written.
    pub text: String,
    /// The Markdown rendered and sanitised.
    pub html: String,
}

/// A comment as the UI shows it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommentView {
    pub id: u32,
    /// The effective status.
    pub status: CommentStatus,
    pub state: AnchorState,
    /// The quote's lines now; the stored lines when it's detached.
    pub start_line: u32,
    pub end_line: u32,
    pub heading_path: Vec<String>,
    /// The line to go to: the quote's, or when detached the pinned heading's, or with no heading
    /// left, the note's top: its first line with text (after any frontmatter), else line 1.
    pub jump_line: Option<u32>,
    /// When detached: the deepest heading of the comment's path still in the note.
    pub pinned_heading: Option<String>,
    pub quote: String,
    /// Where the quote (or the passage it became, when it moved) is in the note's visible text,
    /// the text the UI builds from the page too: from `text_start` up to `text_end`, counted in
    /// UTF-16 code units, as the UI's strings count. `None` when it's detached, or wasn't found.
    pub text_start: Option<u32>,
    pub text_end: Option<u32>,
    /// The passage the quote became, when it moved.
    pub current_text: Option<String>,
    pub entries: Vec<EntryView>,
}

/// A `## C<n>` section that couldn't be read as a comment, as written.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnreadableView {
    pub raw: String,
}

/// Everything the UI shows of a note's review.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewPayload {
    pub note_path: String,
    pub sidecar_path: String,
    /// The paths as a WSL shell knows them, when there's such a path.
    pub note_wsl_path: Option<String>,
    pub sidecar_wsl_path: Option<String>,
    /// There is a sidecar to show.
    pub exists: bool,
    /// Why the sidecar is shown read-only, or not at all, if it is.
    pub read_only: Option<String>,
    /// In file order.
    pub comments: Vec<CommentView>,
    pub unreadable: Vec<UnreadableView>,
    /// The comments whose effective status is still open.
    pub open_count: u32,
}

/// What [`build_payload`] builds from.
pub struct PayloadInput<'a> {
    pub note: &'a Path,
    pub sidecar: &'a Path,
    /// `None` when there's no sidecar to show.
    pub review: Option<&'a Review>,
    /// [`resolve_all`](super::anchor::resolve_all) for `review`: one result per comment, in file
    /// order. A comment without its result is shown detached at its stored place.
    pub resolved: &'a [Resolved],
    /// The note's text, for the heading path of a comment found in it.
    pub text: Option<&'a TextMap>,
    pub read_only: Option<String>,
    pub note_wsl: Option<String>,
    pub sidecar_wsl: Option<String>,
}

/// Builds the payload. `render` turns comment Markdown into sanitised HTML: the caller passes the
/// note's render pipeline, so comment links follow the note's rules.
pub fn build_payload(input: PayloadInput, render: &dyn Fn(&str) -> String) -> ReviewPayload {
    let PayloadInput {
        note,
        sidecar,
        review,
        resolved,
        text,
        read_only,
        note_wsl,
        sidecar_wsl,
    } = input;
    let (mut comments, spans): (Vec<CommentView>, Vec<Option<(usize, usize)>>) = review
        .into_iter()
        .flat_map(Review::comments)
        .enumerate()
        .map(|(i, c)| {
            let found = resolved.get(i).filter(|r| r.id == c.id);
            comment_view(c, found, text, render)
        })
        .unzip();
    if let Some(text) = text {
        let bytes: Vec<usize> = spans.iter().flatten().flat_map(|&(s, e)| [s, e]).collect();
        let mut units = utf16_offsets(text.text(), &bytes).into_iter();
        for (view, span) in comments.iter_mut().zip(&spans) {
            if span.is_some() {
                view.text_start = units.next();
                view.text_end = units.next();
            }
        }
    }
    // A file without a sidecar's frontmatter isn't one Lectern made (store.rs shows it read-only):
    // its `## C…` headings are the owner's own, not comments that couldn't be read.
    let unreadable = review
        .into_iter()
        .filter(|r| !r.note.is_empty())
        .flat_map(|r| &r.items)
        .filter_map(|item| match item {
            Item::Raw(section) if raw_section_id(section).is_some() => Some(UnreadableView {
                raw: section.clone(),
            }),
            _ => None,
        })
        .collect();
    ReviewPayload {
        note_path: note.to_string_lossy().into_owned(),
        sidecar_path: sidecar.to_string_lossy().into_owned(),
        note_wsl_path: note_wsl,
        sidecar_wsl_path: sidecar_wsl,
        exists: review.is_some(),
        read_only,
        comments,
        unreadable,
        open_count: review.map_or(0, Review::open_count),
    }
}

/// A comment at the place `resolved` found for it: there when its quote was found, otherwise at
/// its stored place, pinned to a heading when one survives, or else to the note's top (its first
/// block, or line 1 without the note's text or any block). With it, the byte range of its quote
/// (or of the passage it became) in `text`, when that's known.
fn comment_view(
    c: &Comment,
    resolved: Option<&Resolved>,
    text: Option<&TextMap>,
    render: &dyn Fn(&str) -> String,
) -> (CommentView, Option<(usize, usize)>) {
    let mut view = CommentView {
        id: c.id,
        status: c.effective_status(),
        state: AnchorState::Detached,
        start_line: c.start_line,
        end_line: c.end_line,
        heading_path: c.heading_path.clone(),
        jump_line: Some(
            text.and_then(|t| t.blocks().iter().map(|b| b.start_line).min())
                .unwrap_or(1),
        ),
        pinned_heading: None,
        quote: c.quote.clone(),
        text_start: None,
        text_end: None,
        current_text: None,
        entries: c.entries.iter().map(|e| entry_view(e, render)).collect(),
    };
    let mut span = None;
    match resolved {
        Some(r) if r.state == AnchorState::Detached => {
            if let Some((heading, line)) = &r.pinned_heading {
                view.pinned_heading = Some(heading.clone());
                view.jump_line = Some(*line);
            }
        }
        Some(r) => {
            view.state = r.state;
            view.start_line = r.start_line;
            view.end_line = r.end_line;
            if let Some(text) = text {
                view.heading_path = text.heading_path_at(r.start_line);
                // Left where it was, the note unchanged, it wasn't looked for: look now.
                span = r.span.or_else(|| stored_span(c, text));
            }
            view.jump_line = Some(r.start_line);
            view.current_text = r.current_text.clone();
        }
        None => {}
    }
    (view, span)
}

/// The UTF-16 code-unit offsets into `text` of the byte offsets `at`, each on a character
/// boundary, in one pass over the text.
fn utf16_offsets(text: &str, at: &[usize]) -> Vec<u32> {
    let unit = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    if text.is_ascii() {
        return at.iter().map(|&b| unit(b)).collect();
    }
    let mut order: Vec<usize> = (0..at.len()).collect();
    order.sort_unstable_by_key(|&i| at[i]);
    let mut out = vec![0; at.len()];
    let (mut byte, mut units) = (0, 0);
    let mut chars = text.chars();
    for i in order {
        while byte < at[i] {
            let Some(ch) = chars.next() else { break };
            byte += ch.len_utf8();
            units += ch.len_utf16();
        }
        out[i] = unit(units);
    }
    out
}

fn entry_view(e: &Entry, render: &dyn Fn(&str) -> String) -> EntryView {
    EntryView {
        author: e.author,
        name: e.name.clone(),
        kind: e.kind,
        text: e.text.clone(),
        html: render(&e.text),
    }
}
