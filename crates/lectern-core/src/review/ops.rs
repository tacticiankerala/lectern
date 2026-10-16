//! The changes the UI asks for: add a comment, reply, change its status, reattach it to another
//! passage, or edit one of the reader's own entries. An agent's comment and reply (through Lectern's
//! agent tools) are the same operations, carrying the agent as their author.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::anchor::make_anchor;
use super::format::holds_agent_name;
use super::text::TextMap;
use super::{
    AnchorMeta, ClaudeKind, Comment, CommentStatus, Entry, EntryAuthor, Item, Review,
    MAX_COMMENT_CHARS,
};
use crate::agent::UNNAMED_AGENT;

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
    /// Where the quote starts in the note's visible text, in UTF-16 code units (as
    /// `CommentView::text_start` counts), when the sender knows exactly: an agent's target, which
    /// may have been told apart by the text after it. The quote found there is anchored there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text_start: Option<u32>,
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

/// The agent an entry is written for, under its name (`Claude`), and what the entry is: the entry
/// is `**Claude (question):**`, as an agent writing the sidecar by hand would write it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct AgentAuthor {
    pub name: String,
    pub kind: ClaudeKind,
}

/// One change to the sidecar. An `add` or a `reply` with an `author` is an agent's: its entry is
/// the agent's, never yours (see [`apply`]).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum ReviewOp {
    Add {
        anchor: NewAnchor,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        author: Option<AgentAuthor>,
    },
    Reply {
        id: u32,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        author: Option<AgentAuthor>,
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
///
/// An agent's comment or reply (`author`) is written as the agent would write it by hand: its
/// entry is added after Lectern's count of the thread's entries (the anchor line's `n=`), so the
/// entry's kind gives the comment its status, and the header is left as it was (a new comment's
/// `open`). Your next entry, or status change, then counts it as any agent entry.
pub fn apply(review: &mut Review, op: &ReviewOp, ctx: &OpContext) -> Result<u32, OpError> {
    match op {
        ReviewOp::Add {
            anchor,
            text,
            author,
        } => {
            let text = clean(text)?;
            let parts = make_anchor(ctx.text, anchor, None, ctx.fingerprint, ctx.now);
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
                entries: Vec::new(),
                raw: None,
                dirty: true,
            };
            match author {
                Some(agent) => {
                    c.touch();
                    c.entries.push(agents(agent, text));
                }
                None => {
                    c.entries.push(yours(text));
                    c.touch();
                }
            }
            review.items.push(Item::Comment(c));
            Ok(id)
        }
        ReviewOp::Reply { id, text, author } => {
            let c = comment(review, *id)?;
            let text = clean(text)?;
            Ok(match author {
                // Settled and counted first, so the agent's entry is the newer one.
                Some(agent) => {
                    c.settle();
                    c.touch();
                    c.entries.push(agents(agent, text));
                    c.id
                }
                None => change(c, |c| {
                    c.entries.push(yours(text));
                    c.status = CommentStatus::Open;
                }),
            })
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
            let parts = make_anchor(ctx.text, anchor, Some(c), ctx.fingerprint, ctx.now);
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

/// Changes a comment the way every operation does: newer agent entries are folded into its status
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
        name: "You".to_owned(),
        kind: None,
        text,
    }
}

/// The entry of the agent `author`: under its name when the sidecar reads that back as an agent's,
/// otherwise (`You`, or a name an entry line can't hold) as [`UNNAMED_AGENT`].
fn agents(author: &AgentAuthor, text: String) -> Entry {
    let name = if holds_agent_name(&author.name) {
        author.name.clone()
    } else {
        UNNAMED_AGENT.to_owned()
    };
    Entry {
        author: EntryAuthor::Agent,
        name,
        kind: Some(author.kind),
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
