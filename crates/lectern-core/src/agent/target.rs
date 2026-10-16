//! Where an agent's target is in the file it names, and which targets a file of each kind takes.
//!
//! - **Notes and text files:** core finds the target ([`resolve_target`]) in the text as shown,
//!   the text the review comments are anchored against: a heading by its text or its path, a
//!   source line, or a quoted passage, told apart from others like it by the text just before and
//!   after it. A quote that isn't in that text (an agent quotes what it read, Markdown marks and
//!   all) is looked for in the source instead, where its lines stand in for it. The window's UI
//!   gets the [`TextPlace`]: the lines, and where the passage is in the text when that is known.
//! - **HTML pages:** the window's UI finds a heading or a quote in what the page's bridge reports,
//!   as only it has the page's text once its scripts ran.
//! - **PDFs:** only a page.

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::request::Target;
use crate::ipc::PathForms;
use crate::library::DocKind;
use crate::review::anchor::{common_prefix, common_suffix, occurrences};
use crate::review::text::{normalize, TextMap};
use crate::review::view::utf16_offsets;

/// Where a target is in a note or a text file, as core found it: its source lines, and where it
/// is in the text as shown (the text the UI builds from the document, as the review comments do),
/// from `text_start` up to `text_end`, counted in UTF-16 code units. Only the lines are known for
/// a quote found in the source, or a line with no text.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextPlace {
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text_end: Option<u32>,
}

/// Where an agent's `show` takes the reader once its file has opened in a window that was still
/// starting: its target, and for a note or a text file, where core found it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShowAt {
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub place: Option<TextPlace>,
}

/// `highlight`'s answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HighlightAnswer {
    /// The file, in both forms.
    pub file: PathForms,
    /// The label of the window showing it.
    pub window: String,
    /// How many of the agent's highlights the window shows now.
    pub highlights: u32,
}

/// Whether a file of kind `kind`, named `file`, takes `target`: a note or a text file a heading,
/// a line or a quote; an HTML page a heading or a quote, as its lines aren't shown; a PDF only a
/// page. A comment thread is any file's that takes comments, which a PDF doesn't.
pub fn check_target(kind: DocKind, target: &Target, file: &str) -> Result<(), String> {
    match (kind, target) {
        (DocKind::Pdf, Target::Page { .. }) => Ok(()),
        (DocKind::Pdf, _) => Err(format!("{file} is a PDF: give a page target.")),
        (_, Target::Comment { .. }) => Ok(()),
        (DocKind::Html, Target::Heading { .. } | Target::Quote { .. }) => Ok(()),
        (DocKind::Html, _) => Err(format!(
            "{file} is an HTML page: give a heading or a quote."
        )),
        (_, Target::Page { .. }) => {
            Err("A page target is for PDFs; give a heading, a line or a quote.".to_owned())
        }
        _ => Ok(()),
    }
}

/// Where `target` is in the note or text file named `file`, read as `source`, whose text as shown
/// is `text`; an error, a sentence for the agent, when it isn't there.
///
/// - **A heading:** its text, or a path of headings each under the one before (`Plan › Rollout`,
///   with headings between left out), in any letter case, Markdown marks and a leading `#` aside.
///   Of those that match, the one whose own path is nearest the target's length, then the first.
/// - **A line:** a line of the source; the block that holds it, when one does.
/// - **A quote:** the passage in the text, its whitespace aside, and of two or more, the one the
///   text before and after it agrees with most (`prefix`, `suffix`), then the first. Else the
///   passage in the source, the same way, by its lines alone.
pub fn resolve_target(
    text: &TextMap,
    source: &str,
    file: &str,
    target: &Target,
) -> Result<TextPlace, String> {
    match target {
        Target::Heading { heading } => find_heading(text, heading)
            .ok_or_else(|| format!("There's no heading “{}” in {file}.", heading.trim())),
        Target::Line { line } => find_line(text, source, *line).ok_or_else(|| {
            format!(
                "{file} has {} lines; line {line} is past its end.",
                source.lines().count()
            )
        }),
        Target::Quote {
            quote,
            prefix,
            suffix,
        } => {
            let wanted = Context {
                needle: normalize(quote),
                prefix: normalize(prefix.as_deref().unwrap_or_default()),
                suffix: normalize(suffix.as_deref().unwrap_or_default()),
            };
            find_quote(text, source, &wanted)
                .ok_or_else(|| format!("That passage isn't in {file}."))
        }
        Target::Page { .. } => {
            Err("A page target is for PDFs; give a heading, a line or a quote.".to_owned())
        }
        Target::Comment { .. } => Err("A comment thread isn't a passage of the text.".to_owned()),
    }
}

