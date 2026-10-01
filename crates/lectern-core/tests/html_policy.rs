use std::path::Path;

use lectern_core::library::pathmap::{asset_url, PathMapper};
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
fn placeholders_are_literal() {
    let h = r("Use <slug> and <friend> here.").html;
    assert!(h.contains("&lt;slug&gt;") && h.contains("&lt;friend&gt;"));
}

#[test]
fn prompt_xml_tags_literal() {
    let h = r("<writing_type>\nessay\n</writing_type>\n").html;
    assert!(h.contains("&lt;writing_type&gt;"));
}

#[test]
fn script_never_survives() {
    let h = r("<script>alert(1)</script>\n\nx <script>bad()</script>").html;
    assert!(!h.contains("<script"));
}

#[test]
fn event_handlers_stripped() {
    let h = r(r#"<img src="x.png" onerror="alert(1)">"#).html;
    assert!(!h.contains("onerror"));
}

#[test]
fn javascript_href_stripped() {
    assert!(!r("[c](javascript:alert(1))").html.contains("javascript:"));
}

#[test]
fn allowlisted_kept() {
    let h =
        r("a<br>b <kbd>Ctrl</kbd>\n\n<details><summary>More</summary>\n\nhidden\n\n</details>\n")
            .html;
    assert!(
        h.contains("<br")
            && h.contains("<kbd>Ctrl</kbd>")
            && h.contains("<details>")
            && h.contains("<summary>More</summary>")
    );
}

#[test]
fn br_in_table_cell() {
    assert!(r("|a|b|\n|-|-|\n|x<br>y|z|\n").html.contains("x<br"));
}

#[test]
fn custom_tag_block_becomes_literal_paragraph() {
    let h = r("<context>\nThe user is a <role>.\n</context>\n").html;
    assert!(
        h.contains(r#"<p data-sourcepos="1:1-3:10">&lt;context&gt;<br>"#),
        "{h}"
    );
    assert!(h.contains("The user is a &lt;role&gt;.<br>"), "{h}");
    assert!(h.contains("&lt;/context&gt;</p>"), "{h}");
}

#[test]
fn custom_inline_tags_are_literal() {
    let h = r("Wrap it in <context>this</context>, then render <Loading />.").html;
    assert!(
        h.contains("&lt;context&gt;this&lt;/context&gt;") && h.contains("&lt;Loading /&gt;"),
        "{h}"
    );
}

#[test]
fn style_iframe_object_embed_are_literal() {
    let h = r(concat!(
        "<style>body { display: none }</style>\n\n",
        "<iframe src=\"https://example.com\"></iframe>\n\n",
        "a <object data=\"x\"></object> <embed src=\"y\"> b\n",
    ))
    .html;
    for tag in ["style", "iframe", "object", "embed"] {
        assert!(!h.contains(&format!("<{tag}")), "{tag}: {h}");
        assert!(h.contains(&format!("&lt;{tag}")), "{tag}: {h}");
    }
}

#[test]
fn html_comments_dropped() {
    let h = r("a <!-- inline note --> b\n\n<!-- block\ncomment -->\n\nafter\n").html;
    assert!(!h.contains("inline note") && !h.contains("comment"), "{h}");
    assert!(!h.contains("&lt;!--"), "{h}");
    assert!(h.contains("after"), "{h}");
}

#[test]
fn raw_ids_kept_names_stripped() {
    let h = r(concat!(
        "# Intro\n\n",
        "Jump <a id=\"custom-anchor\" name=\"old-anchor\"></a>here.\n\n",
        "<div id='box' class=\"note\">y</div>\n",
    ))
    .html;
    assert!(h.contains(r#"<h1 id="intro""#), "{h}");
    assert!(h.contains(r#"<a id="custom-anchor"></a>"#), "{h}");
    assert!(!h.contains("name="), "{h}");
    assert!(h.contains(r#"<div id="box" class="note">y</div>"#), "{h}");
}

#[test]
fn content_after_a_block_comment_is_kept() {
    let h = r("<!-- note --><div>Visible</div>\n").html;
    assert_eq!(h.trim(), "<div>Visible</div>");

    let h = r("<!-- a\nmulti-line note --><div>Also visible</div>\n").html;
    assert_eq!(h.trim(), "<div>Also visible</div>");

    let h = r("<!-- note --><context>x</context> and text\n").html;
    assert_eq!(
        h.trim(),
        r#"<p data-sourcepos="1:1-1:42">&lt;context&gt;x&lt;/context&gt; and text</p>"#
    );

    let h = r("<!-- only a\ncomment -->\n\nafter\n").html;
    assert_eq!(h.trim(), r#"<p data-sourcepos="4:1-4:5">after</p>"#);
}

#[test]
fn comments_stay_hidden_in_literal_blocks() {
    let h = r(concat!(
        "<context>\n",
        "before\n",
        "<!-- hidden -->\n",
        "after <!-- also\n",
        "hidden --> end\n",
        "</context>\n",
    ))
    .html;
    assert!(!h.contains("hidden") && !h.contains("&lt;!--"), "{h}");
    assert!(h.contains("before<br>\nafter  end<br>"), "{h}");
}

#[test]
fn details_open_and_picture_sources_kept() {
    let h = r(concat!(
        "<details open><summary>More</summary>body</details>\n\n",
        "<picture><source srcset=\"dark.png\" media=\"(prefers-color-scheme: dark)\" ",
        "type=\"image/png\"><img src=\"light.png\" alt=\"logo\"></picture>\n",
    ))
    .html;
    assert!(h.contains(r#"<details open="">"#), "{h}");
    // The source survives, its local srcset pointed at the file beside the note.
    let dark = asset_url(
        "http://asset.localhost/",
        &Path::new("/vault/notes").join("dark.png"),
    );
    assert!(
        h.contains(&format!(
            r#"<source srcset="{dark}" media="(prefers-color-scheme: dark)" type="image/png">"#
        )),
        "{h}"
    );
}
