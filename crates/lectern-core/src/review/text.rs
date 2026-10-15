//! The note's visible text, as the review anchors see it: the text of every leaf block in
//! document order, with each block's source lines and heading.
//!
//! This is the contract with the UI, which builds the same text from the rendered DOM to find a
//! quote on screen. The leaf blocks are paragraphs (in list items, quotes, footnotes and alerts
//! too), headings, code blocks and table rows. A block's text is its text and inline code, with
//! line breaks and table cell boundaries as spaces, or the code of a code block; image alt text,
//! raw HTML and footnote references are left out. Raw HTML the HTML policy shows as text (a tag
//! not on its allowlist) is text like any other. Every block's text is [`normalize`]d, and empty
//! blocks are dropped.
//!
//! A text file's blocks are its paragraphs, as `render_plain` shows them, with no headings.
//!
//! An HTML page's blocks are those its bridge (`assets/page-bridge.js`) reads from the page as it
//! runs, script output included, and the UI hands over ([`PageBlock`]): the text is theirs, and
//! their source lines are a best effort (see [`TextMap::from_blocks`]).

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::Arena;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::fingerprint;
use crate::library::{doc_kind, DocKind};
use crate::render::parse_for_text;
use crate::render::plain::paragraphs;

/// The most text an HTML page's blocks may hold for comments, as [`page_text_len`] measures it:
/// what the bridge reports at most.
pub const MAX_PAGE_TEXT: usize = 2 * 1024 * 1024;

/// The longest and the shortest leading text a page's block is looked for by in its HTML, in
/// characters (see [`leading_cuts`]).
const LEAD_MAX_CHARS: usize = 40;
const LEAD_MIN_CHARS: usize = 8;

/// The most bytes of a page's HTML that placing its blocks searches, all searches together: a
/// block not in the HTML (a script made it) searches the rest of it, so a page of many such blocks
/// would otherwise cost blocks × HTML. Past it, the blocks left take the line of the last one
/// found. A few milliseconds of searching.
const SEARCH_BUDGET: usize = 32 * 1024 * 1024;

/// A block of an HTML page's text, as the page's bridge reports it: its text, whitespace
/// collapsed, and a heading's level.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageBlock {
    pub text: String,
    /// 1 to 6 for a heading; none for any other block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub heading_level: Option<u8>,
}

/// A note's visible text, which its comments are anchored against, and the fingerprint they record
/// of it.
#[derive(Clone, Debug)]
pub struct NoteText {
    pub text: TextMap,
    /// A note's or a text file's: its source's [`fingerprint`]. An HTML page's: its text's, a NUL,
    /// then its source's, as the same file shows other text when a script writes something else
    /// or scripts are off, and an edit can move the page's lines but not its text. Either change
    /// has its comments looked for again by their quote and its context.
    pub fingerprint: String,
}

impl NoteText {
    /// The text of the note named `note`, read as `source` ([`TextMap::of_note`]).
    pub fn of_note(note: &str, source: &str) -> NoteText {
        NoteText {
            text: TextMap::of_note(note, source),
            fingerprint: fingerprint(source),
        }
    }

    /// The text of an HTML page whose bridge reported `blocks`, its HTML being `source`
    /// ([`TextMap::from_blocks`]), fingerprinted by that text and its HTML.
    pub fn of_page(blocks: &[PageBlock], source: &str) -> NoteText {
        let text = TextMap::from_blocks(blocks, source);
        NoteText {
            fingerprint: fingerprint(&format!("{}\0{source}", text.text())),
            text,
        }
    }
}

/// The length of a page's text, its blocks joined by single spaces, in UTF-16 code units, as the
/// bridge and the UI count it.
pub fn page_text_len(blocks: &[PageBlock]) -> usize {
    blocks
        .iter()
        .map(|b| b.text.encode_utf16().count() + 1)
        .sum::<usize>()
        .saturating_sub(1)
}

/// `s` with every run of Unicode whitespace turned into one space, and trimmed.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_normalized(&mut out, s);
    out
}

/// Appends `s` to `out`, normalised as [`normalize`] does.
fn push_normalized(out: &mut String, s: &str) {
    for (i, word) in s.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(word);
    }
}

/// One leaf block of the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Byte offsets of the block's text in [`TextMap::text`], end exclusive.
    pub start: usize,
    pub end: usize,
    /// The block's source lines, 1-based, counted from the top of the file.
    pub start_line: u32,
    pub end_line: u32,
    /// The index in [`TextMap::headings`] of the last heading, by source line, at or before the
    /// block's first line; a heading block points at itself.
    pub heading: Option<usize>,
}

