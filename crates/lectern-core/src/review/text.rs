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
use comrak::nodes::{AstNode, NodeCodeBlock, NodeValue, Sourcepos};
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

/// How far past the first place a block's shortest leading text is a longer one is looked for, in
/// bytes: a block isn't sought across the whole page for a few more words.
const LOOK_AHEAD: usize = 64 * 1024;

/// The most bytes of a page's HTML that placing its blocks searches, all searches together: a
/// block not in the HTML (a script made it) searches the rest of it, so a page of many such blocks
/// would otherwise cost blocks × HTML. Past it, the blocks left take the line of the last one
/// found. A few milliseconds of searching.
const SEARCH_BUDGET: usize = 32 * 1024 * 1024;

/// `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;`, and the characters they stand for: the entities
/// a page's HTML ([`decode_entities`]) and a note's source lines ([`Decoded`]) are read with.
const ENTITIES: [(&str, char); 5] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#39;", '\''),
];

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
    /// The text of the note named `note`, read as `source` ([`TextMap::of_note`]); `None` for an
    /// HTML page, whose text is what its bridge reports ([`NoteText::of_page`]).
    pub fn of_note(note: &str, source: &str) -> Option<NoteText> {
        Some(NoteText {
            text: TextMap::of_note(note, source)?,
            fingerprint: fingerprint(source),
        })
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
    /// Where each source line's words are, in each paragraph and code block over more lines whose
    /// lines are known, by block, then line: what makes [`TextMap::lines_of`] and
    /// [`TextMap::line_text`] go by lines, not blocks. Built with the text, from the one parse,
    /// and checked against the source before it is trusted ([`TextMap::push_line_spans`]).
    line_spans: Vec<LineSpan>,
}

/// The words of one source line of a block over more lines: from the first's start to the last's
/// end, in bytes of the text. Lines without a word have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LineSpan {
    /// The block's index.
    block: usize,
    line: u32,
    start: usize,
    end: usize,
}

/// Where a source line's own text is in a note's or a text file's text ([`TextMap::line_text`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineText {
    /// The line is its block's, whole: a block on that line alone, a heading, an HTML page's
    /// block, or a block whose lines can't be told apart. From byte to byte of the text.
    Block(usize, usize),
    /// One line of a paragraph or a code block that runs over more lines: its own text, from byte
    /// to byte.
    Own(usize, usize),
    /// No text is on the line: a blank one, a code block's fence, or one no block holds.
    Empty,
}

impl TextMap {
    /// Parses `source` as a render does and collects its visible text.
    pub fn build(source: &str) -> TextMap {
        let arena = Arena::new();
        let root = parse_for_text(&arena, source);
        let by_line = SourceByLine::new(source);
        let mut map = TextMap {
            text: String::with_capacity(source.len()),
            ..TextMap::default()
        };
        let mut raw = String::new();
        for node in root.descendants() {
            let data = node.data();
            let (first, last) = block_lines(data.sourcepos);
            let level = match data.value {
                NodeValue::Heading(ref heading) => Some(heading.level),
                NodeValue::Paragraph | NodeValue::TableRow(_) => None,
                NodeValue::CodeBlock(ref code) => {
                    raw.clear();
                    raw.push_str(&code.literal);
                    let lines = (first < last)
                        .then(|| code_line_map(code, first, last))
                        .flatten();
                    map.push_by_lines(&raw, (first, last), lines.as_ref(), &by_line);
                    continue;
                }
                _ => continue,
            };
            raw.clear();
            // A paragraph over more lines is collected by its lines too.
            let by_lines = matches!(data.value, NodeValue::Paragraph) && first < last;
            let mut starts = by_lines.then(|| LineStarts::new(first, last));
            inline_text(node, &mut raw, starts.as_mut());
            if let Some(level) = level {
                map.headings.push(HeadingLine {
                    level,
                    text: normalize(&raw),
                    line: line_number(data.sourcepos.start.line),
                });
            }
            let lines = starts.and_then(LineStarts::known);
            map.push_by_lines(&raw, (first, last), lines.as_ref(), &by_line);
        }
        map.order_headings();
        map
    }

