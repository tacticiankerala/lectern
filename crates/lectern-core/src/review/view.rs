//! What the UI shows of a note's review: each comment with its place in the note now, its thread
//! rendered to HTML, and the sections that couldn't be read as comments.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::anchor::{AnchorState, Resolved};
use super::text::TextMap;
use super::{raw_section_id, ClaudeKind, Comment, CommentStatus, Entry, EntryAuthor, Item, Review};

/// One entry of a comment's thread.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EntryView {
    pub author: EntryAuthor,
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
    /// The line to go to: the quote's, or when detached the pinned heading's.
    pub jump_line: Option<u32>,
    /// When detached: the deepest heading of the comment's path still in the note.
    pub pinned_heading: Option<String>,
    pub quote: String,
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
    let comments = review
        .into_iter()
        .flat_map(Review::comments)
        .enumerate()
        .map(|(i, c)| {
            let found = resolved.get(i).filter(|r| r.id == c.id);
            comment_view(c, found, text, render)
        })
        .collect();
    let unreadable = review
        .into_iter()
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
/// its stored place, pinned to a heading when one survives.
fn comment_view(
    c: &Comment,
    resolved: Option<&Resolved>,
    text: Option<&TextMap>,
    render: &dyn Fn(&str) -> String,
) -> CommentView {
    let mut view = CommentView {
        id: c.id,
        status: c.effective_status(),
        state: AnchorState::Detached,
        start_line: c.start_line,
        end_line: c.end_line,
        heading_path: c.heading_path.clone(),
        jump_line: None,
        pinned_heading: None,
        quote: c.quote.clone(),
        current_text: None,
        entries: c.entries.iter().map(|e| entry_view(e, render)).collect(),
    };
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
            }
            view.jump_line = Some(r.start_line);
            view.current_text = r.current_text.clone();
        }
        None => {}
    }
    view
}

fn entry_view(e: &Entry, render: &dyn Fn(&str) -> String) -> EntryView {
    EntryView {
        author: e.author,
        kind: e.kind,
        text: e.text.clone(),
        html: render(&e.text),
    }
}
