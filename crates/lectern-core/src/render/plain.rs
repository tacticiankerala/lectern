//! Text files: each paragraph, a run of lines that aren't blank, shows as one block of its lines,
//! as written. Nothing in the file is Markdown or HTML.

use std::fmt::Write as _;
use std::path::Path;

use comrak::html;

use super::{file_title, stats, RenderedDoc, TaskStats};

/// One paragraph of a text file.
pub(crate) struct Paragraph {
    /// The paragraph's source lines, 1-based.
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    /// The length in bytes of its last line: the column its last character ends at, as comrak's
    /// `data-sourcepos` counts it.
    pub(crate) end_column: usize,
    /// Its lines joined by `\n`, each NUL as U+FFFD: the page's HTML parser drops a NUL, and
    /// comrak replaces one the same way in a note.
    pub(crate) text: String,
}

/// The paragraphs of `source`, in order. Lines end at `\r\n`, `\n` or a lone `\r`, as in a note,
/// and a leading BOM isn't text. A line of whitespace only ends a paragraph as an empty one does.
pub(crate) fn paragraphs(source: &str) -> Vec<Paragraph> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut out = Vec::new();
    let mut open: Option<Paragraph> = None;
    for (n, line) in lines(source).enumerate() {
        let number = u32::try_from(n + 1).unwrap_or(u32::MAX);
        if line.trim().is_empty() {
            out.extend(open.take());
            continue;
        }
        let paragraph = open.get_or_insert_with(|| Paragraph {
            start_line: number,
            end_line: number,
            end_column: 0,
            text: String::new(),
        });
        if !paragraph.text.is_empty() {
            paragraph.text.push('\n');
        }
        paragraph.text.extend(line.chars().map(|c| match c {
            '\0' => '\u{fffd}',
            c => c,
        }));
        paragraph.end_line = number;
        paragraph.end_column = line.len();
    }
    out.extend(open);
    out
}

/// The lines of `source`, without their line endings.
fn lines(source: &str) -> impl Iterator<Item = &str> {
    let mut rest = source;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let Some(at) = rest.find(['\r', '\n']) else {
            return Some(std::mem::take(&mut rest));
        };
        let line = &rest[..at];
        let ending = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + ending..];
        Some(line)
    })
}

/// Renders a text file: each paragraph as `<p class="lx-plain" data-sourcepos="…">` holding its
/// lines, escaped and joined by `\n`, which the page shows with `white-space: pre-wrap`. Blank
/// lines aren't rendered. There's no outline, front matter or tasks, and the title is the file's
/// name.
pub fn render_plain(source: &str, doc_path: &Path) -> RenderedDoc {
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    for p in paragraphs(source) {
        write!(
            out,
            "<p class=\"lx-plain\" data-sourcepos=\"{}:1-{}:{}\">",
            p.start_line, p.end_line, p.end_column
        )
        .and_then(|()| html::escape(&mut out, &p.text))
        .expect("writing to a String cannot fail");
        out.push_str("</p>\n");
    }
    RenderedDoc {
        html: out,
        outline: Vec::new(),
        frontmatter: None,
        tasks: TaskStats::default(),
        title: file_title(doc_path),
        word_count: stats::word_count(source),
        has_unresolved_wikilinks: false,
    }
}
