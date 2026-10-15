//! Text files: core renders each paragraph as one block, and the review text map reads the same
//! blocks, so comments, find and the reading position work as they do on notes.

use std::path::Path;

use lectern_core::render::{render_plain, RenderedDoc};
use lectern_core::review::text::TextMap;

fn plain(source: &str) -> RenderedDoc {
    render_plain(source, Path::new("S:/Notes/My Vault/notes/readme.txt"))
}

/// Each block of the text map as `(start line, end line, text)`.
fn blocks(map: &TextMap) -> Vec<(u32, u32, &str)> {
    map.blocks()
        .iter()
        .map(|b| (b.start_line, b.end_line, map.block_text(b)))
        .collect()
}

#[test]
fn each_paragraph_is_one_block_of_its_lines() {
    let doc = plain("One line.\nTwo lines.\n\n\nThird\n");
    assert_eq!(
        doc.html,
        concat!(
            "<p class=\"lx-plain\" data-sourcepos=\"1:1-2:10\">One line.\nTwo lines.</p>\n",
            "<p class=\"lx-plain\" data-sourcepos=\"5:1-5:5\">Third</p>\n",
        )
    );
}

/// A line of spaces and tabs ends a paragraph as an empty line does; a line's own leading and
/// trailing spaces are kept, as `white-space: pre-wrap` shows them.
#[test]
fn whitespace_only_lines_separate_paragraphs_and_indentation_stays() {
    let doc = plain("  indented\n \t \nnext  \n\u{a0}\nlast");
    assert_eq!(
        doc.html,
        concat!(
            "<p class=\"lx-plain\" data-sourcepos=\"1:1-1:10\">  indented</p>\n",
            "<p class=\"lx-plain\" data-sourcepos=\"3:1-3:6\">next  </p>\n",
            "<p class=\"lx-plain\" data-sourcepos=\"5:1-5:4\">last</p>\n",
        )
    );
}

/// Nothing in a text file is Markdown or HTML: tags, headings, emphasis and front matter are the
/// text the file holds.
#[test]
fn the_text_is_escaped_never_parsed() {
    let source = "---\ntitle: Tide\n---\n# Not a heading\n<script>alert(\"x\")</script> & *stars*\n- [ ] not a task\n";
    let doc = plain(source);
    assert_eq!(
        doc.html,
        concat!(
            "<p class=\"lx-plain\" data-sourcepos=\"1:1-6:16\">---\ntitle: Tide\n---\n",
            "# Not a heading\n&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; *stars*\n",
            "- [ ] not a task</p>\n",
        )
    );
    assert!(doc.outline.is_empty());
    assert!(doc.frontmatter.is_none());
    assert_eq!((doc.tasks.done, doc.tasks.total), (0, 0));
    assert!(!doc.has_unresolved_wikilinks);
}

/// CRLF, a lone CR and a BOM end lines and start the text as they do in a note.
#[test]
fn line_endings_and_a_bom_count_as_in_a_note() {
    let doc = plain("\u{feff}first\r\nsecond\r\n\r\nthird\rfourth\n");
    assert_eq!(
        doc.html,
        concat!(
            "<p class=\"lx-plain\" data-sourcepos=\"1:1-2:6\">first\nsecond</p>\n",
            "<p class=\"lx-plain\" data-sourcepos=\"4:1-5:6\">third\nfourth</p>\n",
        )
    );
}

/// The page's HTML parser drops a NUL, so it shows as the replacement character, as in a note,
/// and the text map counts the same character.
#[test]
fn a_nul_shows_as_the_replacement_character() {
    let source = "a\0b\n";
    assert_eq!(
        plain(source).html,
        "<p class=\"lx-plain\" data-sourcepos=\"1:1-1:3\">a\u{fffd}b</p>\n"
    );
    assert_eq!(TextMap::from_plain(source).text(), "a\u{fffd}b");
}

/// The end column is the last line's length in bytes, as comrak counts it.
#[test]
fn the_end_column_counts_bytes() {
    assert_eq!(
        plain("café 日本\n").html,
        "<p class=\"lx-plain\" data-sourcepos=\"1:1-1:12\">café 日本</p>\n"
    );
}

#[test]
fn the_title_is_the_file_name_and_words_are_counted() {
    let doc = plain("Three small words.\n\nAnd two.\n");
    assert_eq!(doc.title, "readme.txt");
    assert_eq!(doc.word_count, 5);
}

#[test]
fn an_empty_or_blank_file_has_no_blocks() {
    for source in ["", "\n\n", " \t\r\n"] {
        assert_eq!(plain(source).html, "", "{source:?}");
        let map = TextMap::from_plain(source);
        assert!(map.blocks().is_empty(), "{source:?}");
        assert_eq!(map.text(), "", "{source:?}");
    }
}

/// The text map reads the blocks the page shows: each paragraph's text normalised, with its lines,
/// joined by single spaces, and no headings.
#[test]
fn the_text_map_reads_the_rendered_blocks() {
    let source =
        "Readme for the vault.\n  Every word   invented.\n\n# Not a heading\n\n\n<b>bold?</b>\n";
    let map = TextMap::from_plain(source);
    assert_eq!(
        blocks(&map),
        [
            (1, 2, "Readme for the vault. Every word invented."),
            (4, 4, "# Not a heading"),
            (7, 7, "<b>bold?</b>"),
        ]
    );
    assert_eq!(
        map.text(),
        "Readme for the vault. Every word invented. # Not a heading <b>bold?</b>"
    );
    assert!(map.headings().is_empty());
    assert!(map.blocks().iter().all(|b| b.heading.is_none()));
    assert!(map.heading_path_at(4).is_empty());
    assert_eq!(map.lines_of(0, map.text().len()), (1, 7));
}

/// The review text of a note follows its kind: a text file's paragraphs, a note's blocks as
/// rendered.
#[test]
fn a_notes_text_follows_its_kind() {
    let source = "# Plan\n\nShip *it*.\n";
    let text = TextMap::of_note("plan.txt", source);
    assert_eq!(text.text(), "# Plan Ship *it*.");
    assert!(text.headings().is_empty());
    let text = TextMap::of_note("PLAN.TXT", source);
    assert_eq!(text.text(), "# Plan Ship *it*.");
    let note = TextMap::of_note("plan.md", source);
    assert_eq!(note.text(), "Plan Ship it.");
    assert_eq!(note.heading_path_at(3), ["Plan"]);
}
