use std::path::Path;

use lectern_core::library::pathmap::PathMapper;
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
fn tag_line() {
    let h = r("#nvim #motions\n\ntext").html;
    assert!(h.contains(r#"<span class="tag" data-tag="nvim">#nvim</span>"#));
}

#[test]
fn not_tags() {
    let h = r("color #519767, PR #6257, Klass#method, `#code`").html;
    assert!(!h.contains("class=\"tag\""));
}

#[test]
fn tags_with_paths_hyphens_and_unicode() {
    let h = r("See #area/billing, #follow-up. and #café\n").html;
    for tag in ["area/billing", "follow-up", "café"] {
        assert!(
            h.contains(&format!(
                r#"<span class="tag" data-tag="{tag}">#{tag}</span>"#
            )),
            "{tag}: {h}"
        );
    }
    assert!(h.contains("</span>, "), "{h}");
}

#[test]
fn hex_colours_are_not_tags() {
    let h = r("#fff and #DeadBeef but #fffg\n").html;
    assert_eq!(h.matches(r#"class="tag""#).count(), 1, "{h}");
    assert!(h.contains(r#"data-tag="fffg""#), "{h}");
}

#[test]
fn tag_needs_whitespace_or_start_before_it() {
    let h = r("**bold**#x and (#y) and a&#z\n").html;
    assert!(!h.contains(r#"class="tag""#), "{h}");
}

#[test]
fn tag_after_line_break() {
    let h = r("line one\n#next\n").html;
    assert!(h.contains(r#"data-tag="next""#), "{h}");
}

#[test]
fn tags_in_list_items_and_tables() {
    let h = r("- [ ] task #todo\n\n|a|b|\n|-|-|\n|#cell|#second|\n").html;
    for tag in ["todo", "cell", "second"] {
        assert!(h.contains(&format!(r#"data-tag="{tag}""#)), "{tag}: {h}");
    }
}

#[test]
fn no_tags_in_links_images_or_code() {
    let h = r("[#inlink](https://example.com) ![#alt](a.png) `#code`\n\n```\n#block\n```\n").html;
    assert!(!h.contains(r#"class="tag""#), "{h}");
    assert!(h.contains(r##"alt="#alt""##), "{h}");
}