    /// The text of a text file: its paragraphs, as `render_plain` renders them, with no headings.
    pub fn from_plain(source: &str) -> TextMap {
        let by_line = SourceByLine::new(source);
        let mut map = TextMap {
            text: String::with_capacity(source.len()),
            ..TextMap::default()
        };
        for p in paragraphs(source) {
            let lines = (p.start_line < p.end_line).then(|| {
                let mut at = 0;
                let starts = (p.start_line..)
                    .zip(p.text.split('\n'))
                    .map(|(line, text)| {
                        let start = (line, at);
                        at += text.len() + 1;
                        start
                    })
                    .collect();
                LineMap {
                    starts,
                    cuts: Vec::new(),
                    verbatim: vec![(0, p.text.len())],
                }
            });
            map.push_by_lines(
                &p.text,
                (p.start_line, p.end_line),
                lines.as_ref(),
                &by_line,
            );
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
            if let Some(found) = lines.find(&text, block.heading_level.is_some()) {
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
    /// ([`TextMap::build`]). `None` for an HTML page: its text is what its bridge reports, which
    /// `source` doesn't hold, so it takes [`TextMap::from_blocks`] instead.
    pub fn of_note(note: &str, source: &str) -> Option<TextMap> {
        match doc_kind(note) {
            Some(DocKind::Html) => None,
            Some(DocKind::Text) => Some(Self::from_plain(source)),
            _ => Some(Self::build(source)),
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

    /// Appends a block with the text `raw`, normalised, on source lines `lines`, unless that's
    /// empty; and where each of its lines' words are, when its line map is given and holds
    /// against `source` ([`TextMap::push_line_spans`]).
    fn push_by_lines(
        &mut self,
        raw: &str,
        lines: (u32, u32),
        map: Option<&LineMap>,
        source: &SourceByLine,
    ) {
        let blocks = self.blocks.len();
        self.push_lines(raw, lines.0, lines.1);
        if let Some(map) = map.filter(|_| self.blocks.len() > blocks) {
            self.push_line_spans(raw, map, source);
        }
    }

    /// Notes where each line's words are in the last block, whose text is `raw` normalised and
    /// whose lines `map` gives. The map is trusted only as far as it holds against `source`: each
    /// word must be on the source line the map puts it on, after the word before it
    /// ([`LineCheck`]). Should a word not be, or a line start inside a word (raw HTML over a line
    /// break, with no space shown between the word's halves), or the words not add up to the
    /// block's text, none are noted: its lines aren't known, and it is taken whole.
    fn push_line_spans(&mut self, raw: &str, map: &LineMap, source: &SourceByLine) {
        let Some(b) = self.blocks.len().checked_sub(1) else {
            return;
        };
        let (base, end) = (self.blocks[b].start, self.blocks[b].end);
        let kept = self.line_spans.len();
        let starts = &map.starts;
        let mut check = LineCheck::new(map, source);
        let mut mark = 0;
        // How long the normalised text is so far: its words, a space between each two.
        let mut out = 0;
        for (at, word) in words(raw) {
            while starts.get(mark + 1).is_some_and(|&(_, start)| start <= at) {
                mark += 1;
            }
            let Some(&(line, _)) = starts.get(mark) else {
                break;
            };
            let onto_next = starts
                .get(mark + 1)
                .is_some_and(|&(_, start)| start < at + word.len());
            if onto_next || !check.holds(raw, at, word, line) {
                // The word runs onto the next line, so neither line is its own; or it isn't on
                // the line counted for it, so the count is off.
                self.line_spans.truncate(kept);
                return;
            }
            let begins = if out == 0 { 0 } else { out + 1 };
            out = begins + word.len();
            let ours = self.line_spans.len() > kept;
            match self.line_spans.last_mut() {
                Some(span) if ours && span.line == line => span.end = base + out,
                _ => self.line_spans.push(LineSpan {
                    block: b,
                    line,
                    start: base + begins,
                    end: base + out,
                }),
            }
        }
        if base + out != end {
            self.line_spans.truncate(kept);
        }
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
    /// holding `end - 1`, each by where in it the text is, line by line, when its lines are known
    /// (a paragraph or code block over more lines), else whole. Footnotes come last in the text but
    /// early in the source, so the end blocks alone don't bound the lines. `(0, 0)` if the note has
    /// no text.
    pub fn lines_of(&self, start: usize, end: usize) -> (u32, u32) {
        if self.blocks.is_empty() {
            return (0, 0);
        }
        let last_byte = end.saturating_sub(1).max(start);
        let (first, last) = (self.block_at(start), self.block_at(last_byte));
        (first..=last).fold((u32::MAX, 0), |(low, high), b| {
            let from = (b == first).then_some(start);
            let to = (b == last).then_some(last_byte);
            let (l, h) = self.lines_in(b, from, to);
            (low.min(l), high.max(h))
        })
    }

    /// The source lines of block `b`'s text from byte `from` of the text (its start when none) to
    /// byte `to` (its end when none): the line each is on, or, between two lines, the nearest with
    /// text within, when its lines are known; else the block's lines.
    fn lines_in(&self, b: usize, from: Option<usize>, to: Option<usize>) -> (u32, u32) {
        let block = &self.blocks[b];
        let spans = self.spans_of(b);
        let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
            return (block.start_line, block.end_line);
        };
        let low = from.map_or(first.line, |at| {
            spans
                .get(spans.partition_point(|s| s.end <= at))
                .unwrap_or(last)
                .line
        });
        let high = to.map_or(last.line, |at| {
            spans[spans.partition_point(|s| s.start <= at).saturating_sub(1)].line
        });
        (low, high.max(low))
    }

    /// Where each line's words are in block `b`, in order; none for a block on one line, a
    /// heading, a page's block, or one whose lines aren't known.
    fn spans_of(&self, b: usize) -> &[LineSpan] {
        let from = self.line_spans.partition_point(|s| s.block < b);
        let to = self.line_spans.partition_point(|s| s.block <= b);
        &self.line_spans[from..to]
    }

    /// Where the text of source line `line` is. A line of a paragraph or a code block that runs
    /// over more lines is its own text alone, Markdown marks aside ([`LineText::Own`]). A block on
    /// that line alone, a heading, an HTML page's block, or a block whose lines can't be told
    /// apart (a code span runs over a line break, or its words aren't on the lines counted for
    /// them) is the block, whole ([`LineText::Block`]). A line no block holds, or one with none of
    /// its block's text (a code block's fence, a line of an image alone), has none
    /// ([`LineText::Empty`]).
    pub fn line_text(&self, line: u32) -> LineText {
        let Some(b) = self
            .blocks
            .iter()
            .position(|b| (b.start_line..=b.end_line).contains(&line))
        else {
            return LineText::Empty;
        };
        let spans = self.spans_of(b);
        if spans.is_empty() {
            return LineText::Block(self.blocks[b].start, self.blocks[b].end);
        }
        spans
            .iter()
            .find(|s| s.line == line)
            .map_or(LineText::Empty, |s| LineText::Own(s.start, s.end))
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
/// Given `lines`, where each of its source lines starts in it, and where each inline's run of
/// text begins and ends, are noted too.
fn inline_text<'a>(node: &'a AstNode<'a>, out: &mut String, mut lines: Option<&mut LineStarts>) {
    let mut image_depth = 0usize;
    for edge in node.traverse() {
        match edge {
            NodeEdge::Start(n) => {
                let data = n.data();
                if let Some(lines) = lines.as_deref_mut() {
                    lines.began(&data.value, data.sourcepos, out.len());
                }
                match data.value {
                    NodeValue::Image(_) => image_depth += 1,
                    _ if image_depth > 0 => {}
                    NodeValue::Text(ref t) => out.push_str(t),
                    NodeValue::Code(ref code) => out.push_str(&code.literal),
                    NodeValue::SoftBreak | NodeValue::LineBreak | NodeValue::TableCell => {
                        out.push(' ');
                    }
                    _ => {}
                }
            }
            NodeEdge::End(n) => {
                let data = n.data();
                if matches!(data.value, NodeValue::Image(_)) {
                    image_depth -= 1;
                }
                if let Some(lines) = lines.as_deref_mut() {
                    lines.ended(&data.value, out.len());
                }
            }
        }
    }
}

fn line_number(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

/// The first and last source lines of a block at `pos`: an end at column 0 is the end of the line
/// before.
fn block_lines(pos: Sourcepos) -> (u32, u32) {
    let start = line_number(pos.start.line);
    let end = match pos.end.column {
        0 => line_number(pos.end.line.saturating_sub(1)),
        _ => line_number(pos.end.line),
    };
    (start, end.max(start))
}

/// Where a block's source lines are in its raw text, as worked out from its structure: what
/// [`TextMap::push_line_spans`] checks against the source and, when it holds, notes the lines by.
struct LineMap {
    /// Where each source line starts, in order of line, the first at 0.
    starts: Vec<(u32, usize)>,
    /// Where the runs of text the block's inlines gave begin and end, in order, for a paragraph;
    /// none for a code block or a text file's paragraph, whose text is one run. Each run is in
    /// its source as written, markup aside; a word across a cut (`` `Vec`s ``, `**bold**text`) is
    /// as many runs.
    cuts: Vec<usize>,
    /// The runs that are in the source exactly as written, in order: a paragraph's code spans;
    /// all of a code block or a text file's paragraph. Any other run shows the source's backslash
    /// escapes and entities as the characters they stand for.
    verbatim: Vec<(usize, usize)>,
}

/// Where each source line of a paragraph starts in its text as [`inline_text`] collects it, from
/// `start` to `last`, by the line breaks its inlines hold: each line break, and those in raw HTML
/// and in a link's or an image's title, start the next line. (comrak's own positions of inlines
/// are off after such a title.) Not known when a line's words can't be told apart, text or a code
/// span running over a line break, or when the line breaks counted don't come to the paragraph's
/// lines. What is counted can still be off (a reference link's title has its breaks in its
/// definition; a comment over a break is dropped before this), so the map is checked against the
/// source before it is trusted ([`LineCheck`]).
struct LineStarts {
    line: u32,
    last: u32,
    map: LineMap,
    known: bool,
}

impl LineStarts {
    fn new(start: u32, last: u32) -> Self {
        Self {
            line: start,
            last,
            map: LineMap {
                starts: vec![(start, 0)],
                cuts: Vec::new(),
                verbatim: Vec::new(),
            },
            known: true,
        }
    }

    /// Notes the inline `value` begun, at `pos` in the source, with the text `at` bytes long.
    fn began(&mut self, value: &NodeValue, pos: Sourcepos, at: usize) {
        self.cut(at);
        match value {
            NodeValue::SoftBreak | NodeValue::LineBreak => self.breaks(1, at),
            NodeValue::HtmlInline(html) => self.breaks(newlines(html), at),
            NodeValue::Code(_) => {
                self.map.verbatim.push((at, at));
                // A code span's line breaks are spaces in its code, and where they fell is lost.
                if pos.start.line != pos.end.line {
                    self.known = false;
                }
            }
            NodeValue::Text(text) if text.contains('\n') => self.known = false,
            _ => {}
        }
    }

    /// Notes the inline `value` ended, with the text `at` bytes long: a title follows a link's or
    /// an image's text, and a code span's text is as written up to here.
    fn ended(&mut self, value: &NodeValue, at: usize) {
        self.cut(at);
        match value {
            NodeValue::Link(link) | NodeValue::Image(link) => {
                self.breaks(newlines(&link.title), at);
            }
            NodeValue::Code(_) => {
                if let Some(code) = self.map.verbatim.last_mut() {
                    code.1 = at;
                }
            }
            _ => {}
        }
    }

    /// A cut between two inlines' runs of text, at byte `at`.
    fn cut(&mut self, at: usize) {
        if self.map.cuts.last() != Some(&at) {
            self.map.cuts.push(at);
        }
    }

    /// `count` line breaks, after which the text from byte `at` on is.
    fn breaks(&mut self, count: usize, at: usize) {
        for _ in 0..count {
            self.line += 1;
            self.map.starts.push((self.line, at));
        }
    }

    /// The map, when it is known: the line breaks counted came to the last line.
    fn known(self) -> Option<LineMap> {
        (self.known && self.line == self.last).then_some(self.map)
    }
}

/// Where each line of a code block's code starts in it: a fenced block's first is the line after
/// its fence, on line `start`, an indented block's is `start`; none should they run past `last`.
fn code_line_map(code: &NodeCodeBlock, start: u32, last: u32) -> Option<LineMap> {
    let first = if code.fenced { start + 1 } else { start };
    let mut at = 0;
    let starts: Vec<(u32, usize)> = (first..)
        .zip(code.literal.split_inclusive('\n'))
        .map(|(line, text)| {
            let begins = (line, at);
            at += text.len();
            begins
        })
        .collect();
    starts
        .last()
        .is_none_or(|&(line, _)| line <= last)
        .then_some(LineMap {
            starts,
            cuts: Vec::new(),
            verbatim: vec![(0, code.literal.len())],
        })
}

/// The source by its lines, for checking a block's line map against ([`LineCheck`]).
struct SourceByLine<'s> {
    source: &'s str,
    /// Where each line starts, in bytes: line `n` at index `n - 1`. Lines end at `\n`; a lone
    /// `\r` doesn't end one here, so a file with those (an old Mac's) has its blocks taken whole.
    starts: Vec<usize>,
}

impl<'s> SourceByLine<'s> {
    fn new(source: &'s str) -> Self {
        let starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { source, starts }
    }

    /// Line `line`, 1-based, without its line ending; empty past the end.
    fn line(&self, line: u32) -> &'s str {
        let n = usize::try_from(line).unwrap_or(usize::MAX);
        let Some(&start) = n.checked_sub(1).and_then(|i| self.starts.get(i)) else {
            return "";
        };
        let end = self.starts.get(n).copied().unwrap_or(self.source.len());
        self.source[start..end].trim_end_matches(['\n', '\r'])
    }
}

/// Checks a block's words against the source lines its map puts them on, in order: each run of a
/// word between two cuts must be in that line's source after the run before it. A verbatim run
/// (a code span's, a code block's, a text file's) is looked for in the line as written; any other
/// in the line as it shows, its backslash escapes and entities read ([`Decoded`]), as the run
/// shows them. One search per run, each from where the one before ended: linear in the line. A
/// word that isn't there is one the map has on the wrong line, however its count came out.
struct LineCheck<'a> {
    source: &'a SourceByLine<'a>,
    cuts: &'a [usize],
    verbatim: &'a [(usize, usize)],
    /// The first cut, and the first verbatim run, not yet passed.
    cut: usize,
    code: usize,
    /// The line in hand (none is 0), its source, the source as it shows when that differs, and
    /// how far along the source the runs so far reached.
    line: u32,
    text: &'a str,
    decoded: Option<Decoded>,
    from: usize,
}

impl<'a> LineCheck<'a> {
    fn new(map: &'a LineMap, source: &'a SourceByLine<'a>) -> Self {
        Self {
            source,
            cuts: &map.cuts,
            verbatim: &map.verbatim,
            cut: 0,
            code: 0,
            line: 0,
            text: "",
            decoded: None,
            from: 0,
        }
    }

    /// Whether the word `word`, at byte `at` of the block's raw text, is on source line `line`,
    /// after the words checked before it there.
    fn holds(&mut self, raw: &str, at: usize, word: &str, line: u32) -> bool {
        if line != self.line {
            self.line = line;
            self.text = self.source.line(line);
            self.decoded = self
                .text
                .contains(['\\', '&'])
                .then(|| Decoded::new(self.text));
            self.from = 0;
        }
        while self.cuts.get(self.cut).is_some_and(|&cut| cut <= at) {
            self.cut += 1;
        }
        let end = at + word.len();
        let mut start = at;
        loop {
            let stop = match self.cuts.get(self.cut) {
                Some(&cut) if cut < end => {
                    self.cut += 1;
                    cut
                }
                _ => end,
            };
            let run = &raw[start..stop];
            // A run is cut at a code span's ends, so it is in one or out of it, whole.
            while self
                .verbatim
                .get(self.code)
                .is_some_and(|&(_, code_end)| code_end <= start)
            {
                self.code += 1;
            }
            let as_written = self
                .verbatim
                .get(self.code)
                .is_some_and(|&(code_start, _)| code_start <= start);
            let found = match &self.decoded {
                Some(decoded) if !as_written => decoded.find(self.from, run),
                _ => self.text[self.from..]
                    .find(run)
                    .map(|found| self.from + found + run.len()),
            };
            match found {
                Some(found) => self.from = found,
                None => return false,
            }
            if stop == end {
                return true;
            }
            start = stop;
        }
    }
}

/// A source line as it shows: its backslash escapes (a backslash before ASCII punctuation) and
/// the entities of [`ENTITIES`] read as the characters they stand for, with where each reading
/// was, so a place in the text is a place in the line. Any other entity (`&copy;`, a numeric one)
/// shows as something else and stays as written here, so a run holding one goes unfound.
struct Decoded {
    text: String,
    readings: Vec<Reading>,
}

/// One escape or entity read: its bytes in the line, and where its character ends in the text.
#[derive(Clone, Copy)]
struct Reading {
    raw_start: usize,
    raw_end: usize,
    dec_end: usize,
}

impl Decoded {
    fn new(line: &str) -> Self {
        let mut text = String::with_capacity(line.len());
        let mut readings = Vec::new();
        let mut from = 0;
        while let Some(found) = line[from..].find(['\\', '&']) {
            let at = from + found;
            text.push_str(&line[from..at]);
            let tail = &line[at..];
            let read = match tail.strip_prefix('\\') {
                Some(escaped) => escaped
                    .chars()
                    .next()
                    .filter(|p| p.is_ascii_punctuation())
                    .map(|p| (2, p)),
                None => ENTITIES
                    .iter()
                    .find(|(name, _)| tail.starts_with(name))
                    .map(|&(name, c)| (name.len(), c)),
            };
            match read {
                Some((len, c)) => {
                    text.push(c);
                    readings.push(Reading {
                        raw_start: at,
                        raw_end: at + len,
                        dec_end: text.len(),
                    });
                    from = at + len;
                }
                None => {
                    // The `\` or `&` is as written.
                    text.push_str(&tail[..1]);
                    from = at + 1;
                }
            }
        }
        text.push_str(&line[from..]);
        Self { text, readings }
    }

    /// Where `run` ends in the line, looked for in the text from the line's byte `from` on.
    fn find(&self, from: usize, run: &str) -> Option<usize> {
        let from = self.to_text(from);
        self.text[from..]
            .find(run)
            .map(|found| self.to_line(from + found + run.len()))
    }

    /// The text's byte for the line's byte `at`; inside a reading, its character's end.
    fn to_text(&self, at: usize) -> usize {
        let i = self.readings.partition_point(|r| r.raw_end <= at);
        let (raw_end, dec_end) = self.ends_before(i);
        match self.readings.get(i) {
            Some(r) if r.raw_start < at => r.dec_end,
            _ => dec_end + (at - raw_end),
        }
    }

    /// The line's byte for the text's byte `at`, which is never inside a reading's character.
    fn to_line(&self, at: usize) -> usize {
        let i = self.readings.partition_point(|r| r.dec_end <= at);
        let (raw_end, dec_end) = self.ends_before(i);
        raw_end + (at - dec_end)
    }

    /// Where reading `i - 1` ends in the line and in the text: (0, 0) before the first. Between
    /// two readings the line and the text run alike.
    fn ends_before(&self, i: usize) -> (usize, usize) {
        i.checked_sub(1).map_or((0, 0), |i| {
            (self.readings[i].raw_end, self.readings[i].dec_end)
        })
    }
}

/// The words of `s`, as `split_whitespace` finds them, each with the byte it starts at.
fn words(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut chars = s.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, _) = chars.find(|(_, c)| !c.is_whitespace())?;
        let mut end = s.len();
        while let Some(&(i, c)) = chars.peek() {
            if c.is_whitespace() {
                end = i;
                break;
            }
            chars.next();
        }
        Some((start, &s[start..end]))
    })
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

    /// The line where the block with the (normalised) text `text`, a heading or not, is, after the
    /// last block found: where the longest of its leading texts (see [`leading_cuts`]) first is,
    /// so a shorter one found earlier, where the rest of the block doesn't follow, never wins.
    /// `None` when none is found within what's left of [`SEARCH_BUDGET`], or the text is too short
    /// to look for.
    ///
    /// One pass finds them all: each place the shortest leading text is, in order, holds the
    /// longest that matches there. The pass stops at the first place the longest of all is, else
    /// [`LOOK_AHEAD`] past the first place the shortest is.
    fn find(&mut self, text: &str, heading: bool) -> Option<u32> {
        let cuts = leading_cuts(text, heading);
        let (&shortest, &longest) = (cuts.first()?, cuts.last()?);
        let left = SEARCH_BUDGET.saturating_sub(self.scanned);
        let end = self
            .html
            .floor_char_boundary(self.from.saturating_add(left).min(self.html.len()));
        let rest = self.html.get(self.from..end)?;
        let needle = &text[..shortest];
        // The best place so far, and the leading text there.
        let mut best: Option<(usize, usize)> = None;
        let mut searched = rest.len();
        let mut next = 0;
        let mut window = rest;
        while let Some(found) = window[next..].find(needle) {
            let at = next + found;
            if best.is_none() {
                // Past the first place, a longer one is looked for only so far.
                let end = rest.floor_char_boundary(at.saturating_add(LOOK_AHEAD).min(rest.len()));
                window = &rest[..end];
                searched = end;
            }
            let same = common_prefix_bytes(&rest[at..], &text[..longest]);
            let cut = cuts
                .iter()
                .rev()
                .copied()
                .find(|&cut| cut <= same)
                .unwrap_or(shortest);
            if best.is_none_or(|(_, best)| cut > best) {
                best = Some((at, cut));
            }
            if cut == longest {
                searched = at + longest;
                break;
            }
            next = at + rest[at..].chars().next().map_or(1, char::len_utf8);
        }
        self.scanned += searched;
        let (found, cut) = best?;
        let at = self.from + found;
        self.from = at + cut;
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

/// `html` with `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;` ([`ENTITIES`]) read as the
/// characters they stand for, once: `&amp;lt;` is `&lt;`.
fn decode_entities(html: &str) -> String {
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
/// as byte lengths, shortest first: those of 8 to 40 characters ending at a word boundary (before a
/// space, or at the end of the text). Text with no word boundary from its 8th to its 40th
/// character (one long word, or a language written without spaces) is looked for by its first 40
/// characters. Text under 8 characters has none, as it says too little to be found by, unless it
/// is a `heading`, which is looked for by all of it.
fn leading_cuts(text: &str, heading: bool) -> Vec<usize> {
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
    if heading || text.chars().count() >= LEAD_MIN_CHARS {
        cuts.push(text.len());
    }
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
    use super::{SourceLines, TextMap, SEARCH_BUDGET};
    use crate::agent::request::Target;
    use crate::agent::target::resolve_target;
    use crate::render::TEXT_PARSES;

    /// Line targets in a large note's long paragraphs and code blocks cost no parse of their own:
    /// the note is parsed once, for its text, whose lines are known from then on, for every
    /// target and comment.
    #[test]
    fn line_targets_share_the_one_parse_of_the_note() {
        let mut note = String::from("# Survey\n\n");
        for section in 0..200 {
            for n in 0..40 {
                note.push_str(&format!("Entry {section}.{n}: the lanternfish count.\n"));
            }
            note.push_str("\n```text\n");
            for n in 0..40 {
                note.push_str(&format!("count_{section}_{n} = {n}\n"));
            }
            note.push_str("```\n\n");
        }
        let before = TEXT_PARSES.with(std::cell::Cell::get);
        let text = TextMap::build(&note);
        let lines: Vec<u32> = (0..20).map(|i| 4 + i * 413).collect();
        for &line in &lines {
            let found = resolve_target(&text, &note, "survey.md", &Target::Line { line });
            let place = found.unwrap_or_else(|e| panic!("line {line}: {e}"));
            assert_eq!((place.start_line, place.end_line), (line, line));
            assert!(place.text_start.is_some(), "line {line}");
        }
        assert_eq!(TEXT_PARSES.with(std::cell::Cell::get) - before, 1);
    }

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
        assert_eq!(lines.find("Intro paragraph.", false), Some(2));
        for i in 0..5000 {
            assert_eq!(
                lines.find(&format!("Row {i} made by the script"), false),
                None
            );
        }
        assert!(lines.scanned <= SEARCH_BUDGET, "{} bytes", lines.scanned);
        // Spent: nothing more is searched, even what is there.
        assert_eq!(lines.find("Closing paragraph.", false), None);
        assert!(lines.scanned <= SEARCH_BUDGET, "{} bytes", lines.scanned);
    }

    /// A block is placed where its longest leading text is: a shorter one found earlier, where the
    /// rest of the block doesn't follow, isn't where the block is.
    #[test]
    fn a_blocks_longest_leading_text_is_looked_for_first() {
        let source = "<body>\n<nav>Conversion rates</nav>\n<p>Conversion dipped in week 3.</p>\n\
                      <p>Conversion dipped in <em>week 4</em>.</p>\n";
        let mut lines = SourceLines::new(source);
        assert_eq!(lines.find("Conversion dipped in week 3.", false), Some(3));
        // Markup splits this one: its longest leading text found whole is at a word boundary.
        assert_eq!(lines.find("Conversion dipped in week 4.", false), Some(4));
    }

    /// A block shorter than 8 characters says too little to be looked for: it isn't, and nothing
    /// is searched for it, unless it is a heading, which is looked for by all of its text.
    #[test]
    fn a_block_under_8_characters_is_not_looked_for_unless_a_heading() {
        let source = "<body>\n<p>Intro paragraph.</p>\n<td>Results</td>\n<h2>Results</h2>\n";
        let mut lines = SourceLines::new(source);
        assert_eq!(lines.find("Intro paragraph.", false), Some(2));
        let scanned = lines.scanned;
        assert_eq!(lines.find("Results", false), None);
        assert_eq!(lines.scanned, scanned);
        let block = |text: &str, heading_level: Option<u8>| super::PageBlock {
            text: text.to_owned(),
            heading_level,
        };
        let map = super::TextMap::from_blocks(
            &[
                block("Intro paragraph.", None),
                block("3", None),
                block("Results", Some(2)),
            ],
            "<body>\n<p>Intro paragraph.</p>\n<td>3</td>\n<h2>Results</h2>\n",
        );
        // The cell takes the line of the block before it; the heading finds its own.
        let lines: Vec<u32> = map.blocks.iter().map(|b| b.start_line).collect();
        assert_eq!(lines, [2, 2, 4]);
    }

    /// Past the first place a block's shortest leading text is, a longer one is looked for no
    /// further than 64 KiB: a block whose longer text is only far beyond goes where the shorter
    /// was.
    #[test]
    fn a_longer_leading_text_is_looked_for_64_kib_past_the_shortest() {
        let near = format!(
            "<body>\n<p>Conversion rates</p>\n{}<p>Conversion dipped in week 3.</p>\n",
            "<p>Filler.</p>\n".repeat(1000)
        );
        let mut lines = SourceLines::new(&near);
        assert_eq!(
            lines.find("Conversion dipped in week 3.", false),
            Some(1003)
        );
        let far = format!(
            "<body>\n<p>Conversion rates</p>\n{}<p>Conversion dipped in week 3.</p>\n",
            "<p>Filler.</p>\n".repeat(10_000)
        );
        let mut lines = SourceLines::new(&far);
        assert_eq!(lines.find("Conversion dipped in week 3.", false), Some(2));
        assert!(lines.scanned <= 80 * 1024, "{} bytes", lines.scanned);
    }

    /// Within the budget, a block is found wherever it is.
    #[test]
    fn a_page_within_the_budget_is_searched_to_its_end() {
        let mut source = String::from("<body>\n<p>Intro paragraph.</p>\n");
        source.push_str(&"<p>Filler.</p>\n".repeat(10_000));
        source.push_str("<p>Closing paragraph.</p>\n");
        let mut lines = SourceLines::new(&source);
        assert_eq!(lines.find("Missing paragraph.", false), None);
        assert_eq!(lines.find("Closing paragraph.", false), Some(10_003));
    }
}