/// A heading of the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingLine {
    pub level: u8,
    /// The heading's visible text, normalised. Empty for a heading with no text.
    pub text: String,
    /// The heading's first source line.
    pub line: u32,
}

/// The note's visible text and where each part of it comes from.
#[derive(Clone, Debug, Default)]
pub struct TextMap {
    text: String,
    blocks: Vec<Block>,
    headings: Vec<HeadingLine>,
    /// A page's: its headings go by the order of its blocks, not by source lines (see
    /// [`TextMap::heading_path_of`]).
    by_order: bool,
}

impl TextMap {
    /// Parses `source` as a render does and collects its visible text.
    pub fn build(source: &str) -> TextMap {
        let arena = Arena::new();
        let root = parse_for_text(&arena, source);
        let mut map = TextMap {
            text: String::with_capacity(source.len()),
            ..TextMap::default()
        };
        let mut raw = String::new();
        for node in root.descendants() {
            let data = node.data();
            let level = match data.value {
                NodeValue::Heading(ref heading) => Some(heading.level),
                NodeValue::Paragraph | NodeValue::TableRow(_) => None,
                NodeValue::CodeBlock(ref code) => {
                    raw.clear();
                    raw.push_str(&code.literal);
                    map.push_block(&raw, data.sourcepos);
                    continue;
                }
                _ => continue,
            };
            raw.clear();
            inline_text(node, &mut raw);
            if let Some(level) = level {
                map.headings.push(HeadingLine {
                    level,
                    text: normalize(&raw),
                    line: line_number(data.sourcepos.start.line),
                });
            }
            map.push_block(&raw, data.sourcepos);
        }
        map.order_headings();
        map
    }

    /// The text of a text file: its paragraphs, as `render_plain` renders them, with no headings.
    pub fn from_plain(source: &str) -> TextMap {
        let mut map = TextMap {
            text: String::with_capacity(source.len()),
            ..TextMap::default()
        };
        for p in paragraphs(source) {
            map.push_lines(&p.text, p.start_line, p.end_line);
        }
        map
    }

    /// The text of an HTML page: the blocks its bridge reported, in order, each [`normalize`]d
    /// and joined by single spaces as the bridge joins them, so offsets into it are offsets into
    /// the page. A block with a heading level from 1 to 6 is a heading, and an empty block is
    /// dropped.
    ///
    /// The lines are a best effort, as the page's text needn't be in its HTML as written (see
    /// `SourceLines`): each block is on the line of `source`, the page's HTML, where its leading
    /// text is first found after where the block before it was. One that isn't found takes the
    /// line of the block before it, or line 1. Each block is under the heading before it in block
    /// order, whatever the lines say, as minified HTML has many headings on one line.
    pub fn from_blocks(blocks: &[PageBlock], source: &str) -> TextMap {
        let mut map = TextMap {
            text: String::with_capacity(blocks.iter().map(|b| b.text.len() + 1).sum()),
            by_order: true,
            ..TextMap::default()
        };
        let mut lines = SourceLines::new(source);
        let mut line = 1;
        let mut text = String::new();
        for block in blocks {
            text.clear();
            push_normalized(&mut text, &block.text);
            if text.is_empty() {
                continue;
            }
            if let Some(found) = lines.find(&text) {
                line = found;
            }
            if let Some(level) = block.heading_level.filter(|l| (1..=6).contains(l)) {
                map.headings.push(HeadingLine {
                    level,
                    text: text.clone(),
                    line,
                });
            }
            map.push_lines(&text, line, line);
            // Pushed in block order, and lines never go back up: already in source order.
            if let Some(block) = map.blocks.last_mut() {
                block.heading = map.headings.len().checked_sub(1);
            }
        }
        map
    }

    /// The text of the note named `note` (its file name or path), read as `source`, by its kind:
    /// a text file's paragraphs ([`TextMap::from_plain`]), else a note's blocks
    /// ([`TextMap::build`]). An HTML page's text is what its bridge reports, which `source`
    /// doesn't hold: it takes [`TextMap::from_blocks`] instead.
    pub fn of_note(note: &str, source: &str) -> TextMap {
        match doc_kind(note) {
            Some(DocKind::Text) => Self::from_plain(source),
            _ => Self::build(source),
        }
    }

