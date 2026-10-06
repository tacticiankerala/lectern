//! The changes the UI asks for: add a comment, reply, change its status, reattach it to another
//! passage, or edit one of the reader's own entries.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::anchor::make_anchor;
use super::text::TextMap;
use super::{
    AnchorMeta, Comment, CommentStatus, Entry, EntryAuthor, Item, Review, MAX_COMMENT_CHARS,
};

/// The passage a comment is about, as the UI sees it: its source lines and visible text.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewAnchor {
    pub start_line: u32,
    pub end_line: u32,
    pub quote: String,
    /// Up to `CONTEXT_CHARS` (32) characters of the note's visible text just before a selection,
    /// which tell apart a phrase found more than once in its lines; empty for a whole block.
    #[serde(default)]
    pub prefix: String,
}

/// A status the reader can set.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum StatusChange {
    Resolve,
    Reopen,
    Dismiss,
}

/// One change to the sidecar.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum ReviewOp {
    Add {
        anchor: NewAnchor,
        text: String,
    },
    Reply {
        id: u32,
        text: String,
    },
    SetStatus {
        id: u32,
        change: StatusChange,
    },
    Reattach {
        id: u32,
        anchor: NewAnchor,
    },
    /// `entry` is the index of the entry in the comment's thread.
    Edit {
        id: u32,
        entry: u32,
        text: String,
    },
}

/// What an operation needs to know about the note.
pub struct OpContext<'a> {
    pub text: &'a TextMap,
    pub fingerprint: &'a str,
    /// The time to record on a new comment, as `iso_utc` writes it.
    pub now: &'a str,
}

/// Why an operation was refused. Shown to the reader as it is.
#[derive(Debug, PartialEq, Eq)]
pub enum OpError {
    NoSuchComment(u32),
    NoSuchEntry,
    NotYourEntry,
    Empty,
    TooLong,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoSuchComment(_) => "That comment no longer exists.",
            Self::NoSuchEntry => "That reply no longer exists.",
            Self::NotYourEntry => "Only your own replies can be edited.",
            Self::Empty => "Write something first.",
            Self::TooLong => "That comment is too long (over 20,000 characters).",
        })
    }
}

impl std::error::Error for OpError {}

/// Applies `op` to the review and returns the id of the comment it changed. A refused operation
/// changes nothing.
pub fn apply(review: &mut Review, op: &ReviewOp, ctx: &OpContext) -> Result<u32, OpError> {
    match op {
        ReviewOp::Add { anchor, text } => {
            let text = clean(text)?;
            let parts = make_anchor(ctx.text, anchor, ctx.fingerprint, ctx.now);
            let id = review.next_id();
            let mut c = Comment {
                id,
                status: CommentStatus::Open,
                start_line: parts.start_line,
                end_line: parts.end_line,
                heading_path: parts.heading_path,
                anchor: Some(parts.meta),
                quote: parts.quote,
                notes: Vec::new(),
                entries: vec![yours(text)],
                raw: None,
                dirty: true,
            };
            c.touch();
            review.items.push(Item::Comment(c));
            Ok(id)
        }
        ReviewOp::Reply { id, text } => {
            let c = comment(review, *id)?;
            let text = clean(text)?;
            Ok(change(c, |c| {
                c.entries.push(yours(text));
                c.status = CommentStatus::Open;
            }))
        }
        ReviewOp::SetStatus { id, change: to } => {
            let status = match to {
                StatusChange::Resolve => CommentStatus::Resolved,
                StatusChange::Reopen => CommentStatus::Open,
                StatusChange::Dismiss => CommentStatus::Dismissed,
            };
            Ok(change(comment(review, *id)?, |c| c.status = status))
        }
        ReviewOp::Reattach { id, anchor } => {
            let c = comment(review, *id)?;
            let parts = make_anchor(ctx.text, anchor, ctx.fingerprint, ctx.now);
            Ok(change(c, |c| {
                c.start_line = parts.start_line;
                c.end_line = parts.end_line;
                c.heading_path = parts.heading_path;
                c.quote = parts.quote;
                let meta = c.anchor.get_or_insert_with(|| AnchorMeta {
                    created: parts.meta.created,
                    ..AnchorMeta::default()
                });
                meta.prefix = parts.meta.prefix;
                meta.suffix = parts.meta.suffix;
                meta.fp = parts.meta.fp;
            }))
        }
        ReviewOp::Edit { id, entry, text } => {
            let c = comment(review, *id)?;
            let index = usize::try_from(*entry).unwrap_or(usize::MAX);
            match c.entries.get(index) {
                None => return Err(OpError::NoSuchEntry),
                Some(e) if e.author != EntryAuthor::You => return Err(OpError::NotYourEntry),
                Some(_) => {}
            }
            let text = clean(text)?;
            Ok(change(c, |c| c.entries[index].text = text))
        }
    }
}

fn comment(review: &mut Review, id: u32) -> Result<&mut Comment, OpError> {
    review.comment_mut(id).ok_or(OpError::NoSuchComment(id))
}

/// Changes a comment the way every operation does: newer Claude entries are folded into its status
/// first, and it's marked for rewriting with its entry count last.
fn change(c: &mut Comment, f: impl FnOnce(&mut Comment)) -> u32 {
    c.settle();
    f(c);
    c.touch();
    c.id
}

fn yours(text: String) -> Entry {
    Entry {
        author: EntryAuthor::You,
        kind: None,
        text,
    }
}

/// Comment text as stored: trimmed, with LF line endings, neither empty nor too long.
fn clean(text: &str) -> Result<String, OpError> {
    let text = text.trim().replace("\r\n", "\n");
    if text.is_empty() {
        Err(OpError::Empty)
    } else if text.chars().count() > MAX_COMMENT_CHARS {
        Err(OpError::TooLong)
    } else {
        Ok(text)
    }
}
