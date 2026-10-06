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

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::Arena;

use crate::render::parse_for_text;

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

    /// Appends a block with the text `raw`, normalised, unless that's empty.
    fn push_block(&mut self, raw: &str, pos: Sourcepos) {
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
        let start_line = line_number(pos.start.line);
        // An end at column 0 is the end of the line before.
        let end_line = match pos.end.column {
            0 => line_number(pos.end.line.saturating_sub(1)),
            _ => line_number(pos.end.line),
        };
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
