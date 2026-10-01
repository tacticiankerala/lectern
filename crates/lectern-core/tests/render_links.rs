mod common;

use std::path::{Path, PathBuf};

use lectern_core::library::pathmap::{asset_url, PathMapper};
use lectern_core::library::LibraryIndex;
use lectern_core::render::{render, RenderContext, RenderedDoc};

const ASSET_BASE: &str = "http://asset.localhost/";

/// The fixture vault, copied and indexed.
struct Vault {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    ix: LibraryIndex,
}

impl Vault {
    fn new() -> Self {
        let (tmp, root) = common::vault_copy();
        let ix = common::index_of(&root);
        Self {
            _tmp: tmp,
            root,
            ix,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        common::native_join(&self.root, rel)
    }

    /// `rel` as an absolute path string, the way `data-target` carries it.
    fn abs(&self, rel: &str) -> String {
        self.path(rel).to_string_lossy().into_owned()
    }

    /// Renders `src` as if it were the note at `rel`, with the index.
    fn render(&self, rel: &str, src: &str) -> RenderedDoc {
        self.render_with(rel, src, Some(&self.ix), &PathMapper::default())
    }

    /// Renders `src` as if it were the note at `rel`, before any index exists.
    fn render_unindexed(&self, rel: &str, src: &str) -> RenderedDoc {
        self.render_with(rel, src, None, &PathMapper::default())
    }

    fn render_with(
        &self,
        rel: &str,
        src: &str,
        index: Option<&LibraryIndex>,
        mapper: &PathMapper,
    ) -> RenderedDoc {
        let doc_path = self.path(rel);
        let ctx = RenderContext {
            doc_path: &doc_path,
            index,
            mapper,
            asset_base: ASSET_BASE,
        };
        render(src, &ctx)
    }

    /// Renders the fixture note at `rel`, with the index.
    fn render_fixture(&self, rel: &str) -> RenderedDoc {
        let src = std::fs::read_to_string(self.path(rel)).unwrap();
        self.render(rel, &src)
    }

    fn redact(&self, text: &str) -> String {
        common::redact_vault(text, &self.root)
    }
}

fn wsl_mapper() -> PathMapper {
    PathMapper {
        wsl_distro: Some("Ubuntu-26.04".into()),
        ..PathMapper::default()
    }
}

#[track_caller]
fn assert_has(html: &str, needle: &str) {
    assert!(html.contains(needle), "missing {needle}\nin {html}");
}

#[track_caller]
fn assert_lacks(html: &str, needle: &str) {
    assert!(!html.contains(needle), "unexpected {needle}\nin {html}");
}

#[test]
fn render_marks_links() {
    let v = Vault::new();

    let notes = v
        .render_fixture("work/alpha/notes/2026-01-02-notes.md")
        .html;
    assert_has(
        &notes,
        &format!(
            r##"<a href="#" class="code-link" data-kind="doc" data-target="{}"><code data-sourcepos="5:34-5:63">plans/2026-01-01-big-plan.md</code></a>"##,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
    assert_has(
        &notes,
        &format!(
            r#"data-kind="doc" data-target="{}" data-line="3">x</a>"#,
            v.abs("work/alpha/README.md")
        ),
    );

    let index = v.render_fixture("memory/index.md");
    assert_has(
        &index.html,
        r##"<a data-sourcepos="9:3-9:18" href="#" class="wikilink broken" data-kind="broken" title="No note named missing-note">missing-note</a>"##,
    );
    assert!(index.has_unresolved_wikilinks);

    let readme = v.render_fixture("README.md").html;
    assert_has(
        &readme,
        &format!(
            r#"class="link-doc" data-kind="doc" data-target="{}">r</a>"#,
            v.abs("notes/résumé notes.md")
        ),
    );

    let friends = v.render_fixture("friends/readme-style.md").html;
    assert_has(
        &friends,
        &format!(
            r#"<img data-sourcepos="7:1-7:21" src="{}" alt="logo" loading="lazy" decoding="async">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
    assert_has(
        &friends,
        r#"src="https://example.com/badges/build-passing.svg" alt="build" loading="lazy" decoding="async">"#,
    );
}

#[test]
fn wikilinks_resolve_with_anchor_and_title() {
    let v = Vault::new();
    let index = v.render_fixture("memory/index.md").html;
    assert_has(
        &index,
        &format!(
            r##"<a data-sourcepos="5:3-5:12" href="#" class="wikilink" data-kind="doc" data-target="{}">README</a>"##,
            v.abs("README.md")
        ),
    );
    assert_has(
        &index,
        &format!(
            r#"class="wikilink" data-kind="doc" data-target="{}">Vault home</a>"#,
            v.abs("README.md")
        ),
    );
    assert_has(
        &index,
        &format!(
            r#"class="wikilink" data-kind="doc" data-target="{}" data-anchor="Section 2" data-slug="section-2">big-plan#Section 2</a>"#,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
    assert_eq!(index.matches(r#"class="wikilink broken""#).count(), 1);
    assert_lacks(&index, "data-wikilink");
}

#[test]
fn wikilinks_that_all_resolve_leave_the_flag_clear() {
    let v = Vault::new();
    let d = v.render(
        "memory/index.md",
        "[[README]] and [[feedback-incremental-prs]]\n",
    );
    assert!(!d.has_unresolved_wikilinks);
}

#[test]
fn without_an_index_wikilinks_render_broken_but_are_not_counted() {
    let v = Vault::new();
    let d = v.render_unindexed("memory/index.md", "[[README]] and [[missing-note]]\n");
    assert_has(
        &d.html,
        r#"class="wikilink broken" data-kind="broken" title="No note named README">README</a>"#,
    );
    assert!(!d.has_unresolved_wikilinks);
}

#[test]
fn broken_wikilink_title_names_the_note_not_the_heading() {
    let v = Vault::new();
    let html = v
        .render("memory/index.md", "[[nowhere#Some heading]]\n")
        .html;
    assert_has(&html, r#"title="No note named nowhere""#);
}

#[test]
fn relative_markdown_link_is_a_doc_link() {
    let v = Vault::new();
    let html = v
        .render(
            "work/alpha/notes/x.md",
            "[plan](../plans/2026-01-01-big-plan.md#Section%202) [up](../README.md#L12)\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r##"href="#" class="link-doc" data-kind="doc" data-target="{}" data-anchor="Section 2" data-slug="section-2">plan</a>"##,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
    assert_has(
        &html,
        &format!(
            r#"data-kind="doc" data-target="{}" data-line="12">up</a>"#,
            v.abs("work/alpha/README.md")
        ),
    );
}

#[test]
fn relative_link_resolves_against_ancestors_and_drops_the_query() {
    let v = Vault::new();
    let html = v
        .render(
            "work/alpha/notes/x.md",
            "[plan](plans/2026-01-01-big-plan.md?plain=1)\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"data-kind="doc" data-target="{}">plan</a>"#,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
}

#[test]
fn missing_relative_markdown_link_is_still_a_doc_link() {
    let v = Vault::new();
    let html = v
        .render("work/alpha/README.md", "[gone](notes/gone.md)\n")
        .html;
    assert_has(
        &html,
        &format!(
            r#"data-kind="doc" data-target="{}">gone</a>"#,
            v.abs("work/alpha/notes/gone.md")
        ),
    );
}

#[test]
fn without_an_index_relative_markdown_links_join_the_doc_folder() {
    let v = Vault::new();
    let d = v.render_unindexed(
        "work/alpha/notes/x.md",
        "[plan](../plans/2026-01-01-big-plan.md) and `../README.md`\n",
    );
    assert_has(
        &d.html,
        &format!(
            r#"class="link-doc" data-kind="doc" data-target="{}">plan</a>"#,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
    // Inline code needs the index to confirm a file exists.
    assert_lacks(&d.html, "code-link");
}

#[test]
fn link_to_another_file_is_a_file_link() {
    let v = Vault::new();
    let html = v
        .render("friends/readme-style.md", "[logo](img/logo.png)\n")
        .html;
    assert_has(
        &html,
        &format!(
            r##"href="#" data-kind="file" data-target="{}">logo</a>"##,
            v.abs("friends/img/logo.png")
        ),
    );
}

#[test]
fn absolute_links_map_or_stay_unverified() {
    let v = Vault::new();
    let src = "[k](/home/dev/projects/app/k.rb:17) [stale](/x/vault/work/beta/README.md)\n";
    let html = v
        .render_with("README.md", src, Some(&v.ix), &wsl_mapper())
        .html;
    assert_has(
        &html,
        r##"href="#" data-kind="path" data-target="\\wsl.localhost\Ubuntu-26.04\home\dev\projects\app\k.rb" data-line="17">k</a>"##,
    );
    assert_has(
        &html,
        &format!(
            r#"data-kind="doc" data-target="{}">stale</a>"#,
            v.abs("archive/beta/README.md")
        ),
    );

    // Nothing maps it: the path as written, checked on click.
    let html = v.render("README.md", src).html;
    assert_has(
        &html,
        r#"data-kind="path" data-target="/home/dev/projects/app/k.rb" data-line="17">k</a>"#,
    );
}

#[test]
fn external_and_in_page_links() {
    let v = Vault::new();
    let html = v
        .render(
            "README.md",
            "[site](https://example.com/a?b=1 \"Title\") <mail@example.com> [top](#Caf%C3%A9) [s](#section-2) [jump](#Section.One)\n",
        )
        .html;
    assert_has(
        &html,
        r#"href="https://example.com/a?b=1" data-kind="external" title="Title">site</a>"#,
    );
    assert_has(
        &html,
        r#"href="mailto:mail@example.com" data-kind="external">mail@example.com</a>"#,
    );
    // The fragment stays as written, for an exact id; the slug is the fallback.
    assert_has(
        &html,
        r##"href="#Caf%C3%A9" data-kind="anchor" data-slug="café">top</a>"##,
    );
    assert_has(
        &html,
        r##"href="#section-2" data-kind="anchor" data-slug="section-2">s</a>"##,
    );
    assert_has(
        &html,
        r##"href="#Section.One" data-kind="anchor" data-slug="sectionone">jump</a>"##,
    );
}

#[test]
fn link_hrefs_are_percent_decoded_once() {
    let v = Vault::new();
    let html = v
        .render(
            "README.md",
            "[a](notes/r%C3%A9sum%C3%A9%20notes.md) [b](odd%zz.md)\n",
        )
        .html;
    assert_has(
        &html,
        &format!(r#"data-target="{}">a</a>"#, v.abs("notes/résumé notes.md")),
    );
    assert_has(
        &html,
        &format!(r#"data-target="{}">b</a>"#, v.abs("odd%zz.md")),
    );
}

#[test]
fn unsafe_link_schemes_are_still_dropped() {
    let v = Vault::new();
    let html = v.render("README.md", "[x](javascript:alert(1))\n").html;
    assert_lacks(&html, "javascript:");
    assert_lacks(&html, "data-kind");
}

#[test]
fn link_targets_are_escaped() {
    let v = Vault::new();
    let html = v.render("README.md", "[x](<a\"b&c.md>)\n").html;
    let target = v
        .abs("a\"b&c.md")
        .replace('&', "&amp;")
        .replace('"', "&quot;");
    assert_has(&html, &format!(r#"data-target="{target}""#));
}

#[test]
fn inline_code_paths_resolve_against_the_index() {
    let v = Vault::new();
    let html = v
        .render(
            "work/alpha/notes/x.md",
            "`../README.md` `README.md` `img/logo.png` `plans/2026-01-01-big-plan.md:40` `nope/missing.md`\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"class="code-link" data-kind="doc" data-target="{}"><code"#,
            v.abs("work/alpha/README.md")
        ),
    );
    assert_eq!(
        html.matches(&format!(
            r#"data-target="{}""#,
            v.abs("work/alpha/README.md")
        ))
        .count(),
        2,
        "{html}"
    );
    assert_has(
        &html,
        &format!(
            r#"data-kind="doc" data-target="{}" data-line="40"><code"#,
            v.abs("work/alpha/plans/2026-01-01-big-plan.md")
        ),
    );
    // `img/logo.png` lives under friends/, not near this note; `nope/missing.md` nowhere.
    assert_eq!(html.matches("code-link").count(), 3, "{html}");
    assert_has(
        &html,
        "<code data-sourcepos=\"1:28-1:41\">img/logo.png</code>",
    );
    assert_has(
        &html,
        "<code data-sourcepos=\"1:77-1:93\">nope/missing.md</code>",
    );
}

#[test]
fn inline_code_files_and_absolute_paths() {
    let v = Vault::new();
    let html = v
        .render_with(
            "friends/readme-style.md",
            "`img/logo.png` `/home/dev/projects/app/k.rb:17:3` `S:\\Notes\\My Vault\\x.md`\n",
            Some(&v.ix),
            &wsl_mapper(),
        )
        .html;
    assert_has(
        &html,
        &format!(
            r##"<a href="#" class="code-link" data-kind="file" data-target="{}"><code"##,
            v.abs("friends/img/logo.png")
        ),
    );
    assert_has(
        &html,
        r#"class="code-link" data-kind="path" data-target="\\wsl.localhost\Ubuntu-26.04\home\dev\projects\app\k.rb" data-line="17"><code"#,
    );
    assert_has(
        &html,
        r#"class="code-link" data-kind="path" data-target="S:\Notes\My Vault\x.md"><code"#,
    );
}

#[test]
fn inline_code_non_paths_untouched() {
    let v = Vault::new();
    let html = v
        .render(
            "README.md",
            "`foo()` `a/b` `v1.2` `notes/résumé notes.md` `x.rb` `work/alpha/`\n",
        )
        .html;
    assert_lacks(&html, "code-link");
    assert_has(&html, "<code data-sourcepos=\"1:1-1:7\">foo()</code>");
}

#[test]
fn inline_code_inside_a_link_is_not_linked_twice() {
    let v = Vault::new();
    let html = v
        .render(
            "work/alpha/README.md",
            "[`plans/2026-01-01-big-plan.md`](https://example.com)\n",
        )
        .html;
    assert_lacks(&html, "code-link");
}

#[test]
fn images_use_the_asset_protocol() {
    let v = Vault::new();
    let html = v
        .render_with(
            "friends/readme-style.md",
            "![a](img/logo.png \"Logo\") ![b](missing/pic.png) ![c](/mnt/c/pics/a%20b.png)\n",
            Some(&v.ix),
            &PathMapper::default(),
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"src="{}" alt="a" title="Logo" loading="lazy" decoding="async">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
    assert_has(
        &html,
        &format!(
            r#"src="{}" alt="b""#,
            asset_url(ASSET_BASE, &v.path("friends/missing/pic.png"))
        ),
    );
    assert_has(
        &html,
        &format!(
            r#"src="{}" alt="c""#,
            asset_url(ASSET_BASE, Path::new(r"C:\pics\a b.png"))
        ),
    );
}

#[test]
fn raw_html_images_use_the_asset_protocol() {
    let v = Vault::new();
    let html = v.render_fixture("friends/readme-style.md").html;
    assert_has(
        &html,
        &format!(
            r#"<img src="{}" alt="Lectern logo" width="64" height="64" loading="lazy" decoding="async">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );

    let html = v
        .render(
            "friends/readme-style.md",
            "Inline <img alt='x' SRC=img/logo.png loading=eager> and <img src=\"https://example.com/a.png\">\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"<img alt="x" src="{}" loading="eager" decoding="async">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
    assert_has(
        &html,
        r#"<img src="https://example.com/a.png" loading="lazy" decoding="async">"#,
    );
}

#[test]
fn without_an_index_images_join_the_doc_folder() {
    let v = Vault::new();
    let html = v
        .render_unindexed("friends/readme-style.md", "![logo](img/logo.png)\n")
        .html;
    assert_has(
        &html,
        &format!(
            r#"src="{}""#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
}

/// The fixtures with links, rendered with the index and pinned. `render_basics` pins every
/// fixture rendered without one.
#[test]
fn indexed_fixtures() {
    let v = Vault::new();
    for rel in [
        "README.md",
        "friends/readme-style.md",
        "memory/index.md",
        "work/alpha/README.md",
        "work/alpha/notes/2026-01-02-notes.md",
    ] {
        let d = v.render_fixture(rel);
        let text = format!(
            "unresolved wikilinks: {}\n---- html ----\n{}",
            d.has_unresolved_wikilinks, d.html
        );
        insta::assert_snapshot!(
            format!("indexed@{}", rel.replace('/', "__")),
            v.redact(&text)
        );
    }
}

#[test]
fn raw_html_image_sources_are_entity_decoded() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "<img src=\"img/logo&#46;png\"> <img src=\"https&#58;//example.com/a.png\">\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"<img src="{}" loading="lazy" decoding="async">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
    assert_has(
        &html,
        r#"<img src="https://example.com/a.png" loading="lazy" decoding="async">"#,
    );
}

#[test]
fn an_img_inside_an_attribute_value_is_not_an_image() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "<div title=\"<img src='img/logo.png'>\">x</div>\n",
        )
        .html;
    assert_lacks(&html, "asset.localhost");
    assert_lacks(&html, "loading=");
    assert_has(&html, "src='img/logo.png'");
}

#[test]
fn raw_source_srcset_uses_the_asset_protocol() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "<picture><source srcset=\"img/logo.png 2x, https://example.com/b.png 3x\"><img src=\"img/logo.png\"></picture>\n",
        )
        .html;
    assert_has(
        &html,
        &format!(
            r#"<source srcset="{} 2x, https://example.com/b.png 3x">"#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
}

#[test]
fn protocol_relative_urls_stay_remote() {
    let v = Vault::new();
    let html = v
        .render_with(
            "README.md",
            "[site](//example.com/page) ![a](//example.com/a.png) <img src=\"//example.com/b.png\">\n",
            Some(&v.ix),
            &wsl_mapper(),
        )
        .html;
    assert_has(
        &html,
        r#"href="//example.com/page" data-kind="external">site</a>"#,
    );
    assert_has(&html, r#"src="//example.com/a.png" alt="a""#);
    assert_has(&html, r#"<img src="//example.com/b.png" loading="lazy""#);
    assert_lacks(&html, "wsl.localhost");
    assert_lacks(&html, "asset.localhost");
}

#[test]
fn image_fragments_follow_the_asset_url() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "![icon](img/sprite.svg#icon) <img src=\"img/sprite.svg#other\">\n",
        )
        .html;
    let url = asset_url(ASSET_BASE, &v.path("friends/img/sprite.svg"));
    assert_has(&html, &format!(r#"src="{url}#icon" alt="icon""#));
    assert_has(&html, &format!(r#"<img src="{url}#other""#));
}

#[test]
fn inline_code_inside_a_raw_anchor_is_not_linked_twice() {
    let v = Vault::new();
    let html = v
        .render(
            "work/alpha/notes/x.md",
            concat!(
                "<a href=\"https://example.com\">`README.md`</a> then `README.md`\n\n",
                "<A HREF=\"https://example.com\">**`README.md`**</A>\n\n",
                "<a href=\"https://example.com\">unclosed\n\n",
                "`README.md` in the next paragraph\n",
            ),
        )
        .html;
    // Only the span after `</a>` and the one in the last paragraph link.
    assert_eq!(html.matches("code-link").count(), 2, "{html}");
    assert_has(
        &html,
        r#"<a href="https://example.com"><code data-sourcepos="1:31-1:41">README.md</code></a>"#,
    );
}

#[test]
fn absolute_code_paths_directly_under_a_root() {
    let v = Vault::new();
    let html = v.render("README.md", "`/x.md` and `C:\\x y.md`\n").html;
    assert_has(
        &html,
        r#"class="code-link" data-kind="path" data-target="/x.md"><code"#,
    );
    assert_has(
        &html,
        r#"class="code-link" data-kind="path" data-target="C:\x y.md"><code"#,
    );
}

#[test]
fn snapshot_redaction_reads_the_same_on_windows() {
    let vault = Path::new(r"C:\Users\runner\AppData\Local\Temp\.tmp1\vault");
    let text = concat!(
        r#"<a data-target="C:\Users\runner\AppData\Local\Temp\.tmp1\vault\notes\a b.md">x</a>"#,
        r#"<img src="http://asset.localhost/C%3A%5CUsers%5Crunner%5CAppData%5CLocal%5CTemp%5C.tmp1%5Cvault%5Cimg%5Cl.png">"#,
        r#"<p>a \| b and \sum</p>"#,
    );
    assert_eq!(
        common::redact_vault(text, vault),
        concat!(
            r#"<a data-target="[vault]/notes/a b.md">x</a>"#,
            r#"<img src="http://asset.localhost/[vault]%2Fimg%2Fl.png">"#,
            r#"<p>a \| b and \sum</p>"#,
        )
    );
}

#[test]
fn srcset_urls_with_commas_stay_whole() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "<picture><source srcset=\"https://example.com/a,b.png 1x\"><img src=\"img/logo.png\"></picture>\n",
        )
        .html;
    assert_has(&html, r#"<source srcset="https://example.com/a,b.png 1x">"#);
}

#[test]
fn raw_html_image_sources_are_trimmed_before_classifying() {
    let v = Vault::new();
    let html = v
        .render(
            "friends/readme-style.md",
            "<img src=\" https://example.com/a.png \"> <img src=\" img/logo.png \">\n",
        )
        .html;
    assert_has(
        &html,
        r#"<img src=" https://example.com/a.png " loading="lazy""#,
    );
    assert_has(
        &html,
        &format!(
            r#"<img src="{}" loading="lazy""#,
            asset_url(ASSET_BASE, &v.path("friends/img/logo.png"))
        ),
    );
    assert_eq!(html.matches("asset.localhost").count(), 1, "{html}");
}