    /// Puts the headings in source order and points each block at the last heading above it.
    ///
    /// The blocks and the text stay in document order, which is the page's: comrak moves footnote
    /// definitions to the end, as the page shows them. A heading inside one would otherwise come
    /// after headings below it in the source, and heading paths go by source line.
    fn order_headings(&mut self) {
        self.headings.sort_by_key(|h| h.line);
        for block in &mut self.blocks {
            block.heading = self
                .headings
                .partition_point(|h| h.line <= block.start_line)
                .checked_sub(1);
        }
    }

    /// Appends a block with the text `raw`, normalised, from the source at `pos`, unless that's
    /// empty.
    fn push_block(&mut self, raw: &str, pos: Sourcepos) {
        let start_line = line_number(pos.start.line);
        // An end at column 0 is the end of the line before.
        let end_line = match pos.end.column {
            0 => line_number(pos.end.line.saturating_sub(1)),
            _ => line_number(pos.end.line),
        };
        self.push_lines(raw, start_line, end_line);
    }

    /// Appends a block with the text `raw`, normalised, on source lines `start_line` to
    /// `end_line`, unless that's empty.
    fn push_lines(&mut self, raw: &str, start_line: u32, end_line: u32) {
        let before = self.text.len();
        if before > 0 {
            self.text.push(' ');
        }
        let start = self.text.len();
        push_normalized(&mut self.text, raw);
        if self.text.len() == start {
            self.text.truncate(before);
            return;
        }
        self.blocks.push(Block {
            start,
            end: self.text.len(),
            start_line,
            end_line: end_line.max(start_line),
            heading: None,
        });
    }

    /// The blocks' texts, joined by single spaces.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The leaf blocks, in document order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The headings, in source order.
    pub fn headings(&self) -> &[HeadingLine] {
        &self.headings
    }

    /// The first source line with text: the lowest first line of any block. `None` without text.
    pub fn top_line(&self) -> Option<u32> {
        self.blocks.iter().map(|b| b.start_line).min()
    }

    pub fn block_text(&self, b: &Block) -> &str {
        &self.text[b.start..b.end]
    }

    /// The index of the block holding the byte `at` of the text. A separator space belongs to
    /// the block after it, and an offset past the end to the last block.
    fn block_at(&self, at: usize) -> usize {
        self.blocks
            .partition_point(|b| b.end <= at)
            .min(self.blocks.len().saturating_sub(1))
    }

    /// The source lines of the text from byte `start` to byte `end`: the lowest first line and the
    /// highest last line of every block it touches, from the block holding `start` to the one
    /// holding `end - 1`. Footnotes come last in the text but early in the source, so the end
    /// blocks alone don't bound the lines. `(0, 0)` if the note has no text.
    pub fn lines_of(&self, start: usize, end: usize) -> (u32, u32) {
        if self.blocks.is_empty() {
            return (0, 0);
        }
        let first = self.block_at(start);
        let last = self.block_at(end.saturating_sub(1).max(start));
        self.blocks[first..=last]
            .iter()
            .fold((u32::MAX, 0), |(low, high), b| {
                (low.min(b.start_line), high.max(b.end_line))
            })
    }

    /// The headings a reader is under at `line`, outermost first: the last heading at or before
    /// the line, and each heading above it of a lower level. Headings without text are left out.
    pub fn heading_path_at(&self, line: u32) -> Vec<String> {
        let count = self.headings.partition_point(|h| h.line <= line);
        let mut stack = Vec::new();
        for h in &self.headings[..count] {
            push_heading(&mut stack, h);
        }
        path_of(&stack)
    }

    /// The headings a reader is under at the text's byte `at`, on source line `line`: a note's by
    /// [`TextMap::heading_path_at`] the line; a page's by the order of its blocks, from the heading
    /// the block holding `at` is under.
    pub fn heading_path_of(&self, at: usize, line: u32) -> Vec<String> {
        if !self.by_order {
            return self.heading_path_at(line);
        }
        let under = self
            .blocks
            .get(self.block_at(at))
            .and_then(|b| b.heading)
            .map_or(0, |h| h + 1);
        let mut stack = Vec::new();
        for h in &self.headings[..under] {
            push_heading(&mut stack, h);
        }
        path_of(&stack)
    }

