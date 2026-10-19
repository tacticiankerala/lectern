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
//! - **PDFs:** only a page, which the standard `#page=N` after a PDF's path names too
//!   ([`pdf_suffix_page`]).

use std::cmp::Reverse;

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::request::Target;
use super::status::HEADING_SEPARATOR;
use super::tools::number;
use crate::ipc::PathForms;
use crate::library::DocKind;
use crate::render::heading_ids;
use crate::render::slug::slugify;
use crate::review::anchor::{common_prefix, common_suffix, occurrences};
use crate::review::text::{normalize, LineText, TextMap};
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
    /// Where `text_start` is in the text of the block holding it (its paragraph, code block or
    /// list item), in UTF-16 code units: the window finds the passage in that block alone,
    /// without indexing the whole document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub block_offset: Option<u32>,
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
/// a line or a quote; an HTML page a heading or a quote, as its lines aren't shown, or the anchor
/// after its path; a PDF only a page. A comment thread is any file's that takes comments, which a
/// PDF doesn't.
pub fn check_target(kind: DocKind, target: &Target, file: &str) -> Result<(), String> {
    match (kind, target) {
        (DocKind::Pdf, Target::Page { .. }) => Ok(()),
        (DocKind::Pdf, _) => Err(format!("{file} is a PDF: give a page target.")),
        (_, Target::Comment { .. }) => Ok(()),
        (DocKind::Html, Target::Heading { .. } | Target::Quote { .. } | Target::Anchor { .. }) => {
            Ok(())
        }
        (_, Target::Anchor { anchor }) => Err(not_a_page_anchor(anchor, file)),
        (DocKind::Html, _) => Err(format!(
            "{file} is an HTML page: give a heading or a quote."
        )),
        (_, Target::Page { .. }) => {
            Err("A page target is for PDFs; give a heading, a line or a quote.".to_owned())
        }
        _ => Ok(()),
    }
}

/// The page target what follows the path of the PDF named `file` names, when a `show` gives no
/// target: the standard `#page=N` (`page=` in any letter case), `N` checked as a page target's
/// `page` is, with its message. `fragment` is what follows the `#`; none when a line follows the
/// path (`:12`, `#L12`). Anything else is an error, a sentence for the agent saying what a PDF
/// takes.
pub fn pdf_suffix_page(fragment: Option<&str>, file: &str) -> Result<Target, String> {
    let page = fragment
        .and_then(|fragment| fragment.split_at_checked(PAGE_FRAGMENT.len()))
        .filter(|(key, _)| key.eq_ignore_ascii_case(PAGE_FRAGMENT))
        .map(|(_, page)| page)
        .ok_or_else(|| {
            format!("{file} is a PDF: give #page=N after its path, or a page target.")
        })?;
    // Read as a page target's argument would be: a number, else the text, which isn't one.
    let value = page
        .parse::<u64>()
        .map_or_else(|_| Value::from(page), Value::from);
    number(&value, "page", 1, u32::MAX).map(|page| Target::Page { page })
}

/// What starts a PDF's `#page=N`, in any letter case.
const PAGE_FRAGMENT: &str = "page=";

/// Where `target` is in the note or text file named `file`, read as `source`, whose text as shown
/// is `text`; an error, a sentence for the agent, when it isn't there.
///
/// - **A heading:** its text, or a path of headings each under the one before (`Plan › Rollout`,
///   with headings between left out), in any letter case, Markdown marks and a leading `#` aside.
///   Of those that match, the one whose own path is nearest the target's length, then the first.
/// - **A line:** a line of the source: its own text, when it is one line of a longer paragraph or
///   code block; else the block that holds it, when one does ([`TextMap::line_text`]).
/// - **A quote:** the passage in the text, its whitespace aside, and of two or more, the one the
///   text before and after it agrees with most (`prefix`, `suffix`), then the first. Else the
///   passage in the source, the same way, by its lines alone.
pub fn resolve_target(
    text: &TextMap,
    source: &str,
    file: &str,
    target: &Target,
) -> Result<TextPlace, String> {
    find_target(text, source, file, target).map(|found| found.place(text))
}

/// Where a target is in a note or a text file: its source lines, and where it is in the text as
/// shown, in bytes, when that is known.
pub(crate) struct Found {
    pub start_line: u32,
    pub end_line: u32,
    pub span: Option<(usize, usize)>,
}

impl Found {
    /// The passage from byte `start` to byte `end` of `text`, on the lines that hold it.
    fn at(text: &TextMap, start: usize, end: usize) -> Self {
        let (start_line, end_line) = text.lines_of(start, end);
        Self {
            start_line,
            end_line,
            span: Some((start, end)),
        }
    }

