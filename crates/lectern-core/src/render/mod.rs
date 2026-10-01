//! Markdown to HTML for the reading view, plus the outline, frontmatter and stats around it.

mod code_blocks;
pub mod highlight;
mod html_policy;
mod links;
mod options;
mod sanitize;
pub mod slug;
mod stats;
mod tags;

use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::path::Path;

use comrak::html::{self, ChildRendering, Context};
use comrak::nodes::{AstNode, NodeValue};
use comrak::options::Plugins;
use comrak::{parse_document, Arena};
use serde::Serialize;

use crate::frontmatter::{parse_frontmatter, Frontmatter};
use crate::library::pathmap::PathMapper;
use crate::library::LibraryIndex;
use code_blocks::CodeBlocks;
use slug::Slugger;

/// Bumped whenever the rendered output changes, so cached renders are discarded.
pub const RENDER_VERSION: u32 = 3;

/// What a render needs besides the Markdown source.
#[derive(Debug, Clone, Copy)]
pub struct RenderContext<'a> {
    pub doc_path: &'a Path,
    pub index: Option<&'a LibraryIndex>,
    pub mapper: &'a PathMapper,
    /// The asset-protocol prefix for local images, `http://asset.localhost/` on Windows.
    pub asset_base: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub level: u8,
    pub text: String,
    /// The heading's `id`, empty when its text has nothing to slug (an emoji-only heading).
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskStats {
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedDoc {
    pub html: String,
    pub outline: Vec<OutlineItem>,
    pub frontmatter: Option<Frontmatter>,
    pub tasks: TaskStats,
    pub title: String,
    pub word_count: u32,
    pub has_unresolved_wikilinks: bool,
}

/// Heading ids by the heading's start position (line, column), for the formatter.
type HeadingIds = HashMap<(usize, usize), String>;

/// What the formatter writes in place of comrak's own markup: worked out before formatting, or
/// (links) resolved as the formatter reaches them.
struct Prepared<'c> {
    heading_ids: HeadingIds,
    code_blocks: CodeBlocks,
    links: links::Links<'c>,
}

/// Renders a note to sanitised HTML and collects its outline, frontmatter and stats.
pub fn render(source: &str, ctx: &RenderContext) -> RenderedDoc {
    let blanked = blank_empty_frontmatter(source);
    let source = blanked.as_deref().unwrap_or(source);

    let options = options::comrak_options();
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);

    let frontmatter = match blanked {
        Some(_) => Some(Frontmatter::Parsed { entries: vec![] }),
        None => take_frontmatter(root),
    };
    html_policy::apply(&arena, root);
    let (outline, heading_ids) = collect_headings(root);
    let tasks = stats::count_tasks(root);
    let word_count = stats::word_count(&stats::visible_text(root));
    tags::apply(&arena, root);
    let links = links::Links::new(ctx);
    let code_blocks = code_blocks::render_all(root);

    let mut html = String::with_capacity(source.len() * 2);
    let prepared = html::format_document_with_formatter(
        root,
        &options,
        &mut html,
        &Plugins::default(),
        format_node,
        Prepared {
            heading_ids,
            code_blocks,
            links,
        },
    )
    .expect("formatting into a String cannot fail");
    let html = sanitize::clean(&html, prepared.links.images);
    let title = stats::title_of(frontmatter.as_ref(), &outline, ctx.doc_path);

    RenderedDoc {
        html,
        outline,
        frontmatter,
        tasks,
        title,
        word_count,
        has_unresolved_wikilinks: prepared.links.unresolved_wikilinks,
    }
}

/// comrak finds no front matter when the two `---` lines are adjacent and would render them as two
/// rules. For that case this returns the source with both lines emptied, keeping their line breaks
/// so every source position below stays the same.
fn blank_empty_frontmatter(source: &str) -> Option<String> {
    let rest = source.strip_prefix('\u{feff}').unwrap_or(source);
    let (first_eol, rest) = delimiter_line(rest)?;
    if first_eol.is_empty() {
        return None;
    }
    let (second_eol, body) = delimiter_line(rest)?;
    Some(format!("{first_eol}{second_eol}{body}"))
}

/// A `---` line at the start of `s`, as comrak matches it: its line ending (empty at the end of
/// the input) and what follows.
fn delimiter_line(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("---")?;
    if let Some(after) = rest.strip_prefix("\r\n") {
        Some(("\r\n", after))
    } else if let Some(after) = rest.strip_prefix('\n') {
        Some(("\n", after))
    } else if rest.is_empty() {
        Some(("", rest))
    } else {
        None
    }
}