    /// Where heading `h` is in the text: the start of its own block, the first under it. `None`
    /// for a heading without text, which has none.
    pub(crate) fn heading_start(&self, h: usize) -> Option<usize> {
        self.blocks
            .iter()
            .find(|b| b.heading == Some(h))
            .map(|b| b.start)
    }

    /// The heading path of every heading, by index: `heading_path_at(headings[i].line)` for each
    /// `i`, built in one pass.
    pub(crate) fn heading_paths(&self) -> Vec<Vec<String>> {
        let mut stack: Vec<&HeadingLine> = Vec::new();
        self.headings
            .iter()
            .map(|h| {
                push_heading(&mut stack, h);
                path_of(&stack)
            })
            .collect()
    }

    /// Up to `chars` characters of the text ending at byte `at`.
    pub fn before(&self, at: usize, chars: usize) -> String {
        if chars == 0 {
            return String::new();
        }
        let head = &self.text[..at];
        let from = head
            .char_indices()
            .rev()
            .nth(chars - 1)
            .map_or(0, |(i, _)| i);
        head[from..].to_owned()
    }

    /// Up to `chars` characters of the text starting at byte `at`.
    pub fn after(&self, at: usize, chars: usize) -> String {
        self.text[at..].chars().take(chars).collect()
    }
}

/// Opens `h`, closing every open heading of its level or deeper.
fn push_heading<'h>(stack: &mut Vec<&'h HeadingLine>, h: &'h HeadingLine) {
    while stack.last().is_some_and(|top| top.level >= h.level) {
        stack.pop();
    }
    stack.push(h);
}

/// The texts of the open headings, outermost first, without the empty ones.
fn path_of(stack: &[&HeadingLine]) -> Vec<String> {
    stack
        .iter()
        .filter(|h| !h.text.is_empty())
        .map(|h| h.text.clone())
        .collect()
}

/// The text of a paragraph, heading or table row: text and inline code, with a space for each
/// line break and before each table cell. Images, raw HTML and footnote references add nothing.
fn inline_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    let mut image_depth = 0usize;
    for edge in node.traverse() {
        match edge {
            NodeEdge::Start(n) => match n.data().value {
                NodeValue::Image(_) => image_depth += 1,
                _ if image_depth > 0 => {}
                NodeValue::Text(ref t) => out.push_str(t),
                NodeValue::Code(ref code) => out.push_str(&code.literal),
                NodeValue::SoftBreak | NodeValue::LineBreak | NodeValue::TableCell => {
                    out.push(' ');
                }
                _ => {}
            },
            NodeEdge::End(n) => {
                if matches!(n.data().value, NodeValue::Image(_)) {
                    image_depth -= 1;
                }
            }
        }
    }
}

