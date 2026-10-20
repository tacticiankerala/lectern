//! `read`: the text of the file a window shows, as shown, a page at a time. A note's or a text
//! file's is the text the review comments see, read from its source; an HTML page's is what its
//! bridge reported as the page ran, scripts and all, which its window's UI hands over. Either is
//! one text, its blocks joined by single spaces, paged by character from `from`: each page is that
//! text from there, once, with where each block it holds starts in it, the block's source line
//! (notes and text files only) and the headings it is under, each path of headings listed once.
//!
//! A page is kept small enough for an agent's tool output, which carries it twice (as structured
//! content and as text): at most `max` characters, and the whole answer, file and all, at most
//! [`READ_BYTES`] as JSON. A page of many small blocks ends sooner, and `next` goes on from there.
//!
//! An HTML page whose bridge stopped its text at its limit reads as cut short ([`PAGE_TEXT_CUT`]):
//! every page of it says so, and the last is `truncated` all the same, with no `next`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::status::HEADING_SEPARATOR;
use super::tools::READ_MAX;
use crate::ipc::PathForms;
use crate::review::text::TextMap;

/// The most characters a `read` gives when it doesn't say.
pub const READ_DEFAULT: u32 = 8_000;

/// The most bytes a `read`'s answer takes as JSON.
pub const READ_BYTES: usize = 48 * 1024;

/// Why an HTML page's text is incomplete when its bridge stopped it at its limit.
pub const PAGE_TEXT_CUT: &str = "page text over 2 MiB";

/// The most characters of a path of headings a page lists; a longer one is cut, with an ellipsis.
const HEADINGS_MAX: usize = 2000;

/// A page of a file's text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadText {
    /// The text from the character asked for, at most as many characters as asked for.
    pub text: String,
    /// The paths of headings the page's blocks are under, each once: `Plan › Rollout`.
    pub headings: Vec<String>,
    /// The blocks that text holds, in order.
    pub blocks: Vec<ReadBlock>,
    /// More text follows, or (with `incomplete`) the file's text goes on past what could be read.
    pub truncated: bool,
    /// Where the next page starts, the `from` to ask for it with; none at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    /// Why the text read is less than the file's: an HTML page's cut at its bridge's limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<String>,
}

/// A block of a page of text: a paragraph, a heading, a code block or a table row. Its text is the
/// page's from `start` up to where the next block starts, or the page's end.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadBlock {
    /// Where it starts in the page's text, in characters; 0 for one the page before began.
    pub start: u32,
    /// The source line it starts on: notes and text files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The headings it is under, as their place in the page's `headings`; a heading is under
    /// itself. None above the first heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<u32>,
}

/// `read`'s answer: the file, in both forms, and the page of its text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadAnswer {
    pub file: PathForms,
    #[serde(flatten)]
    pub read: ReadText,
}

/// A block the longest page could hold: where it starts in the text (or the page does), its line
/// and the path of headings it is under.
struct Held {
    at: usize,
    line: Option<u32>,
    heading: Option<String>,
}

/// `read`'s answer for `file`: the page of `text` from the character `from` (0 by default), at most
/// `max` characters ([`READ_DEFAULT`] by default, [`READ_MAX`] at most), characters being Unicode
/// scalar values, and fewer when that keeps the whole answer within [`READ_BYTES`] as JSON (at
/// least one, so a page always goes on). Its blocks have their source lines when `lines` (a note's
/// or a text file's). When `incomplete` says why the text is less than the file's, every page is
/// `truncated` and says so.
pub fn read_page(
    file: PathForms,
    text: &TextMap,
    lines: bool,
    incomplete: Option<&str>,
    from: Option<u32>,
    max: Option<u32>,
) -> ReadAnswer {
    let all = text.text();
    let from = from.unwrap_or(0);
    let max = max.unwrap_or(READ_DEFAULT).min(READ_MAX);
    let start = byte_after(all, 0, from as usize);
    let end = byte_after(all, start, max as usize);
    let held: Vec<Held> = text
        .blocks()
        .iter()
        .filter(|b| b.end > start && b.start < end)
        .map(|b| {
            let path = text.heading_path_of(b.start, b.start_line);
            Held {
                at: b.start.max(start),
                line: lines.then_some(b.start_line),
                heading: (!path.is_empty()).then(|| capped(&path.join(HEADING_SEPARATOR))),
            }
        })
        .collect();
    let page = |chars: usize| {
        let mut read = page_of(all, start, from, chars, &held);
        if let Some(why) = incomplete {
            read.truncated = true;
            read.incomplete = Some(why.to_owned());
        }
        ReadAnswer {
            file: file.clone(),
            read,
        }
    };
    let longest = all[start..end].chars().count();
    let whole = page(longest);
    if longest <= 1 || fits(&whole) {
        return whole;
    }
    // The longest page that fits: the answer only grows with the page.
    let (mut fitting, mut too_long) = (1, longest);
    while too_long - fitting > 1 {
        let mid = fitting + (too_long - fitting) / 2;
        if fits(&page(mid)) {
            fitting = mid;
        } else {
            too_long = mid;
        }
    }
    page(fitting)
}

/// The page of `all` from byte `start` (the character `from`) of `chars` characters, with the blocks
/// of `held` it holds.
fn page_of(all: &str, start: usize, from: u32, chars: usize, held: &[Held]) -> ReadText {
    let end = byte_after(all, start, chars);
    let mut headings: Vec<String> = Vec::new();
    let mut listed: HashMap<&str, u32> = HashMap::new();
    // Where in the page each block starts, in characters, counted on from the block before.
    let (mut counted, mut offset) = (start, 0);
    let blocks = held
        .iter()
        .take_while(|b| b.at < end)
        .map(|b| {
            offset += all[counted..b.at].chars().count();
            counted = b.at;
            let heading = b.heading.as_deref().map(|path| {
                *listed.entry(path).or_insert_with(|| {
                    headings.push(path.to_owned());
                    u32::try_from(headings.len() - 1).unwrap_or(u32::MAX)
                })
            });
            ReadBlock {
                start: u32::try_from(offset).unwrap_or(u32::MAX),
                line: b.line,
                heading,
            }
        })
        .collect();
    let truncated = end < all.len();
    ReadText {
        text: all[start..end].to_owned(),
        headings,
        blocks,
        truncated,
        next: truncated.then(|| from.saturating_add(u32::try_from(chars).unwrap_or(u32::MAX))),
        incomplete: None,
    }
}

/// Whether `answer` takes at most [`READ_BYTES`] as JSON.
fn fits(answer: &ReadAnswer) -> bool {
    serde_json::to_vec(answer).is_ok_and(|json| json.len() <= READ_BYTES)
}

/// `path`, cut to [`HEADINGS_MAX`] characters and an ellipsis when longer.
fn capped(path: &str) -> String {
    match path.char_indices().nth(HEADINGS_MAX) {
        Some((cut, _)) => format!("{}…", &path[..cut]),
        None => path.to_owned(),
    }
}

/// The byte `chars` characters after byte `from` of `s`, or its end.
fn byte_after(s: &str, from: usize, chars: usize) -> usize {
    s.get(from..)
        .and_then(|rest| rest.char_indices().nth(chars))
        .map_or(s.len(), |(i, _)| from + i)
}