/// Detaches comrak's front-matter node, if any, and parses the YAML inside it.
fn take_frontmatter<'a>(root: &'a AstNode<'a>) -> Option<Frontmatter> {
    let node = root.first_child()?;
    let block = match node.data().value {
        NodeValue::FrontMatter(ref block) => frontmatter_body(block),
        _ => return None,
    };
    node.detach();
    Some(parse_frontmatter(&block))
}

/// The YAML inside a front-matter block: comrak keeps both `---` lines and any blank lines after.
fn frontmatter_body(block: &str) -> String {
    let mut lines: Vec<&str> = block.lines().skip(1).collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.pop();
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// Slugs every heading in document order and builds the outline.
fn collect_headings<'a>(root: &'a AstNode<'a>) -> (Vec<OutlineItem>, HeadingIds) {
    let mut slugger = Slugger::new();
    let mut outline = Vec::new();
    let mut ids = HeadingIds::new();
    for node in root.descendants() {
        let data = node.data();
        let NodeValue::Heading(heading) = data.value else {
            continue;
        };
        let text = stats::visible_text(node).trim().to_owned();
        let id = slugger.slug(&text);
        ids.insert(
            (data.sourcepos.start.line, data.sourcepos.start.column),
            id.clone(),
        );
        outline.push(OutlineItem {
            level: heading.level,
            text,
            id,
        });
    }
    (outline, ids)
}

/// comrak's HTML formatter, with heading ids, highlighted code blocks, a scroll wrapper around
/// tables, and resolved links, wikilinks, code paths and images.
fn format_node<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    let data = node.data();
    if entering && data.value.contains_inlines() {
        links::start_inlines(context);
    }
    match data.value {
        NodeValue::Heading(ref heading) => format_heading(context, node, entering, heading.level),
        NodeValue::CodeBlock(_) => format_code_block(context, node, entering),
        NodeValue::Table(_) => format_table(context, node, entering),
        NodeValue::Link(ref link) => links::format_link(context, node, entering, link),
        NodeValue::WikiLink(ref link) => links::format_wikilink(context, node, entering, &link.url),
        NodeValue::Code(ref code) => links::format_code(context, node, entering, &code.literal),
        NodeValue::Image(ref image) => links::format_image(context, node, entering, image),
        NodeValue::HtmlInline(ref raw) => links::format_raw_inline(context, node, entering, raw),
        _ => html::format_node_default(context, node, entering),
    }
}

/// `<hN id="…" data-sourcepos="…">`, the id omitted when the slug is empty.
fn format_heading<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    level: u8,
) -> Result<ChildRendering, fmt::Error> {
    if entering {
        context.cr()?;
        write!(context, "<h{level}")?;
        let start = node.data().sourcepos.start;
        if let Some(id) = context.user.heading_ids.remove(&(start.line, start.column)) {
            if !id.is_empty() {
                context.write_str(" id=\"")?;
                context.escape(&id)?;
                context.write_str("\"")?;
            }
        }
        html::render_sourcepos(context, node)?;
        context.write_str(">")?;
    } else {
        write!(context, "</h{level}>")?;
        context.lf()?;
    }
    Ok(ChildRendering::HTML)
}

/// The markup `code_blocks::render_all` built for this block.
fn format_code_block<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    if entering {
        let start = node.data().sourcepos.start;
        let Some(markup) = context.user.code_blocks.remove(&(start.line, start.column)) else {
            return html::format_node_default(context, node, entering);
        };
        context.cr()?;
        context.write_str(&markup)?;
        context.lf()?;
    }
    Ok(ChildRendering::HTML)
}

/// comrak's table markup inside `<div class="table-wrap">`, which scrolls wide tables sideways.
fn format_table<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    if entering {
        context.cr()?;
        context.write_str("<div class=\"table-wrap\"><table")?;
        html::render_sourcepos(context, node)?;
        context.write_str(">")?;
        context.lf()?;
    } else {
        // comrak opens <tbody> at the first body row, so close it when there is one.
        let has_body = match (node.first_child(), node.last_child()) {
            (Some(first), Some(last)) => !first.same_node(last),
            _ => false,
        };
        if has_body {
            context.cr()?;
            context.write_str("</tbody>")?;
            context.lf()?;
        }
        context.cr()?;
        context.write_str("</table></div>")?;
        context.lf()?;
    }
    Ok(ChildRendering::HTML)
}

/// `text` with `&`, `<`, `>` and `"` escaped, for HTML text and double-quoted attributes.
pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + text.len() / 8);
    html::escape(&mut escaped, text).expect("writing to a String cannot fail");
    escaped
}
