//! The raw-HTML policy: a short allowlist of tags passes through, anything else in angle brackets
//! is shown as the literal text it is (`<slug>`, `<context>`, `<Loading />`).

use std::borrow::Cow;

use comrak::nodes::{AstNode, NodeValue};
use comrak::Arena;

/// The raw tags a note may use. `script`, `style`, `iframe`, `object` and `embed` are never on it.
const ALLOW: &[&str] = &[
    "br", "kbd", "sub", "sup", "details", "summary", "img", "a", "b", "i", "em", "strong", "mark",
    "del", "ins", "s", "u", "p", "div", "span", "picture", "source", "small", "abbr", "hr",
];

/// Applies the policy to every raw HTML node. Comments are dropped, allowlisted HTML passes
/// through to the sanitiser, and everything else becomes text.
pub(crate) fn apply<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) {
    let raw: Vec<&AstNode> = root
        .descendants()
        .filter(|node| {
            matches!(
                node.data().value,
                NodeValue::HtmlInline(_) | NodeValue::HtmlBlock(_)
            )
        })
        .collect();
    for node in raw {
        let mut data = node.data_mut();
        match data.value {
            NodeValue::HtmlInline(ref mut html) => {
                if html.starts_with("<!--") {
                    drop(data);
                    node.detach();
                } else if !is_allowed(html) {
                    data.value = NodeValue::Text(std::mem::take(html).into());
                }
            }
            // comrak ends a comment block at the line holding `-->`, so whatever follows the
            // comment on that line is part of the block too.
            NodeValue::HtmlBlock(ref mut block) => {
                let literal = strip_comments(&block.literal);
                let html = literal.trim_start();
                if html.is_empty() {
                    drop(data);
                    node.detach();
                } else if is_allowed(html) {
                    if let Cow::Owned(stripped) = literal {
                        block.literal = stripped;
                    }
                } else {
                    let literal = literal.into_owned();
                    data.value = NodeValue::Paragraph;
                    drop(data);
                    append_lines(arena, node, &literal);
                }
            }
            _ => {}
        }
    }
}

/// Whether the fragment starts with an allowlisted tag, opening or closing, in any letter case.
fn is_allowed(html: &str) -> bool {
    tag_name(html).is_some_and(|name| ALLOW.iter().any(|tag| tag.eq_ignore_ascii_case(name)))
}

/// The name in a leading `<name` or `</name`: a letter, then letters, digits and hyphens.
fn tag_name(html: &str) -> Option<&str> {
    let rest = html.strip_prefix('<')?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(rest.len());
    let name = &rest[..end];
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        .then_some(name)
}

/// Fills a paragraph with the lines of `literal` as text, a hard break between each.
fn append_lines<'a>(arena: &'a Arena<'a>, paragraph: &'a AstNode<'a>, literal: &str) {
    let lines = literal.trim_end_matches(['\n', '\r']).split('\n');
    for (i, line) in lines.enumerate() {
        if i > 0 {
            paragraph.append(arena.alloc(NodeValue::LineBreak.into()));
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !line.is_empty() {
            paragraph.append(arena.alloc(NodeValue::Text(line.to_owned().into()).into()));
        }
    }
}

/// `html` without its comments. An unclosed comment runs to the end, and a comment alone on its
/// lines takes those lines with it.
fn strip_comments(html: &str) -> Cow<'_, str> {
    if !html.contains("<!--") {
        return Cow::Borrowed(html);
    }
    let mut kept = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        // Searching from inside the opener also closes `<!-->` and `<!--->`, as HTML does.
        let end = rest[start + 2..]
            .find("-->")
            .map_or(rest.len(), |offset| start + 2 + offset + 3);
        rest = &rest[end..];
        let line_start = kept.rfind('\n').map_or(0, |i| i + 1);
        let line_end = rest.find('\n').map_or(rest.len(), |i| i + 1);
        if kept[line_start..].trim().is_empty() && rest[..line_end].trim().is_empty() {
            kept.truncate(line_start);
            rest = &rest[line_end..];
        }
    }
    kept.push_str(rest);
    Cow::Owned(kept)
}