/// The place of the text from byte `start` to byte `end` of `text`.
fn text_place(text: &TextMap, start: usize, end: usize) -> TextPlace {
    let (start_line, end_line) = text.lines_of(start, end);
    let units = utf16_offsets(text.text(), &[start, end]);
    TextPlace {
        start_line,
        end_line,
        text_start: units.first().copied(),
        text_end: units.get(1).copied(),
    }
}

/// A heading's text as a target is matched against it: without Markdown's marks for emphasis and
/// code or a leading `#`, its whitespace normalised, in lower case.
fn heading_key(heading: &str) -> String {
    let unmarked: String = heading
        .trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    normalize(&unmarked).to_lowercase()
}

/// The heading `wanted` names (see [`resolve_target`]), as the place of its own block.
fn find_heading(text: &TextMap, wanted: &str) -> Option<TextPlace> {
    let segments: Vec<String> = wanted
        .split('›')
        .map(heading_key)
        .filter(|s| !s.is_empty())
        .collect();
    let (last, over) = segments.split_last()?;
    let headings = text.headings();
    let (_, found) = text
        .heading_paths()
        .iter()
        .enumerate()
        .filter(|(i, _)| heading_key(&headings[*i].text) == *last)
        .filter_map(|(i, path)| {
            let keys: Vec<String> = path.iter().map(|h| heading_key(h)).collect();
            let above = keys.split_last().map_or(&[][..], |(_, above)| above);
            under_in_order(over, above).then(|| (keys.len().saturating_sub(segments.len()), i))
        })
        .min()?;
    let line = headings[found].line;
    let own = text
        .blocks()
        .iter()
        .filter(|b| b.heading == Some(found))
        .min_by_key(|b| b.start_line != line)?;
    let mut place = text_place(text, own.start, own.end);
    place.start_line = line;
    Some(place)
}

/// Whether each of `wanted` is in `path`, in the same order, others between them or not.
fn under_in_order(wanted: &[String], path: &[String]) -> bool {
    let mut path = path.iter();
    wanted.iter().all(|w| path.any(|p| p == w))
}

/// Line `line` of `source`: its block's place in `text` when a block holds it, else the line
/// alone; none past the source's last line.
fn find_line(text: &TextMap, source: &str, line: u32) -> Option<TextPlace> {
    if line == 0 || line as usize > source.lines().count() {
        return None;
    }
    let held = text
        .blocks()
        .iter()
        .find(|b| (b.start_line..=b.end_line).contains(&line));
    Some(match held {
        Some(block) => TextPlace {
            start_line: line,
            end_line: line,
            ..text_place(text, block.start, block.end)
        },
        None => TextPlace {
            start_line: line,
            end_line: line,
            text_start: None,
            text_end: None,
        },
    })
}

/// A quote, with the text just before and after it, each normalised.
struct Context {
    needle: String,
    prefix: String,
    suffix: String,
}

impl Context {
    /// The occurrence of the quote in `hay` that the text before and after it agrees with most
    /// (the spaces next to it aside), then the first: its byte range.
    fn best_in(&self, hay: &str) -> Option<(usize, usize)> {
        occurrences(hay, &self.needle)
            .map(|s| {
                let e = s + self.needle.len();
                let agreement = common_suffix(hay[..s].trim_end(), &self.prefix)
                    + common_prefix(hay[e..].trim_start(), &self.suffix);
                ((Reverse(agreement), s), (s, e))
            })
            .min_by_key(|&(key, _)| key)
            .map(|(_, span)| span)
    }
}

/// The quote in the text as shown; else in the source, by its lines.
fn find_quote(text: &TextMap, source: &str, wanted: &Context) -> Option<TextPlace> {
    if let Some((start, end)) = wanted.best_in(text.text()) {
        return Some(text_place(text, start, end));
    }
    let (normalized, starts) = normalized_lines(source);
    let (start, end) = wanted.best_in(&normalized)?;
    let line_at =
        |at: usize| u32::try_from(starts.partition_point(|&s| s <= at)).unwrap_or(u32::MAX);
    Some(TextPlace {
        start_line: line_at(start),
        end_line: line_at(end.saturating_sub(1).max(start)),
        text_start: None,
        text_end: None,
    })
}

/// `source` normalised as [`normalize`] does, line breaks and all, with where each of its lines
/// starts in that text: a line's first character is there or just after (the space joining it to
/// the line before).
fn normalized_lines(source: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(source.len());
    let mut starts = Vec::new();
    let mut space = false;
    for line in source.split('\n') {
        starts.push(out.len());
        for c in line.chars() {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
        space = true;
    }
    (out, starts)
}