    /// As the window's UI is sent it: its offsets in UTF-16 code units, and where it starts in
    /// its own block's text.
    fn place(&self, text: &TextMap) -> TextPlace {
        let units = self
            .span
            .map(|(start, end)| utf16_offsets(text.text(), &[start, end]))
            .unwrap_or_default();
        TextPlace {
            start_line: self.start_line,
            end_line: self.end_line,
            text_start: units.first().copied(),
            text_end: units.get(1).copied(),
            block_offset: self.span.and_then(|(start, _)| block_offset(text, start)),
        }
    }
}

/// Where byte `start` of `text` is in the text of the block holding it, in UTF-16 code units;
/// none when it is in no block.
fn block_offset(text: &TextMap, start: usize) -> Option<u32> {
    let blocks = text.blocks();
    let block = &blocks[blocks
        .partition_point(|b| b.start <= start)
        .checked_sub(1)?];
    let within = text
        .text()
        .get(block.start..start)
        .filter(|_| start <= block.end)?;
    u32::try_from(within.encode_utf16().count()).ok()
}

/// Where `target` is, as [`resolve_target`] finds it, in bytes of the text.
pub(crate) fn find_target(
    text: &TextMap,
    source: &str,
    file: &str,
    target: &Target,
) -> Result<Found, String> {
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
        Target::Anchor { anchor } => Err(not_a_page_anchor(anchor, file)),
    }
}

/// Why the file named `file`, not an HTML page, can't take `#anchor` as a target: a note's anchor
/// is found as its heading (`show_anchor`).
fn not_a_page_anchor(anchor: &str, file: &str) -> String {
    format!("Only an HTML page takes #{anchor} as a target, not {file}.")
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
fn find_heading(text: &TextMap, wanted: &str) -> Option<Found> {
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
    heading_place(text, found)
}

/// Where heading `h` of `text` is: its own block's place, on its first line.
fn heading_place(text: &TextMap, h: usize) -> Option<Found> {
    let line = text.headings().get(h)?.line;
    let own = text
        .blocks()
        .iter()
        .filter(|b| b.heading == Some(h))
        .min_by_key(|b| b.start_line != line)?;
    Some(Found {
        start_line: line,
        ..Found::at(text, own.start, own.end)
    })
}

/// Where a `show` of the note named `file`, read as `source`, whose text as shown is `text`, goes
/// for the `#anchor` after its path, when no target is given: the heading the anchor names, found
/// as the reader's own links find one, by its `id` (the anchor as written, else its slug). Its
/// target is the heading by its path. An error, a sentence for the agent naming the anchor and the
/// file, when no heading has it: a text file has none.
pub fn show_anchor(
    text: &TextMap,
    source: &str,
    file: &str,
    anchor: &str,
) -> Result<ShowAt, String> {
    let missing = || format!("There's no heading #{anchor} in {file}.");
    // A text file's map has no headings, and its source isn't Markdown to look for them in.
    if text.headings().is_empty() {
        return Err(missing());
    }
    let ids = heading_ids(source);
    let decoded = percent_decode_str(anchor).decode_utf8_lossy();
    let line = [anchor.to_owned(), decoded.to_string(), slugify(&decoded)]
        .iter()
        .find_map(|wanted| ids.iter().find(|(_, id)| !id.is_empty() && id == wanted))
        .map(|&(line, _)| line)
        .ok_or_else(missing)?;
    let h = text
        .headings()
        .iter()
        .position(|h| h.line == line)
        .ok_or_else(missing)?;
    let found = heading_place(text, h).ok_or_else(missing)?;
    let path = text.heading_paths().swap_remove(h);
    Ok(ShowAt {
        target: Target::Heading {
            heading: path.join(HEADING_SEPARATOR),
        },
        place: Some(found.place(text)),
    })
}

/// Whether each of `wanted` is in `path`, in the same order, others between them or not.
fn under_in_order(wanted: &[String], path: &[String]) -> bool {
    let mut path = path.iter();
    wanted.iter().all(|w| path.any(|p| p == w))
}

/// Line `line` of `source`: where its text is in `text` ([`TextMap::line_text`]), its own when
/// it is one line of a longer block; the line alone when it has none; none past the source's last
/// line.
fn find_line(text: &TextMap, source: &str, line: u32) -> Option<Found> {
    if line == 0 || line as usize > source.lines().count() {
        return None;
    }
    let span = match text.line_text(line) {
        LineText::Block(start, end) | LineText::Own(start, end) => Some((start, end)),
        LineText::Empty => None,
    };
    Some(Found {
        start_line: line,
        end_line: line,
        span,
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
fn find_quote(text: &TextMap, source: &str, wanted: &Context) -> Option<Found> {
    if let Some((start, end)) = wanted.best_in(text.text()) {
        return Some(Found::at(text, start, end));
    }
    let (normalized, starts) = normalized_lines(source);
    let (start, end) = wanted.best_in(&normalized)?;
    let line_at =
        |at: usize| u32::try_from(starts.partition_point(|&s| s <= at)).unwrap_or(u32::MAX);
    Some(Found {
        start_line: line_at(start),
        end_line: line_at(end.saturating_sub(1).max(start)),
        span: None,
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
