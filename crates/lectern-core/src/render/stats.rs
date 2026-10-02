//! Task counts, word counts and the document title.

use std::path::Path;

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, NodeValue};
use unicode_segmentation::UnicodeSegmentation;

use super::{OutlineItem, TaskStats};
use crate::frontmatter::{Frontmatter, PropValue};

/// Counts task-list items, nested ones included.
pub(crate) fn count_tasks<'a>(root: &'a AstNode<'a>) -> TaskStats {
    let mut stats = TaskStats::default();
    for node in root.descendants() {
        if let NodeValue::TaskItem(ref task) = node.data().value {
            stats.total += 1;
            if task.symbol.is_some() {
                stats.done += 1;
            }
        }
    }
    stats
}

/// The text a reader sees under `node`: text and code spans, with a space for each line break and
/// before each block. Code blocks, raw HTML and image alt text are left out.
pub(crate) fn visible_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    let mut image_depth = 0usize;
    for edge in node.traverse() {
        match edge {
            NodeEdge::Start(n) => match n.data().value {
                NodeValue::Image(_) => image_depth += 1,
                _ if image_depth > 0 => {}
                NodeValue::Text(ref t) => text.push_str(t),
                NodeValue::Code(ref code) => text.push_str(&code.literal),
                NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
                ref value if value.block() => text.push(' '),
                _ => {}
            },
            NodeEdge::End(n) => {
                if matches!(n.data().value, NodeValue::Image(_)) {
                    image_depth -= 1;
                }
            }
        }
    }
    text
}

/// Words by Unicode word boundaries (UAX #29), ignoring runs of punctuation.
pub(crate) fn word_count(text: &str) -> u32 {
    u32::try_from(text.unicode_words().count()).unwrap_or(u32::MAX)
}

/// The frontmatter `title`, else the first h1, else the file stem.
pub(crate) fn title_of(
    frontmatter: Option<&Frontmatter>,
    outline: &[OutlineItem],
    doc_path: &Path,
) -> String {
    frontmatter_title(frontmatter)
        .or_else(|| {
            outline
                .iter()
                .find(|o| o.level == 1 && !o.text.is_empty())
                .map(|o| o.text.clone())
        })
        .unwrap_or_else(|| {
            doc_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

fn frontmatter_title(frontmatter: Option<&Frontmatter>) -> Option<String> {
    let Some(Frontmatter::Parsed { entries }) = frontmatter else {
        return None;
    };
    let property = entries.iter().find(|p| p.key == "title")?;
    let title = match &property.value {
        PropValue::Text(s) | PropValue::Date(s) | PropValue::DateTime(s) | PropValue::Number(s) => {
            s.trim()
        }
        PropValue::Bool(_) | PropValue::List(_) => return None,
    };
    (!title.is_empty()).then(|| title.to_owned())
}
