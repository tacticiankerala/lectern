mod common;

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use lectern_core::frontmatter::{Frontmatter, PropValue};
use lectern_core::library::pathmap::{asset_url, PathMapper};
use lectern_core::render::slug::{slugify, Slugger};
use lectern_core::render::{render, RenderContext, RenderedDoc};
use lectern_core::text::decode;

const ASSET_BASE: &str = "http://lxasset.localhost/";

fn r_at(src: &str, doc_path: &Path) -> RenderedDoc {
    let mapper = PathMapper::default();
    let ctx = RenderContext {
        doc_path,
        index: None,
        mapper: &mapper,
        asset_base: ASSET_BASE,
        trusted_unc_hosts: &[],
    };
    render(src, &ctx)
}

fn r(src: &str) -> RenderedDoc {
    r_at(src, Path::new("/vault/notes/Untitled note.md"))
}

#[test]
fn headings_get_github_slugs_and_outline() {
    let d = r("# Hello World\n## Hello World\n### Café & Co.\n");
    assert!(d.html.contains(r#"id="hello-world""#) && d.html.contains(r#"id="hello-world-1""#));
    assert_eq!(
        d.outline.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        ["hello-world", "hello-world-1", "café--co"]
    );
}

#[test]
fn outline_has_levels_and_plain_text() {
    let d = r("# Top\n\n## The `render()` *pipeline*\n\nSetext\n------\n");
    let outline: Vec<_> = d
        .outline
        .iter()
        .map(|o| (o.level, o.text.as_str(), o.id.as_str()))
        .collect();
    assert_eq!(
        outline,
        [
            (1, "Top", "top"),
            (2, "The render() pipeline", "the-render-pipeline"),
            (2, "Setext", "setext")
        ]
    );
    assert!(d
        .html
        .contains(r#"<h2 id="the-render-pipeline" data-sourcepos="3:1-3:28">"#));
}

#[test]
fn heading_without_slug_text_gets_no_id() {
    let d = r("# 🎉\n");
    assert_eq!(d.outline[0].id, "");
    assert!(d.html.contains(r#"<h1 data-sourcepos="1:1-1:6">"#));
}

#[test]
fn slugify_follows_github() {
    assert_eq!(slugify("Hello, World!"), "hello-world");
    assert_eq!(slugify("snake_case and-kebab"), "snake_case-and-kebab");
    assert_eq!(slugify("Ünïcödé 中文 ✅"), "ünïcödé-中文-");
    assert_eq!(slugify("Section 2"), "section-2");
}

#[test]
fn slugify_keeps_marks_and_connector_punctuation() {
    // GitHub keeps \p{L}, \p{M}, \p{N} and \p{Pc}, turns spaces into `-` and keeps `-`.
    assert_eq!(slugify("Cafe\u{301}"), "cafe\u{301}");
    assert_eq!(slugify("a\u{203f}b"), "a\u{203f}b");
    // Other whitespace is dropped, not turned into `-`.
    assert_eq!(slugify("a\tb\u{a0}c"), "abc");
}

#[test]
fn heading_ids_keep_combining_marks() {
    let d = r("# Cafe\u{301}\n");
    assert_eq!(d.outline[0].id, "cafe\u{301}");
    assert!(d.html.contains("id=\"cafe\u{301}\""), "{}", d.html);
}

#[test]
fn slugger_never_repeats_an_id() {
    let mut s = Slugger::new();
    let ids: Vec<String> = ["a", "a", "a-1", "a"].iter().map(|t| s.slug(t)).collect();
    assert_eq!(ids, ["a", "a-1", "a-1-1", "a-2"]);
}

#[test]
fn sourcepos_present() {
    assert!(r("para\n").html.contains("data-sourcepos=\"1:1-1:4\""));
}

#[test]
fn tasks_counted() {
    let d = r("- [ ] a\n- [x] b\n- [X] c\n");
    assert_eq!((d.tasks.done, d.tasks.total), (2, 3));
}

#[test]
fn task_checkboxes_survive_sanitising() {
    let html = r("- [x] done\n- [ ] open\n").html;
    assert!(
        html.contains(r#"<input type="checkbox" checked="" disabled="">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<input type="checkbox" disabled="">"#),
        "{html}"
    );
}

#[test]
fn title_prefers_frontmatter_then_h1_then_stem() {
    let path = Path::new("/vault/notes/My Note.md");
    assert_eq!(
        r_at("---\ntitle: From Frontmatter\n---\n# The H1\n", path).title,
        "From Frontmatter"
    );
    assert_eq!(r_at("## Not an h1\n\n# The H1\n", path).title, "The H1");
    assert_eq!(r_at("No headings at all.\n", path).title, "My Note");
}

#[test]
fn tables_wrapped() {
    assert!(r("|a|b|\n|-|-|\n|1|2|\n")
        .html
        .contains(r#"<div class="table-wrap"><table"#));
}

#[test]
fn table_wrapper_closes_after_the_table() {
    let html = r("|a|b|\n|-|-|\n|1|2|\n\nafter\n").html;
    assert!(html.contains("</table></div>"), "{html}");
}

#[test]
fn bare_urls_autolinked() {
    assert!(r("see https://example.com/x now")
        .html
        .contains(r#"href="https://example.com/x""#));
}

#[test]
fn frontmatter_extracted_not_rendered() {
    let d = r("---\nstatus: done\n---\n# T\n");
    assert!(!d.html.contains("status: done"));
    assert!(d.frontmatter.is_some());
}

#[test]
fn invalid_frontmatter_is_reported_not_rendered() {
    let d = r("---\na: [unclosed\n---\nbody\n");
    assert!(matches!(d.frontmatter, Some(Frontmatter::Invalid { .. })));
    assert!(!d.html.contains("unclosed"));
}

#[test]
fn frontmatter_with_crlf_line_endings() {
    let d = r("---\r\nstatus: done\r\n---\r\n# T\r\n");
    let Some(Frontmatter::Parsed { entries }) = d.frontmatter else {
        panic!("{:?}", d.frontmatter)
    };
    assert_eq!(entries[0].value, PropValue::Text("done".into()));
    assert_eq!(d.title, "T");
}

#[test]
fn adjacent_delimiters_are_empty_frontmatter() {
    for src in ["---\n---\n# Title\n", "---\r\n---\r\n# Title\r\n"] {
        let d = r(src);
        assert!(!d.html.contains("<hr"), "{src:?}: {}", d.html);
        assert_eq!(
            d.frontmatter,
            Some(Frontmatter::Parsed { entries: vec![] }),
            "{src:?}"
        );
        assert!(
            d.html
                .contains(r#"<h1 id="title" data-sourcepos="3:1-3:7">"#),
            "{src:?}: {}",
            d.html
        );
    }
}

#[test]
fn a_lone_rule_is_not_frontmatter() {
    let d = r("---\n\n# Title\n");
    assert!(d.frontmatter.is_none());
    assert!(d.html.contains("<hr"), "{}", d.html);
}

#[test]
fn no_frontmatter_is_none() {
    assert!(r("# T\n").frontmatter.is_none());
}

#[test]
fn word_count() {
    assert_eq!(r("one two three\n\n`code` four").word_count, 5);
}

#[test]
fn word_count_skips_code_blocks_and_frontmatter() {
    let d = r("---\ntitle: not counted here\n---\none\n\n```\nnot counted\n```\n\n**two**three\n");
    assert_eq!(d.word_count, 2);
}

#[test]
fn raw_html_is_sanitised() {
    let html = r(concat!(
        "<script>alert(1)</script>\n\n",
        "<img src=x onerror=alert(1)>\n\n",
        "[click](javascript:alert(1))\n\n",
        "<a href=\"#top\" onclick=\"steal()\">ok</a>\n",
    ))
    .html;
    assert!(!html.contains("<script"), "{html}");
    // The raw-HTML policy shows the script block as text; no other `alert(1)` survives.
    assert!(
        html.contains("<p data-sourcepos=\"1:1-1:25\">&lt;script&gt;alert(1)&lt;/script&gt;</p>"),
        "{html}"
    );
    assert_eq!(html.matches("alert(1)").count(), 1, "{html}");
    assert!(!html.contains("onerror"), "{html}");
    assert!(!html.contains("javascript:"), "{html}");
    assert!(!html.contains("onclick"), "{html}");
    // The raw image survives as a lazy asset-protocol image, without its handler.
    let src = asset_url(ASSET_BASE, &Path::new("/vault/notes").join("x"));
    assert!(
        html.contains(&format!(
            r#"<img src="{src}" loading="lazy" decoding="async">"#
        )),
        "{html}"
    );
    assert!(html.contains(r##"<a href="#top">ok</a>"##), "{html}");
}

#[test]
fn five_mb_document_renders_quickly_enough() {
    let mut src = String::with_capacity(5 * 1024 * 1024 + 1024);
    let mut sections = 0;
    while src.len() < 5 * 1024 * 1024 {
        sections += 1;
        let i = sections;
        write!(
            src,
            "## Heading {i}\n\n\
             Paragraph {i} with *emphasis*, `src/lib_{i}.rs`, a [link](https://example.com/{i}) \
             and a bare https://example.com/bare/{i} URL.\n\n\
             | Name | Path | Notes |\n| --- | --- | --- |\n| row {i} | `a/b/{i}.md` | fine |\n\n\
             - [ ] open task {i}\n- [x] done task {i}\n\n"
        )
        .unwrap();
    }

    let start = Instant::now();
    let d = r(&src);
    let elapsed = start.elapsed();

    assert_eq!(d.outline.len(), sections);
    assert_eq!(
        (d.tasks.done, d.tasks.total),
        (sections as u32, 2 * sections as u32)
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "rendering 5 MB took {elapsed:?}"
    );
}

#[test]
fn asset_url_encodes_spaces_unicode_unc() {
    assert_eq!(
        asset_url(
            "http://lxasset.localhost/",
            std::path::Path::new(r"S:\Notes\My Vault\a é.png")
        ),
        "http://lxasset.localhost/S%3A%5CNotes%5CMy%20Vault%5Ca%20%C3%A9.png"
    );
    assert_eq!(
        asset_url(
            "http://lxasset.localhost/",
            std::path::Path::new(r"\\nas\Shared\it's (1).png")
        ),
        "http://lxasset.localhost/%5C%5Cnas%5CShared%5Cit's%20(1).png"
    );
}

const BIG_PLAN: &str = "work/alpha/plans/2026-01-01-big-plan.md";

/// Every Markdown fixture, rendered without an index and pinned. Binary files with a `.md` name
/// are skipped, and so is the big plan, which `big_plan_structure` covers instead of a snapshot
/// that every rendering change would rewrite. The vault's path is redacted, since links and
/// images carry absolute paths.
#[test]
fn fixtures() {
    let vault = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/vault")
        .canonicalize()
        .unwrap();
    insta::glob!("../../../fixtures/vault", "**/*.md", |path| {
        if path.ends_with(BIG_PLAN) {
            return;
        }
        let bytes = std::fs::read(path).unwrap();
        let Ok(decoded) = decode(&bytes) else { return };
        let doc = r_at(&decoded.text, path);
        insta::assert_snapshot!(common::redact_vault(&describe(&doc), &vault));
    });
}

/// Exact counts for the generated big plan. They follow from `fixtures/gen-big-plan.mjs`: 15
/// sections of 3 tasks, 5 steps per task, steps ticked in the first 2 sections, and per task one
/// table and 4 code fences (3 inside the steps, 1 after them).
#[test]
fn big_plan_structure() {
    const SECTIONS: usize = 15;
    const TASKS: usize = SECTIONS * 3;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/vault")
        .join(BIG_PLAN);
    let d = r_at(&std::fs::read_to_string(&path).unwrap(), &path);

    // The h1, an h2 per section, an h3 per task and the closing "Wrap-up" h2.
    assert_eq!(d.outline.len(), 1 + SECTIONS + TASKS + 1);
    assert_eq!(d.tasks.total, (TASKS * 5) as u32);
    assert_eq!(d.tasks.done, (2 * 3 * 5) as u32);
    assert_eq!(d.html.matches(r#"class="table-wrap""#).count(), TASKS);
    assert_eq!(d.html.matches("<pre").count(), TASKS * 4);
    assert_eq!(d.title, "Big Plan");
}

/// A readable dump of everything `render` returns, for snapshots.
fn describe(d: &RenderedDoc) -> String {
    let mut s = String::new();
    writeln!(s, "title: {}", d.title).unwrap();
    writeln!(s, "words: {}", d.word_count).unwrap();
    writeln!(s, "tasks: {}/{}", d.tasks.done, d.tasks.total).unwrap();
    writeln!(s, "unresolved wikilinks: {}", d.has_unresolved_wikilinks).unwrap();
    match &d.frontmatter {
        None => writeln!(s, "frontmatter: none").unwrap(),
        Some(Frontmatter::Parsed { entries }) => {
            writeln!(s, "frontmatter:").unwrap();
            for p in entries {
                writeln!(s, "  {}: {:?}", p.key, p.value).unwrap();
            }
        }
        Some(Frontmatter::Invalid { raw, error }) => {
            writeln!(s, "frontmatter: invalid ({error})\n{raw}").unwrap();
        }
    }
    writeln!(s, "outline:").unwrap();
    for o in &d.outline {
        writeln!(
            s,
            "  {}h{} #{} {}",
            "  ".repeat(usize::from(o.level - 1)),
            o.level,
            o.id,
            o.text
        )
        .unwrap();
    }
    writeln!(s, "---- html ----").unwrap();
    s.push_str(&d.html);
    s
}
