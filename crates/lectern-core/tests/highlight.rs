use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use lectern_core::library::pathmap::PathMapper;
use lectern_core::render::highlight::{canonical_lang, highlight_to_html, warm_up};
use lectern_core::render::{render, RenderContext, RenderedDoc};

fn r(src: &str) -> RenderedDoc {
    let mapper = PathMapper::default();
    let ctx = RenderContext {
        doc_path: Path::new("/vault/notes/Untitled note.md"),
        index: None,
        mapper: &mapper,
        asset_base: "http://asset.localhost/",
    };
    render(src, &ctx)
}

#[test]
fn ruby_highlighted_with_classes() {
    let h = r("```ruby\ndef x; end\n```\n").html;
    assert!(h.contains("hl-") && h.contains(r#"data-lang="ruby""#));
}

#[test]
fn jsx_and_vim_supported() {
    for l in [
        "jsx", "vim", "bash", "ts", "scss", "yaml", "sql", "diff", "json", "jsonc",
    ] {
        assert!(
            lectern_core::render::highlight::canonical_lang(l).is_some(),
            "{l}"
        );
    }
}

#[test]
fn unknown_lang_escaped_plain() {
    let h = r("```weird\n<b>x</b>\n```\n").html;
    assert!(h.contains("&lt;b&gt;x&lt;/b&gt;") && !h.contains("hl-"));
}

#[test]
fn tabs_preserved() {
    assert!(r("```\n\tindented\n```\n").html.contains("\tindented"));
}

#[test]
fn mermaid_labelled() {
    assert!(r("```mermaid\ngraph TD\n```\n").html.contains("code-note"));
}

#[test]
fn copy_button_present() {
    assert!(r("```bash\nls\n```\n")
        .html
        .contains(r#"class="code-copy""#));
}

#[test]
fn code_block_markup() {
    let h = r("```ruby\nx = 1\n```\n").html;
    assert!(
        h.starts_with(concat!(
            r#"<div class="code-block" data-lang="ruby" data-sourcepos="1:1-3:3">"#,
            r#"<div class="code-head"><span class="code-lang">ruby</span>"#,
            r#"<button type="button" class="code-copy" aria-label="Copy code">Copy</button></div>"#,
            r#"<pre><code class="language-ruby"><span class="hl-source hl-ruby">"#,
        )),
        "{h}"
    );
    assert!(h.trim_end().ends_with("</code></pre></div>"), "{h}");
}

#[test]
fn block_without_language() {
    let h = r("```\nplain & simple\n```\n").html;
    assert_eq!(
        h.trim_end(),
        concat!(
            r#"<div class="code-block" data-sourcepos="1:1-3:3">"#,
            r#"<div class="code-head"><span class="code-lang"></span>"#,
            r#"<button type="button" class="code-copy" aria-label="Copy code">Copy</button></div>"#,
            "<pre><code>plain &amp; simple\n</code></pre></div>",
        )
    );
}

#[test]
fn indented_code_gets_the_same_chrome() {
    let h = r("para\n\n    indented()\n").html;
    assert!(
        h.contains(r#"<div class="code-block" data-sourcepos="3:5-3:14">"#),
        "{h}"
    );
    assert!(h.contains("<pre><code>indented()\n</code></pre>"), "{h}");
}

#[test]
fn diagram_and_math_notes() {
    let h = r("```mermaid\ngraph TD\n```\n\n```math\nx^2\n```\n").html;
    assert!(
        h.contains(concat!(
            r#"Copy</button></div><div class="code-note">Diagram rendering isn't supported yet</div>"#,
            r#"<pre><code class="language-mermaid">graph TD"#
        )),
        "{h}"
    );
    assert!(
        h.contains(concat!(
            r#"<div class="code-note">Math rendering isn't supported yet</div>"#,
            r#"<pre><code class="language-math">x^2"#
        )),
        "{h}"
    );
}

#[test]
fn language_is_escaped_in_attributes() {
    let h = r("```a\"b<i>\nx\n```\n").html;
    assert!(h.contains(r#"data-lang="a&quot;b&lt;i&gt;""#), "{h}");
    assert!(
        h.contains(r#"<span class="code-lang">a"b&lt;i&gt;</span>"#),
        "{h}"
    );
}

#[test]
fn aliases_resolve_to_syntax_names() {
    for tag in ["sh", "bash", "shell", "zsh"] {
        assert_eq!(
            canonical_lang(tag),
            Some("Bourne Again Shell (bash)"),
            "{tag}"
        );
    }
    assert_eq!(canonical_lang("vim"), Some("VimL"));
    assert_eq!(canonical_lang("jsonc"), Some("JSON"));
    // two-face leaves JavaScript (Babel) out of its pure-Rust set; TSX understands JSX.
    assert_eq!(canonical_lang("jsx"), Some("TypeScriptReact"));
    assert_eq!(canonical_lang("Ruby"), Some("Ruby"));
    assert_eq!(canonical_lang("RB"), Some("Ruby"));
    assert_eq!(canonical_lang("rust"), Some("Rust"));
    assert_eq!(canonical_lang("weird"), None);
    assert_eq!(canonical_lang("txt"), None);
    assert_eq!(canonical_lang(""), None);
}

#[test]
fn every_alias_highlights() {
    let tags = "jsx tsx ts typescript js javascript sh bash shell zsh vim rb ruby yml yaml json \
                jsonc diff sql scss python py html markdown md rust toml css";
    for tag in tags.split_whitespace() {
        let html = highlight_to_html(Some(tag), "a = \"b\" <c> & d\n");
        assert!(html.contains("hl-"), "{tag}: {html}");
        assert!(!html.contains("<c>"), "{tag}: {html}");
    }
}

#[test]
fn plain_text_is_escaped() {
    assert_eq!(
        highlight_to_html(None, "a <b> & c\n"),
        "a &lt;b&gt; &amp; c\n"
    );
    assert_eq!(highlight_to_html(Some("weird"), "<i>"), "&lt;i&gt;");
}

#[test]
fn a_huge_token_is_not_highlighted() {
    let src = format!("```bash\necho {}\n```\n", "a".repeat(100_000));
    let start = Instant::now();
    let h = r(&src).html;
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(200), "took {elapsed:?}");
    assert!(!h.contains("hl-"));
    assert!(h.contains(&"a".repeat(100_000)));
}

#[test]
fn minified_json_is_not_inflated() {
    let mut json = String::from("{");
    for i in 0..5000 {
        write!(json, "\"key{i}\":[{i},true,null,\"v{i}\"],").unwrap();
    }
    json.push_str("\"end\":1}");
    assert!(json.len() > 170_000);
    let src = format!("```json\n{json}\n```\n");
    let h = r(&src).html;
    assert!(
        h.len() < 2 * src.len(),
        "{} bytes of HTML from {} bytes",
        h.len(),
        src.len()
    );
    assert!(!h.contains("hl-"));
}

#[test]
fn highlighting_limits() {
    // A line may be 2,000 characters long; one more and the block is shown plain.
    let line = "é".repeat(2_000);
    assert!(highlight_to_html(Some("ruby"), &format!("{line}\n")).contains("hl-"));
    assert!(!highlight_to_html(Some("ruby"), &format!("{line}é\n")).contains("hl-"));
    // A block may be 100 KB.
    let small = "x = 1\n".repeat(1_000);
    assert!(highlight_to_html(Some("ruby"), &small).contains("hl-"));
    let large = "x = 1\n".repeat(20_000);
    assert!(large.len() > 100 * 1024);
    assert_eq!(highlight_to_html(Some("ruby"), &large), large);
}

#[test]
fn warm_up_compiles_common_grammars() {
    warm_up();
    assert!(highlight_to_html(Some("tsx"), "const a = <b />;\n").contains("hl-"));
}
