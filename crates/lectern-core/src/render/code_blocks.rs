//! Code blocks: highlighted in parallel, each wrapped in a header with its language and a copy
//! button.

use std::collections::HashMap;

use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use rayon::prelude::*;

use super::escape_html;
use super::highlight::highlight_to_html;

/// The finished markup for each code block, by the block's start position (line, column).
pub(crate) type CodeBlocks = HashMap<(usize, usize), String>;

const COPY_BUTTON: &str =
    r#"<button type="button" class="code-copy" aria-label="Copy code">Copy</button>"#;

struct Block {
    sourcepos: Sourcepos,
    lang: String,
    code: String,
}

/// Highlights every code block in the document across threads and builds its markup.
pub(crate) fn render_all<'a>(root: &'a AstNode<'a>) -> CodeBlocks {
    let blocks: Vec<Block> = root
        .descendants()
        .filter_map(|node| {
            let data = node.data();
            let NodeValue::CodeBlock(ref block) = data.value else {
                return None;
            };
            Some(Block {
                sourcepos: data.sourcepos,
                lang: first_word(&block.info).to_owned(),
                code: block.literal.clone(),
            })
        })
        .collect();
    blocks
        .par_iter()
        .map(|block| {
            let start = block.sourcepos.start;
            ((start.line, start.column), markup(block))
        })
        .collect()
}

/// The language is the info string's first word, as comrak reads it for `language-…`.
fn first_word(info: &str) -> &str {
    info.split(|c: char| c.is_ascii_whitespace())
        .next()
        .unwrap_or_default()
}

/// `<div class="code-block">` holding the header, an optional note and the highlighted `<pre>`.
/// A block without a language gets no `data-lang`, an empty label and a bare `<code>`.
fn markup(block: &Block) -> String {
    let lang = escape_html(&block.lang);
    let (data_lang, code_open) = if lang.is_empty() {
        (String::new(), "<code>".to_owned())
    } else {
        (
            format!(r#" data-lang="{lang}""#),
            format!(r#"<code class="language-{lang}">"#),
        )
    };
    let note = note_for(&block.lang)
        .map(|note| format!(r#"<div class="code-note">{note}</div>"#))
        .unwrap_or_default();
    let code = highlight_to_html(
        (!block.lang.is_empty()).then_some(block.lang.as_str()),
        &block.code,
    );
    format!(
        concat!(
            r#"<div class="code-block"{data_lang} data-sourcepos="{sourcepos}">"#,
            r#"<div class="code-head"><span class="code-lang">{lang}</span>{copy}</div>"#,
            "{note}<pre>{code_open}{code}</code></pre></div>",
        ),
        data_lang = data_lang,
        sourcepos = block.sourcepos,
        lang = lang,
        copy = COPY_BUTTON,
        note = note,
        code_open = code_open,
        code = code,
    )
}

/// Diagrams and maths are shown as source until Lectern can render them.
fn note_for(lang: &str) -> Option<&'static str> {
    if lang.eq_ignore_ascii_case("mermaid") {
        Some("Diagram rendering isn't supported yet")
    } else if lang.eq_ignore_ascii_case("math") {
        Some("Math rendering isn't supported yet")
    } else {
        None
    }
}
