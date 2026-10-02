//! `#tag` chips in prose.

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, NodeValue};
use comrak::Arena;
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

use super::escape_html;

/// Turns each `#tag` in text into `<span class="tag" data-tag="…">#…</span>`. Text inside links,
/// images and wikilinks is left alone; code never holds text nodes.
pub(crate) fn apply<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) {
    let mut texts = Vec::new();
    let mut skip_depth = 0usize;
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(node) => match node.data().value {
                NodeValue::Link(_) | NodeValue::Image(_) | NodeValue::WikiLink(_) => {
                    skip_depth += 1;
                }
                NodeValue::Text(ref text) if skip_depth == 0 && text.contains('#') => {
                    texts.push(node);
                }
                _ => {}
            },
            NodeEdge::End(node) => {
                if matches!(
                    node.data().value,
                    NodeValue::Link(_) | NodeValue::Image(_) | NodeValue::WikiLink(_)
                ) {
                    skip_depth -= 1;
                }
            }
        }
    }
    for node in texts {
        split_tags(arena, node);
    }
}

/// Replaces the tags in one text node with chips, keeping the text around them.
fn split_tags<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>) {
    let text = match node.data().value {
        NodeValue::Text(ref text) => text.to_string(),
        _ => return,
    };
    let tags = find_tags(&text, starts_after_space(node));
    if tags.is_empty() {
        return;
    }
    let mut copied = 0;
    for (start, end) in tags {
        if start > copied {
            node.insert_before(text_node(arena, &text[copied..start]));
        }
        let name = escape_html(&text[start + 1..end]);
        let chip = format!(r#"<span class="tag" data-tag="{name}">#{name}</span>"#);
        node.insert_before(arena.alloc(NodeValue::Raw(chip).into()));
        copied = end;
    }
    if copied < text.len() {
        node.data_mut().value = NodeValue::Text(text[copied..].to_owned().into());
    } else {
        node.detach();
    }
}

fn text_node<'a>(arena: &'a Arena<'a>, text: &str) -> &'a AstNode<'a> {
    arena.alloc(NodeValue::Text(text.to_owned().into()).into())
}

/// The byte ranges of each `#tag` in `text`. A tag follows whitespace (or the start of the text
/// when `at_boundary`), starts with an ASCII letter, runs over word characters, `/` and `-`, and
/// is not a hex colour such as `#fff`.
fn find_tags(text: &str, at_boundary: bool) -> Vec<(usize, usize)> {
    let mut tags = Vec::new();
    let mut after_space = at_boundary;
    for (i, c) in text.char_indices() {
        if c == '#' && after_space {
            let rest = &text[i + 1..];
            if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                let len = rest.find(|c| !is_tag_char(c)).unwrap_or(rest.len());
                if !is_hex_colour(&rest[..len]) {
                    tags.push((i, i + 1 + len));
                }
            }
        }
        after_space = c.is_whitespace();
    }
    tags
}

/// `\w` (letters, marks, numbers, connector punctuation such as `_`) plus `/` and `-`.
fn is_tag_char(c: char) -> bool {
    matches!(c, '/' | '-')
        || matches!(
            c.general_category_group(),
            GeneralCategoryGroup::Letter
                | GeneralCategoryGroup::Mark
                | GeneralCategoryGroup::Number
        )
        || c.general_category() == GeneralCategory::ConnectorPunctuation
}

fn is_hex_colour(token: &str) -> bool {
    (3..=8).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether the text node starts a block or follows whitespace or a line break, looking out of
/// inline containers such as emphasis.
fn starts_after_space<'a>(node: &'a AstNode<'a>) -> bool {
    let mut current = node;
    loop {
        if let Some(previous) = current.previous_sibling() {
            return match previous.data().value {
                NodeValue::Text(ref text) => text.ends_with(char::is_whitespace),
                NodeValue::SoftBreak | NodeValue::LineBreak => true,
                _ => false,
            };
        }
        match current.parent() {
            Some(parent) if !parent.data().value.block() => current = parent,
            _ => return true,
        }
    }
}