fn line_number(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

/// Where an HTML page's blocks are in its source, looked for one after another, in the blocks'
/// order (see [`TextMap::from_blocks`]). The page's text is its body's, so the search starts at
/// the `<body` tag when there is one; `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;` count as the
/// characters they stand for. Text a script makes, or that markup or a line break splits, isn't
/// found as it shows.
struct SourceLines {
    /// The source from its `<body` tag, or all of it, entities decoded. Decoding keeps every line
    /// break, so lines are counted in it.
    html: String,
    /// Where the next block is looked for from: just after the last one found.
    from: usize,
    /// How far into `html` lines have been counted, and the line there.
    counted: usize,
    line: u32,
    /// The bytes of `html` searched so far, never more than [`SEARCH_BUDGET`].
    scanned: usize,
}

impl SourceLines {
    fn new(source: &str) -> Self {
        let body = body_start(source);
        SourceLines {
            html: decode_entities(&source[body..]),
            from: 0,
            counted: 0,
            line: line_number(newlines(&source[..body]).saturating_add(1)),
            scanned: 0,
        }
    }

    /// The line where the block with the (normalised) text `text` is: the first place after the
    /// last block found where its leading text (see [`leading_cuts`]) is, taking the longest of
    /// its prefixes there. `None` when it isn't found, or isn't within what's left of
    /// [`SEARCH_BUDGET`].
    fn find(&mut self, text: &str) -> Option<u32> {
        let cuts = leading_cuts(text);
        let (&shortest, &longest) = (cuts.first()?, cuts.last()?);
        let left = SEARCH_BUDGET.saturating_sub(self.scanned);
        let end = self
            .html
            .floor_char_boundary(self.from.saturating_add(left).min(self.html.len()));
        let rest = self.html.get(self.from..end)?;
        let Some(found) = rest.find(&text[..shortest]) else {
            self.scanned += rest.len();
            return None;
        };
        self.scanned += found + shortest;
        let at = self.from + found;
        let same = common_prefix_bytes(&self.html[at..], &text[..longest]);
        let matched = cuts
            .iter()
            .rev()
            .copied()
            .find(|&cut| cut <= same)
            .unwrap_or(shortest);
        self.from = at + matched;
        self.line = self
            .line
            .saturating_add(line_number(newlines(&self.html[self.counted..at])));
        self.counted = at;
        Some(self.line)
    }
}

/// Where the page's `<body` tag starts in `source` (in any letter case), or 0 without one.
fn body_start(source: &str) -> usize {
    let bytes = source.as_bytes();
    source
        .match_indices('<')
        .map(|(at, _)| at)
        .find(|&at| {
            bytes
                .get(at + 1..at + 5)
                .is_some_and(|name| name.eq_ignore_ascii_case(b"body"))
                && matches!(
                    bytes.get(at + 5),
                    Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>')
                )
        })
        .unwrap_or(0)
}

/// `html` with `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;` read as the characters they stand
/// for, once: `&amp;lt;` is `&lt;`.
fn decode_entities(html: &str) -> String {
    const ENTITIES: [(&str, char); 5] = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&#39;", '\''),
    ];
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match ENTITIES.iter().find(|(name, _)| tail.starts_with(name)) {
            Some(&(name, ch)) => {
                out.push(ch);
                rest = &tail[name.len()..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The prefixes of a page block's (normalised) text that it's looked for by in the page's HTML,
/// as byte lengths, shortest first: those of at most 40 characters ending at a word boundary
/// (before a space, or at the end of the text), of at least 8 characters unless the whole text is
/// shorter. Text with no word boundary from its 8th to its 40th character (one long word, or a
/// language written without spaces) is looked for by its first 40 characters.
fn leading_cuts(text: &str) -> Vec<usize> {
    let mut cuts = Vec::new();
    for (chars, (at, c)) in text.char_indices().enumerate() {
        if c == ' ' && chars >= LEAD_MIN_CHARS {
            cuts.push(at);
        }
        if chars == LEAD_MAX_CHARS {
            // Longer than 40 characters: the whole text is too long to look for.
            if cuts.is_empty() {
                cuts.push(at);
            }
            return cuts;
        }
    }
    cuts.push(text.len());
    cuts
}

/// How many bytes `a` and `b` share at their starts.
fn common_prefix_bytes(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// The line breaks in `s`.
fn newlines(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::{SourceLines, SEARCH_BUDGET};

    /// Each block a script made is nowhere in the page's HTML, and looking for it searches the rest
    /// of the HTML. All the searching for one page stays within a budget, however many such blocks
    /// there are; past it, the blocks left take the line of the last one found.
    #[test]
    fn searching_a_page_stays_within_its_budget() {
        // About 2 MiB of HTML, nearly all a script; then 5,000 blocks the script made.
        let mut source = String::from("<body>\n<p>Intro paragraph.</p>\n<script>\n");
        while source.len() < 2 * 1024 * 1024 {
            source.push_str("var data = 'abcdefghijklmnopqrstuvwxyz0123456789';\n");
        }
        source.push_str("</script>\n<p>Closing paragraph.</p>\n");
        let mut lines = SourceLines::new(&source);
        assert_eq!(lines.find("Intro paragraph."), Some(2));
        for i in 0..5000 {
            assert_eq!(lines.find(&format!("Row {i} made by the script")), None);
        }
        assert!(lines.scanned <= SEARCH_BUDGET, "{} bytes", lines.scanned);
        // Spent: nothing more is searched, even what is there.
        assert_eq!(lines.find("Closing paragraph."), None);
        assert!(lines.scanned <= SEARCH_BUDGET, "{} bytes", lines.scanned);
    }

    /// Within the budget, a block is found wherever it is.
    #[test]
    fn a_page_within_the_budget_is_searched_to_its_end() {
        let mut source = String::from("<body>\n<p>Intro paragraph.</p>\n");
        source.push_str(&"<p>Filler.</p>\n".repeat(10_000));
        source.push_str("<p>Closing paragraph.</p>\n");
        let mut lines = SourceLines::new(&source);
        assert_eq!(lines.find("Missing paragraph."), None);
        assert_eq!(lines.find("Closing paragraph."), Some(10_003));
    }
}
