//! Code blocks: highlighted in parallel, each wrapped in a header with its language and a copy
//! button.

use std::collections::HashMap;

use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use rayon::prelude::*;
use syntect::parsing::SyntaxSet;

use super::escape_html;
use super::highlight::{highlight_with, on_pool, syntaxes, Rendering};

/// The finished markup for each code block, by the block's start position (line, column).
pub(crate) type CodeBlocks = HashMap<(usize, usize), String>;

const COPY_BUTTON: &str =
    r#"<button type="button" class="code-copy" aria-label="Copy code">Copy</button>"#;

struct Block {
    sourcepos: Sourcepos,
    lang: String,
    code: String,
}

/// Highlights every code block in the document on the highlighting threads and builds its markup.
/// A document without code never touches the highlighter, and one whose blocks have no language
/// never waits for its threads.
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
    let key = |block: &Block| (block.sourcepos.start.line, block.sourcepos.start.column);
    if !blocks.iter().any(|block| !block.lang.is_empty()) {
        return blocks
            .iter()
            .map(|block| (key(block), markup(None, block)))
            .collect();
    }
    // Counted before the grammars are fetched, so the warm-up steps aside from here on.
    let _rendering = Rendering::begin();
    let set = syntaxes();
    on_pool(|| {
        blocks
            .par_iter()
            .map(|block| (key(block), markup(Some(&set), block)))
            .collect()
    })
}

/// The languages of the code blocks in rendered `html`, in order of first use, each once: what to
/// warm first when the document is on screen. Tags that needed escaping are left out. Raw HTML can
/// add a tag of its own, which costs nothing: the warm-up skips a tag without a grammar.
pub fn code_languages(html: &str) -> Vec<String> {
    const MARK: &str = r#"<div class="code-block" data-lang=""#;
    let mut langs: Vec<String> = Vec::new();
    for (at, _) in html.match_indices(MARK) {
        let rest = &html[at + MARK.len()..];
        let Some(lang) = rest.split('"').next().filter(|lang| !lang.contains('&')) else {
            continue;
        };
        if !langs.iter().any(|known| known.eq_ignore_ascii_case(lang)) {
            langs.push(lang.to_owned());
        }
    }
    langs
}

/// The language is the info string's first word, as comrak reads it for `language-…`.
fn first_word(info: &str) -> &str {
    info.split(|c: char| c.is_ascii_whitespace())
        .next()
        .unwrap_or_default()
}

/// `<div class="code-block">` holding the header, an optional note and the highlighted `<pre>`.
/// A block without a language gets no `data-lang`, an empty label and a bare `<code>`. `set` is
/// the syntax set, there whenever the block has a language.
fn markup(set: Option<&SyntaxSet>, block: &Block) -> String {
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
    let code = match set.filter(|_| !block.lang.is_empty()) {
        Some(set) => highlight_with(set, &block.lang, &block.code),
        None => escape_html(&block.code),
    };
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::Duration;

    use crate::library::pathmap::PathMapper;
    use crate::render::highlight::{self, release_grammars};
    use crate::render::{render, RenderContext};

    const RUBY: &str =
        "# Plan\n\n```ruby\ndef step(record)\n  record.save! if record.valid?\nend\n```\n";
    const NO_CODE: &str = "# Notes\n\nPlain text with `inline code` and a [link](x.md).\n";

    fn html(src: &str) -> String {
        let mapper = PathMapper::default();
        let ctx = RenderContext {
            doc_path: Path::new("/vault/notes/a.md"),
            index: None,
            mapper: &mapper,
            asset_base: "http://lxasset.localhost/",
            trusted_unc_hosts: &[],
        };
        render(src, &ctx).html
    }

    /// Renders `src` on a thread of its own; the result arrives on the receiver.
    fn render_async(src: &'static str) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(html(src));
        });
        rx
    }

    /// One test at a time holds the threads or releases the grammars.
    fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps every highlighting thread busy until dropped.
    struct HeldPool {
        open: Arc<(Mutex<bool>, Condvar)>,
        holder: Option<thread::JoinHandle<()>>,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    fn hold_pool() -> HeldPool {
        let turn = turn();
        let pool = highlight::pool().expect("the highlighting threads started");
        let open = Arc::new((Mutex::new(false), Condvar::new()));
        let busy = Arc::new(std::sync::Barrier::new(pool.current_num_threads() + 1));
        let (gate, ready) = (Arc::clone(&open), Arc::clone(&busy));
        let holder = thread::spawn(move || {
            pool.broadcast(|_| {
                ready.wait();
                let (open, cv) = &*gate;
                drop(cv.wait_while(open.lock().unwrap(), |open| !*open).unwrap());
            });
        });
        busy.wait();
        HeldPool {
            open,
            holder: Some(holder),
            _turn: turn,
        }
    }

    impl Drop for HeldPool {
        fn drop(&mut self) {
            let (open, cv) = &*self.open;
            *open.lock().unwrap() = true;
            cv.notify_all();
            if let Some(holder) = self.holder.take() {
                holder.join().unwrap();
            }
        }
    }

    #[test]
    fn code_languages_lists_each_blocks_language_once_in_order() {
        let src = concat!(
            "```ruby\na\n```\n\n```jsx\n<b />\n```\n\n```\nplain\n```\n\n",
            "```Ruby\nb\n```\n\n```a\"b\nc\n```\n",
        );
        assert_eq!(super::code_languages(&html(src)), ["ruby", "jsx"]);
        assert!(super::code_languages(&html(NO_CODE)).is_empty());
    }

    #[test]
    fn a_document_without_code_never_waits_for_the_highlighting_threads() {
        let held = hold_pool();
        let plain = render_async(NO_CODE);
        assert!(plain
            .recv_timeout(Duration::from_secs(5))
            .expect("a document without code rendered while the threads were busy")
            .contains("inline code"));
        // The hold is real: a document with code waits for it.
        let code = render_async(RUBY);
        assert_eq!(
            code.recv_timeout(Duration::from_millis(300)).err(),
            Some(RecvTimeoutError::Timeout)
        );
        drop(held);
        assert!(code
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .contains("hl-"));
    }

    #[test]
    fn a_render_under_way_survives_a_release() {
        let held = hold_pool();
        // The render takes the syntax set, then waits for the threads.
        let code = render_async(RUBY);
        thread::sleep(Duration::from_millis(200));
        assert!(release_grammars(), "the render loaded the grammars");
        drop(held);
        let out = code.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(out.contains("hl-"), "{out}");
    }

    #[test]
    fn a_render_after_a_release_compiles_the_grammars_again() {
        let _turn = turn();
        let before = html(RUBY);
        assert!(before.contains("hl-"));
        assert!(release_grammars());
        let after = html(RUBY);
        assert_eq!(after, before);
        assert!(highlight::grammars_last_used().is_some());
    }
}
