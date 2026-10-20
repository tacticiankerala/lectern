//! Targets and reading, in core: where a heading, a line or a quote is in a note or a text file
//! (in the text as shown, else, for a quote with Markdown marks, in the source), which targets each
//! kind of file takes, what a window's UI is sent and answers for them, and `read`'s pages of the
//! rendered text.

use lectern_core::agent::read::{
    read_page, ReadBlock, ReadText, PAGE_TEXT_CUT, READ_BYTES, READ_DEFAULT,
};
use lectern_core::agent::request::{HighlightArgs, Target};
use lectern_core::agent::review::comment_anchor;
use lectern_core::agent::status::{AgentCall, AgentReply};
use lectern_core::agent::target::{
    check_target, pdf_suffix_page, resolve_target, show_anchor, HighlightAnswer, ShowAt, TextPlace,
};
use lectern_core::agent::tools::{parse_call, CallError, READ_MAX};
use lectern_core::agent::AgentRequest;
use lectern_core::ipc::PathForms;
use lectern_core::library::DocKind;
use lectern_core::review::text::{PageBlock, TextMap};
use serde_json::json;

/// Plan, with Rollout › Steps and Risks › Steps, a passage that is there twice, and one with a
/// Markdown mark. Lines: Plan 1, Intro 3, Rollout 5, its passage 7, Steps 9, the mark 11, Risks
/// 13, Steps 15, the passage again 17.
const PLAN: &str = "# Plan\n\nIntro text.\n\n## Rollout\n\nRoll back the migration first.\n\n### Steps\n\nThe plan **must** ship.\n\n## Risks\n\n### Steps\n\nRoll back the migration first, then call.\n";

fn heading(text: &str) -> Target {
    Target::Heading {
        heading: text.to_owned(),
    }
}

fn quote(text: &str, prefix: Option<&str>, suffix: Option<&str>) -> Target {
    Target::Quote {
        quote: text.to_owned(),
        prefix: prefix.map(str::to_owned),
        suffix: suffix.map(str::to_owned),
    }
}

/// Where `target` is in the note `source`, named plan.md.
fn place(source: &str, target: &Target) -> Result<TextPlace, String> {
    resolve_target(&TextMap::build(source), source, "plan.md", target)
}

/// `at`, for a test whose own `at` is a closure.
fn at_(source: &str, found: &str, nth: usize, lines: (u32, u32)) -> TextPlace {
    at(source, found, nth, lines)
}

/// The place of the text `found` in the note's text, at its `nth` occurrence (from 0), counted as
/// the UI counts it, in UTF-16 code units, on the lines given; and where it starts in its block.
fn at(source: &str, found: &str, nth: usize, lines: (u32, u32)) -> TextPlace {
    let map = TextMap::build(source);
    let text = map.text();
    let start = text.match_indices(found).nth(nth).expect("in the text").0;
    let units =
        |from: usize, to: usize| u32::try_from(text[from..to].encode_utf16().count()).unwrap();
    let block = map
        .blocks()
        .iter()
        .rev()
        .find(|b| b.start <= start)
        .expect("in a block");
    TextPlace {
        start_line: lines.0,
        end_line: lines.1,
        text_start: Some(units(0, start)),
        text_end: Some(units(0, start + found.len())),
        block_offset: Some(units(block.start, start)),
    }
}

// Headings.

#[test]
fn a_heading_is_found_by_its_text_in_any_letter_case() {
    assert_eq!(
        place(PLAN, &heading("rollout")),
        Ok(at(PLAN, "Rollout", 0, (5, 5)))
    );
    assert_eq!(
        place(PLAN, &heading("PLAN")),
        Ok(at(PLAN, "Plan", 0, (1, 1)))
    );
    // Marks and a leading `#` an agent may copy from the source don't count.
    assert_eq!(
        place(PLAN, &heading("## **Rollout**")),
        Ok(at(PLAN, "Rollout", 0, (5, 5)))
    );
}

#[test]
fn a_heading_is_found_by_its_path() {
    assert_eq!(
        place(PLAN, &heading("Risks › Steps")),
        Ok(at(PLAN, "Steps", 1, (15, 15)))
    );
    assert_eq!(
        place(PLAN, &heading("plan › rollout › steps")),
        Ok(at(PLAN, "Steps", 0, (9, 9)))
    );
    // A path may leave out the headings between: Plan is over both Steps; the first is taken.
    assert_eq!(
        place(PLAN, &heading("Plan › Steps")),
        Ok(at(PLAN, "Steps", 0, (9, 9)))
    );
    // Of two with the text alone, the first.
    assert_eq!(
        place(PLAN, &heading("Steps")),
        Ok(at(PLAN, "Steps", 0, (9, 9)))
    );
}

#[test]
fn of_the_headings_a_target_matches_the_nearest_by_depth_is_taken() {
    let source = "# Plan\n\n## Phase one\n\n### Rollout\n\n## Rollout\n";
    // Plan › Rollout is nearer the target's depth than Plan › Phase one › Rollout.
    assert_eq!(
        place(source, &heading("Rollout")),
        Ok(at(source, "Rollout", 1, (7, 7)))
    );
    assert_eq!(
        place(source, &heading("Phase one › Rollout")),
        Ok(at(source, "Rollout", 0, (5, 5)))
    );
}

#[test]
fn a_heading_that_is_not_there_is_an_error() {
    assert_eq!(
        place(PLAN, &heading("Rollback")),
        Err("There's no heading “Rollback” in plan.md.".to_owned())
    );
    // The path must hold: Risks isn't over Rollout.
    assert_eq!(
        place(PLAN, &heading("Risks › Rollout")),
        Err("There's no heading “Risks › Rollout” in plan.md.".to_owned())
    );
}

// Lines.

#[test]
fn a_line_that_is_its_own_block_is_that_block() {
    assert_eq!(
        place(PLAN, &Target::Line { line: 7 }),
        Ok(at(PLAN, "Roll back the migration first.", 0, (7, 7)))
    );
    // A heading is its block, whatever its line.
    assert_eq!(
        place(PLAN, &Target::Line { line: 5 }),
        Ok(at(PLAN, "Rollout", 0, (5, 5)))
    );
    let setext = "Rollout\n=======\n\nText.\n";
    assert_eq!(
        place(setext, &Target::Line { line: 2 }),
        Ok(at(setext, "Rollout", 0, (2, 2)))
    );
}

/// A line inside a longer paragraph is that line, as shown: its own text, Markdown marks aside,
/// not the paragraph's.
#[test]
fn a_line_inside_a_paragraph_is_that_line_alone() {
    let source = "First line\nsecond line.\n";
    assert_eq!(
        place(source, &Target::Line { line: 2 }),
        Ok(at(source, "second line.", 0, (2, 2)))
    );
    assert_eq!(
        place(source, &Target::Line { line: 1 }),
        Ok(at(source, "First line", 0, (1, 1)))
    );
    let marked = "# Plan\n\nThe plan **must**\nship with [the guide](guide.md)\nand `cargo   test`\n- a list item\n  that wraps.\n";
    assert_eq!(
        place(marked, &Target::Line { line: 4 }),
        Ok(at(marked, "ship with the guide", 0, (4, 4)))
    );
    assert_eq!(
        place(marked, &Target::Line { line: 5 }),
        Ok(at(marked, "and cargo test", 0, (5, 5)))
    );
    assert_eq!(
        place(marked, &Target::Line { line: 7 }),
        Ok(at(marked, "that wraps.", 0, (7, 7)))
    );
}

/// A line is found by the line breaks a paragraph holds, a link's title and raw HTML over a break
/// included, after which comrak's own positions of the text are off.
#[test]
fn a_line_after_a_title_or_raw_html_over_a_break_is_that_line() {
    let titled = "See [the guide](guide.md \"one\ntwo\") first.\nThen this.\n";
    assert_eq!(
        place(titled, &Target::Line { line: 3 }),
        Ok(at(titled, "Then this.", 0, (3, 3)))
    );
    let html = "Text <span\nclass=\"x\">inside</span> after\nlast line\n";
    assert_eq!(
        place(html, &Target::Line { line: 2 }),
        Ok(at(html, "inside after", 0, (2, 2)))
    );
    assert_eq!(
        place(html, &Target::Line { line: 3 }),
        Ok(at(html, "last line", 0, (3, 3)))
    );
}

/// A word that runs over a line break (raw HTML between its halves, no space shown): the lines
/// can't be told apart, so the paragraph's lines stand, whole, never a wrong line or none.
#[test]
fn a_word_across_a_line_break_makes_its_paragraph_whole() {
    let source = "one<span\nclass=\"x\">two</span>\n";
    assert_eq!(
        place(source, &Target::Line { line: 2 }),
        Ok(at(source, "onetwo", 0, (2, 2)))
    );
    assert_eq!(
        place(source, &quote("two", None, None)).map(|p| (p.start_line, p.end_line)),
        Ok((1, 2))
    );
    let text = TextMap::build(source);
    let at_line = comment_anchor(&text, source, "plan.md", &Target::Line { line: 2 }).unwrap();
    assert_eq!(
        (at_line.start_line, at_line.end_line, at_line.quote.as_str()),
        (1, 2, "onetwo")
    );
    // A paragraph whose words keep to their lines is still taken line by line.
    let plain = "one <span\nclass=\"x\">two</span>\n";
    assert_eq!(
        place(plain, &Target::Line { line: 2 }),
        Ok(at(plain, "two", 0, (2, 2)))
    );
}

/// A line map whose count comes out right can still be wrong: a reference link's title holds a
/// line break that is in its definition, not in the paragraph, and the parse drops a comment that
/// runs over a line break, whose break then goes uncounted. The map is checked against the source,
/// word by word, and a paragraph whose words aren't on the lines it says is taken whole: never a
/// line with no text while it shows some, and never a quote on the wrong line.
#[test]
fn a_line_map_whose_words_are_not_on_their_lines_is_not_trusted() {
    let source =
        "alpha [beta][ref]\ngamma <!--\n-->delta\nepsilon\n\n[ref]: /x \"two-line\ntitle\"\n";
    let whole = "alpha beta gamma delta epsilon";
    for line in 1..=4 {
        assert_eq!(
            place(source, &Target::Line { line }),
            Ok(at(source, whole, 0, (line, line))),
            "line {line}"
        );
    }
    assert_eq!(
        place(source, &quote("gamma", None, None)).map(|p| (p.start_line, p.end_line)),
        Ok((1, 4))
    );
    let text = TextMap::build(source);
    let at_line = comment_anchor(&text, source, "plan.md", &Target::Line { line: 2 }).unwrap();
    assert_eq!(
        (at_line.start_line, at_line.end_line, at_line.quote.as_str()),
        (1, 4, whole)
    );
}

/// A line shows other characters than it holds where it has an entity or a backslash escape. The
/// check reads an escape, and `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;`, as what they show, so
/// such a line is still its own; a code span's backslashes are as written. Any other entity makes
/// the paragraph whole: never a wrong line.
#[test]
fn a_line_with_an_entity_or_an_escape_is_its_own_or_its_paragraph_whole() {
    let source =
        "First line\nsecond &lt;line&gt; with snake\\_case &amp; \\\\ more\nthird `\\.\\d+` line\n";
    assert_eq!(
        place(source, &Target::Line { line: 2 }),
        Ok(at(
            source,
            "second <line> with snake_case & \\ more",
            0,
            (2, 2)
        ))
    );
    assert_eq!(
        place(source, &Target::Line { line: 3 }),
        Ok(at(source, "third \\.\\d+ line", 0, (3, 3)))
    );
    let other = "one\n&copy; two\nthree\n";
    let whole = "one © two three";
    for line in 1..=3 {
        assert_eq!(
            place(other, &Target::Line { line }),
            Ok(at(other, whole, 0, (line, line))),
            "line {line}"
        );
    }
    assert_eq!(
        place(other, &quote("three", None, None)).map(|p| (p.start_line, p.end_line)),
        Ok((1, 3))
    );
}

/// An attribute and a visible word that share a long prefix (160 KB of it, twice) don't make the
/// source check retry at every character: a line with an entity or an escape is read once, and
/// the build stays linear. The lines are their own, or the block's: never wrong.
#[test]
fn a_long_shared_prefix_in_an_attribute_keeps_the_source_check_linear() {
    let a = "a".repeat(160 * 1024);
    let source = format!("first line\nprefix <span title=\"{a}\">{a}&amp;Z</span>\nlast line\n");
    let started = std::time::Instant::now();
    let text = TextMap::build(&source);
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    let line = |n| resolve_target(&text, &source, "plan.md", &Target::Line { line: n });
    assert_eq!(line(1), Ok(at(&source, "first line", 0, (1, 1))));
    assert_eq!(
        line(2),
        Ok(at(&source, &format!("prefix {a}&Z"), 0, (2, 2)))
    );
    assert_eq!(line(3), Ok(at(&source, "last line", 0, (3, 3))));
}

/// Markup glued to a word (`` `Vec`s ``, `**bold**text`, `*really*.`) makes a word of the text that
/// is nowhere in the source as one: each run of text between marks is checked on its own, so the
/// lines stay their own.
#[test]
fn a_word_glued_to_its_markup_keeps_its_line() {
    let source = "The `Vec`s are **bold**text,\n*really*. See [the guide](guide.md).\n";
    assert_eq!(
        place(source, &Target::Line { line: 1 }),
        Ok(at(source, "The Vecs are boldtext,", 0, (1, 1)))
    );
    assert_eq!(
        place(source, &Target::Line { line: 2 }),
        Ok(at(source, "really. See the guide.", 0, (2, 2)))
    );
}

/// A text file of one long paragraph (no blank line in 300): a line far down it is that line.
#[test]
fn a_line_of_a_long_text_file_paragraph_is_that_line_alone() {
    let source: String = (1..=300)
        .map(|n| format!("Survey entry {n:03}: the lanternfish count.\n"))
        .collect();
    let text = TextMap::from_plain(&source);
    let found = resolve_target(&text, &source, "survey.txt", &Target::Line { line: 250 }).unwrap();
    let wanted = "Survey entry 250: the lanternfish count.";
    let start = text.text().find(wanted).unwrap();
    assert_eq!(
        found,
        TextPlace {
            start_line: 250,
            end_line: 250,
            text_start: Some(u32::try_from(start).unwrap()),
            text_end: Some(u32::try_from(start + wanted.len()).unwrap()),
            // The file is one paragraph, from the text's start.
            block_offset: Some(u32::try_from(start).unwrap()),
        }
    );
}

/// A line of a fenced code block is that line of code; its fences have no text.
#[test]
fn a_line_of_a_code_block_is_that_line_alone() {
    // The fence on line 3, code on lines 4 to 63, the closing fence on line 64.
    let code: String = (4..=63)
        .map(|n| format!("    step_{n}(  batch );\n"))
        .collect();
    let source = format!("# Plan\n\n```rust\n{code}```\n\nAfter.\n");
    assert_eq!(
        place(&source, &Target::Line { line: 50 }),
        Ok(at(&source, "step_50( batch );", 0, (50, 50)))
    );
    for fence in [3, 64] {
        assert_eq!(
            place(&source, &Target::Line { line: fence }),
            Ok(TextPlace {
                start_line: fence,
                end_line: fence,
                text_start: None,
                text_end: None,
                block_offset: None,
            }),
            "line {fence}"
        );
    }
}

#[test]
fn a_line_without_text_has_only_its_line_and_one_past_the_end_is_an_error() {
    assert_eq!(
        place(PLAN, &Target::Line { line: 6 }),
        Ok(TextPlace {
            start_line: 6,
            end_line: 6,
            text_start: None,
            text_end: None,
            block_offset: None,
        })
    );
    assert_eq!(
        place(PLAN, &Target::Line { line: 18 }),
        Err("plan.md has 17 lines; line 18 is past its end.".to_owned())
    );
}

// Quotes.

#[test]
fn a_quote_is_found_in_the_text_as_shown_whitespace_aside() {
    assert_eq!(
        place(PLAN, &quote("Intro\n   text.", None, None)),
        Ok(at(PLAN, "Intro text.", 0, (3, 3)))
    );
}

#[test]
fn a_quote_that_appears_twice_is_chosen_by_its_prefix_or_suffix() {
    let passage = "Roll back the migration first";
    // The first, with nothing to tell them apart.
    assert_eq!(
        place(PLAN, &quote(passage, None, None)),
        Ok(at(PLAN, passage, 0, (7, 7)))
    );
    // The text before the second: the heading over it, with or without the space.
    assert_eq!(
        place(PLAN, &quote(passage, Some("Risks\nSteps"), None)),
        Ok(at(PLAN, passage, 1, (17, 17)))
    );
    assert_eq!(
        place(PLAN, &quote(passage, Some("Steps "), None)),
        Ok(at(PLAN, passage, 1, (17, 17)))
    );
    assert_eq!(
        place(PLAN, &quote(passage, None, Some(", then call."))),
        Ok(at(PLAN, passage, 1, (17, 17)))
    );
    assert_eq!(
        place(PLAN, &quote(passage, Some("Rollout"), Some(". Steps"))),
        Ok(at(PLAN, passage, 0, (7, 7)))
    );
}

#[test]
fn a_quote_with_markdown_marks_is_found_in_the_source_by_its_lines() {
    // `**must**` in the source, "must" on screen.
    assert_eq!(
        place(PLAN, &quote("The plan **must** ship.", None, None)),
        Ok(TextPlace {
            start_line: 11,
            end_line: 11,
            text_start: None,
            text_end: None,
            block_offset: None,
        })
    );
    // Over a soft line break, and told apart by its prefix in the source.
    let source = "A **bold\nword** here.\n\nAgain a **bold\nword** there.\n";
    assert_eq!(
        place(source, &quote("**bold word**", Some("Again a"), None)),
        Ok(TextPlace {
            start_line: 4,
            end_line: 5,
            text_start: None,
            text_end: None,
            block_offset: None,
        })
    );
}

#[test]
fn a_quote_that_is_not_there_is_an_error() {
    assert_eq!(
        place(PLAN, &quote("roll forward", None, None)),
        Err("That passage isn't in plan.md.".to_owned())
    );
}

#[test]
fn offsets_are_counted_as_the_ui_counts_them() {
    let source = "# Café ☕\n\nNaïve 𝄞 text here.\n";
    let found = place(source, &quote("text here", None, None)).unwrap();
    // "Café ☕ Naïve 𝄞 " before it: 𝄞 is two UTF-16 units.
    let before = "Café ☕ Naïve 𝄞 ".encode_utf16().count() as u32;
    assert_eq!(found.text_start, Some(before));
    assert_eq!(found.text_end, Some(before + 9));
}

// Anchors.

fn anchor(text: &str) -> Target {
    Target::Anchor {
        anchor: text.to_owned(),
    }
}

/// A `#anchor` after a path names a heading as the reader's own links do: the heading whose id is
/// the anchor as written, else its slug. The show goes to that heading, its target the anchor as
/// the agent wrote it.
#[test]
fn an_anchor_names_a_heading_by_its_id_or_its_slug() {
    let source = "# Plan\n\n## Rollout plan\n\nText.\n\n## Notes\n\n## Notes\n";
    let text = TextMap::build(source);
    let at = |anchor: &str| show_anchor(&text, source, "plan.md", anchor);
    let rollout = Some(at_(source, "Rollout plan", 0, (3, 3)));
    assert_eq!(
        at("rollout-plan"),
        Ok(ShowAt {
            target: anchor("rollout-plan"),
            place: rollout.clone(),
        })
    );
    // Not an id, the anchor is slugged as the reader's links slug it.
    assert_eq!(
        at("Rollout Plan"),
        Ok(ShowAt {
            target: anchor("Rollout Plan"),
            place: rollout,
        })
    );
    // Two headings alike: the second's id has a number after it.
    assert_eq!(
        at("notes").map(|a| a.place),
        Ok(Some(at_(source, "Notes", 0, (7, 7))))
    );
    assert_eq!(
        at("notes-1").map(|a| a.place),
        Ok(Some(at_(source, "Notes", 1, (9, 9))))
    );
    assert_eq!(
        at("risks"),
        Err("There's no heading #risks in plan.md.".to_owned())
    );
    // A text file has no headings.
    let notes = "Rollout plan\n\nText.\n";
    assert_eq!(
        show_anchor(
            &TextMap::from_plain(notes),
            notes,
            "notes.txt",
            "rollout-plan"
        ),
        Err("There's no heading #rollout-plan in notes.txt.".to_owned())
    );
}

/// A show's anchor is found again as it was found first (a window finding its target again in the
/// file as it shows it): the same heading, the second of two alike included, never the first by
/// its text; one no heading has is the same error.
#[test]
fn an_anchor_is_found_again_as_it_was_found_first() {
    let source = "# Plan\n\n## Rollout plan\n\nText.\n\n## Notes\n\n## Notes\n";
    let text = TextMap::build(source);
    for wanted in ["notes", "notes-1", "Rollout Plan"] {
        let shown = show_anchor(&text, source, "plan.md", wanted).unwrap();
        assert_eq!(
            resolve_target(&text, source, "plan.md", &shown.target).map(Some),
            Ok(shown.place),
            "{wanted}"
        );
    }
    assert_eq!(
        place(source, &anchor("notes-1")),
        Ok(at_(source, "Notes", 1, (9, 9)))
    );
    assert_eq!(
        place(source, &anchor("risks")),
        Err("There's no heading #risks in plan.md.".to_owned())
    );
    let notes = "Rollout plan\n\nText.\n";
    assert_eq!(
        resolve_target(
            &TextMap::from_plain(notes),
            notes,
            "notes.txt",
            &anchor("rollout-plan")
        ),
        Err("There's no heading #rollout-plan in notes.txt.".to_owned())
    );
}

#[test]
fn a_text_file_has_its_paragraphs_and_no_headings() {
    let source = "Notes\n\nRoll back first.\nThen call.\n";
    let text = TextMap::from_plain(source);
    // On the line it sits on, of its paragraph's two; across both, on both.
    assert_eq!(
        resolve_target(&text, source, "notes.txt", &quote("Then call", None, None))
            .map(|p| (p.start_line, p.end_line)),
        Ok((4, 4))
    );
    assert_eq!(
        resolve_target(
            &text,
            source,
            "notes.txt",
            &quote("first. Then", None, None)
        )
        .map(|p| (p.start_line, p.end_line)),
        Ok((3, 4))
    );
    assert_eq!(
        resolve_target(&text, source, "notes.txt", &heading("Notes")),
        Err("There's no heading “Notes” in notes.txt.".to_owned())
    );
}

// Which targets each kind of file takes.

#[test]
fn each_kind_of_file_takes_its_own_targets() {
    let page = Target::Page { page: 3 };
    let line = Target::Line { line: 3 };
    for target in [heading("Plan"), line.clone(), quote("x", None, None)] {
        assert_eq!(check_target(DocKind::Markdown, &target, "plan.md"), Ok(()));
        assert_eq!(check_target(DocKind::Text, &target, "notes.txt"), Ok(()));
    }
    for kind in [DocKind::Markdown, DocKind::Text] {
        assert_eq!(
            check_target(kind, &page, "plan.md"),
            Err("A page target is for PDFs; give a heading, a line or a quote.".to_owned())
        );
    }
    for target in [heading("Plan"), quote("x", None, None)] {
        assert_eq!(check_target(DocKind::Html, &target, "report.html"), Ok(()));
    }
    for target in [line.clone(), page.clone()] {
        assert_eq!(
            check_target(DocKind::Html, &target, "report.html"),
            Err("report.html is an HTML page: give a heading or a quote.".to_owned())
        );
    }
    assert_eq!(check_target(DocKind::Pdf, &page, "brief.pdf"), Ok(()));
    for target in [heading("Plan"), line, quote("x", None, None)] {
        assert_eq!(
            check_target(DocKind::Pdf, &target, "brief.pdf"),
            Err("brief.pdf is a PDF: give a page target.".to_owned())
        );
    }
}

/// Where a passage starts in the text of its own block, in UTF-16 code units, so the window finds
/// it in that block alone, never indexing the whole document: a line of a long paragraph, a quote
/// starting a later paragraph, a line of a code block, past a character UTF-16 counts as two.
#[test]
fn a_place_says_where_it_starts_in_its_own_block() {
    let source =
        "# Plan\n\nIntro.\n\nHi 😀 there\nsecond line.\n\n```\nlet a = 1;\nlet b = 2;\n```\n";
    let text = TextMap::build(source);
    let found = |target: &Target| resolve_target(&text, source, "plan.md", target).unwrap();
    // "Hi 😀 there second line.": "second" is 12 units in.
    assert_eq!(found(&Target::Line { line: 6 }).block_offset, Some(12));
    assert_eq!(found(&Target::Line { line: 5 }).block_offset, Some(0));
    assert_eq!(found(&quote("Intro", None, None)).block_offset, Some(0));
    assert_eq!(
        found(&quote("there second", None, None)).block_offset,
        Some(6)
    );
    // The code "let a = 1; let b = 2;": line 10 is 11 in.
    assert_eq!(found(&Target::Line { line: 10 }).block_offset, Some(11));
    // Only the lines of a quote found in the source: no text, so no offset either.
    let marked = "# Plan\n\nThe plan **must** ship.\n";
    let place = resolve_target(
        &TextMap::build(marked),
        marked,
        "plan.md",
        &quote("plan **must** ship", None, None),
    )
    .unwrap();
    assert_eq!((place.text_start, place.block_offset), (None, None));
}

/// An `#anchor` after an HTML page's path is a target of its own, which its window finds; a note
/// resolves its anchor to the heading, so no other file takes one.
#[test]
fn only_an_html_page_takes_an_anchor_target() {
    let anchor = Target::Anchor {
        anchor: "rollout-plan".to_owned(),
    };
    assert_eq!(check_target(DocKind::Html, &anchor, "report.html"), Ok(()));
    for (kind, file) in [(DocKind::Markdown, "plan.md"), (DocKind::Text, "notes.txt")] {
        assert_eq!(
            check_target(kind, &anchor, file),
            Err(format!(
                "Only an HTML page takes #rollout-plan as a target, not {file}."
            ))
        );
    }
    assert_eq!(
        check_target(DocKind::Pdf, &anchor, "brief.pdf"),
        Err("brief.pdf is a PDF: give a page target.".to_owned())
    );
    // A show's `#anchor` after a note's path, found again, is the heading it names (as
    // `show_anchor` found it first).
    let source = "# Plan\n\n## Rollout plan\n";
    let text = TextMap::of_note("plan.md", source).unwrap();
    assert_eq!(
        resolve_target(&text, source, "plan.md", &anchor).map(|p| p.start_line),
        Ok(3)
    );
}

/// What follows a PDF's path is its page when it is the standard `#page=N` (in any letter case),
/// checked as a page target is, with its messages; anything else is an error saying what a PDF
/// takes.
#[test]
fn a_pdfs_page_fragment_is_a_page_target() {
    for (fragment, page) in [("page=3", 3), ("PAGE=2", 2), ("Page=12", 12)] {
        assert_eq!(
            pdf_suffix_page(Some(fragment), "brief.pdf"),
            Ok(Target::Page { page }),
            "{fragment}"
        );
    }
    // A page target's own message, as `tools/call` gives it.
    let Err(CallError::Invalid(bad_page)) = parse_call(
        "show",
        json!({"path": r"S:\Notes\brief.pdf", "target": {"page": 0}})
            .as_object()
            .unwrap(),
    ) else {
        panic!("a page 0 checks out");
    };
    for fragment in ["page=0", "page=x", "page=", "page=-1"] {
        assert_eq!(
            pdf_suffix_page(Some(fragment), "brief.pdf"),
            Err(bad_page.clone()),
            "{fragment}"
        );
    }
    let takes = "brief.pdf is a PDF: give #page=N after its path, or a page target.";
    for fragment in [Some("intro"), Some("pages=3"), None] {
        assert_eq!(
            pdf_suffix_page(fragment, "brief.pdf"),
            Err(takes.to_owned()),
            "{fragment:?}"
        );
    }
}

// What a window's UI is sent and answers.

#[test]
fn a_window_is_sent_the_places_core_found_and_answers_how_many_highlights_it_shows() {
    let place = TextPlace {
        start_line: 7,
        end_line: 8,
        text_start: Some(24),
        text_end: Some(53),
        block_offset: Some(5),
    };
    let call = AgentCall {
        id: 4,
        request: AgentRequest::Highlight(HighlightArgs {
            path: Some(r"S:\Notes\My Vault\plan.md".to_owned()),
            targets: vec![Target::Line { line: 7 }],
            clear: false,
        }),
        agent: "Claude".to_owned(),
        places: vec![place.clone()],
        within_ms: None,
    };
    let sent = serde_json::to_value(&call).unwrap();
    assert_eq!(
        sent["places"],
        json!([{"startLine": 7, "endLine": 8, "textStart": 24, "textEnd": 53, "blockOffset": 5}])
    );
    assert_eq!(serde_json::from_value::<AgentCall>(sent).unwrap(), call);
    // A call with no places leaves them out, and is read without them.
    let status: AgentCall =
        serde_json::from_value(json!({"id": 1, "request": {"tool": "status"}, "agent": "Claude"}))
            .unwrap();
    assert!(status.places.is_empty());
    assert_eq!(
        serde_json::to_value(TextPlace {
            text_start: None,
            text_end: None,
            block_offset: None,
            ..place
        })
        .unwrap(),
        json!({"startLine": 7, "endLine": 8})
    );
    assert_eq!(
        serde_json::from_value::<AgentReply>(json!({"kind": "highlighted", "count": 3})).unwrap(),
        AgentReply::Highlighted { count: 3 }
    );
    assert_eq!(
        serde_json::from_value::<AgentReply>(json!({
            "kind": "pageText",
            "blocks": [{"text": "Title", "headingLevel": 1}, {"text": "Body."}],
        }))
        .unwrap(),
        AgentReply::PageText {
            blocks: vec![
                PageBlock {
                    text: "Title".to_owned(),
                    heading_level: Some(1),
                },
                PageBlock {
                    text: "Body.".to_owned(),
                    heading_level: None,
                },
            ],
            truncated: false,
        }
    );
    // A page whose text stopped at its bridge's limit says so.
    assert_eq!(
        serde_json::from_value::<AgentReply>(json!({
            "kind": "pageText",
            "blocks": [],
            "truncated": true,
        }))
        .unwrap(),
        AgentReply::PageText {
            blocks: vec![],
            truncated: true,
        }
    );
}

#[test]
fn a_target_held_for_a_starting_window_keeps_its_place() {
    let held = ShowAt {
        target: Target::Quote {
            quote: "Roll back".to_owned(),
            prefix: None,
            suffix: None,
        },
        place: Some(TextPlace {
            start_line: 7,
            end_line: 7,
            text_start: Some(24),
            text_end: Some(33),
            block_offset: None,
        }),
    };
    assert_eq!(
        serde_json::to_value(&held).unwrap(),
        json!({
            "target": {"quote": "Roll back"},
            "place": {"startLine": 7, "endLine": 7, "textStart": 24, "textEnd": 33},
        })
    );
    let page = ShowAt {
        target: Target::Page { page: 3 },
        place: None,
    };
    assert_eq!(
        serde_json::to_value(&page).unwrap(),
        json!({"target": {"page": 3}})
    );
}

#[test]
fn highlight_answers_the_file_the_window_and_how_many_it_shows() {
    let answer = HighlightAnswer {
        file: PathForms {
            windows: Some(r"S:\Notes\My Vault\plan.md".to_owned()),
            wsl: Some("/mnt/s/Notes/My Vault/plan.md".to_owned()),
        },
        window: "main".to_owned(),
        highlights: 2,
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({
            "file": {"windows": r"S:\Notes\My Vault\plan.md", "wsl": "/mnt/s/Notes/My Vault/plan.md"},
            "window": "main",
            "highlights": 2,
        })
    );
}

// Reading.

/// No file: the forms an answer gives when a path has none.
fn no_file() -> PathForms {
    PathForms {
        windows: None,
        wsl: None,
    }
}

/// A page of `map`'s text, as `read` gives it for a file with no forms.
fn read_text(map: &TextMap, lines: bool, from: Option<u32>, max: Option<u32>) -> ReadText {
    read_page(no_file(), map, lines, None, from, max).read
}

fn block(start: u32, line: Option<u32>, heading: Option<u32>) -> ReadBlock {
    ReadBlock {
        start,
        line,
        heading,
    }
}

/// Each block's text in a page: from its start to the next one's, the joining space aside.
fn pieces(read: &ReadText) -> Vec<String> {
    let chars: Vec<char> = read.text.chars().collect();
    let starts: Vec<usize> = read.blocks.iter().map(|b| b.start as usize).collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &from)| {
            let to = starts.get(i + 1).copied().unwrap_or(chars.len());
            chars[from..to].iter().collect::<String>().trim().to_owned()
        })
        .collect()
}

#[test]
fn a_note_reads_as_its_text_once_with_where_each_block_starts_its_line_and_headings() {
    let map = TextMap::build(PLAN);
    let read = read_text(&map, true, None, None);
    assert_eq!(read.text, map.text());
    assert!(!read.truncated);
    assert_eq!(read.next, None);
    // Each heading path once; a block names its own by its place in the list.
    assert_eq!(
        read.headings,
        [
            "Plan",
            "Plan › Rollout",
            "Plan › Rollout › Steps",
            "Plan › Risks",
            "Plan › Risks › Steps",
        ]
    );
    assert_eq!(read.blocks.len(), 9);
    assert_eq!(read.blocks[0], block(0, Some(1), Some(0)));
    assert_eq!(read.blocks[1], block(5, Some(3), Some(0)));
    assert_eq!(read.blocks[3], block(25, Some(7), Some(1)));
    assert_eq!(read.blocks[8], block(94, Some(17), Some(4)));
    assert_eq!(pieces(&read)[3], "Roll back the migration first.");
    assert_eq!(
        pieces(&read)[8],
        "Roll back the migration first, then call."
    );
    // Above the first heading, no heading.
    let read = read_text(&TextMap::build("Loose.\n\n# Then\n"), true, None, None);
    assert_eq!(read.blocks[0], block(0, Some(1), None));
    assert_eq!(read.headings, ["Then"]);
}

#[test]
fn a_read_is_paged_by_character_from_where_the_last_stopped() {
    let map = TextMap::build(PLAN);
    let first = read_text(&map, true, Some(5), Some(11));
    assert_eq!(first.text, "Intro text.");
    assert!(first.truncated);
    assert_eq!(first.next, Some(16));
    assert_eq!(first.headings, ["Plan"]);
    assert_eq!(first.blocks, [block(0, Some(3), Some(0))]);
    // A page that ends inside a block has that much of it; a block starts where it does in the
    // page.
    let second = read_text(&map, true, first.next, Some(12));
    assert_eq!(second.text, " Rollout Rol");
    assert_eq!(second.headings, ["Plan › Rollout"]);
    assert_eq!(
        second.blocks,
        [block(1, Some(5), Some(0)), block(9, Some(7), Some(0))]
    );
    assert_eq!(pieces(&second), ["Rollout", "Rol"]);
    // One that began on the page before starts this one.
    let third = read_text(&map, true, Some(28), Some(4));
    assert_eq!(third.text, "l ba");
    assert_eq!(third.blocks, [block(0, Some(7), Some(0))]);
    // Past the end, nothing.
    let past = read_text(&map, true, Some(10_000), None);
    assert_eq!(
        past,
        ReadText {
            text: String::new(),
            headings: vec![],
            blocks: vec![],
            truncated: false,
            next: None,
            incomplete: None,
        }
    );
}

#[test]
fn a_read_counts_characters_not_bytes() {
    let map = TextMap::build("Café 𝄞 au lait.\n");
    let read = read_text(&map, true, Some(5), Some(3));
    assert_eq!(read.text, "𝄞 a");
    assert_eq!(read.next, Some(8));
    // Where a block starts, too: 𝄞 is one character.
    let read = read_text(&TextMap::build("# Café 𝄞\n\nNaïve.\n"), true, None, None);
    assert_eq!(read.text, "Café 𝄞 Naïve.");
    assert_eq!(read.blocks[1].start, 7);
}

#[test]
fn a_read_gives_eight_thousand_characters_by_default_and_sixteen_thousand_at_most() {
    assert_eq!((READ_DEFAULT, READ_MAX), (8_000, 16_000));
    let source = "word ".repeat(10_000);
    let map = TextMap::from_plain(&source);
    let read = read_text(&map, true, None, None);
    assert_eq!(read.text.chars().count(), READ_DEFAULT as usize);
    assert!(read.truncated);
    assert_eq!(read.next, Some(READ_DEFAULT));
    let read = read_text(&map, true, None, Some(READ_MAX + 1000));
    assert_eq!(read.text.chars().count(), READ_MAX as usize);
}

/// The whole answer, file and blocks and headings, is kept under 48 KiB: a page of many small
/// blocks ends sooner, and `next` goes on from where it did.
#[test]
fn a_read_of_many_tiny_blocks_under_a_long_heading_stays_under_its_size() {
    assert_eq!(READ_BYTES, 48 * 1024);
    let heading = "A long heading over every paragraph ".repeat(30);
    let source = format!("# {}\n\n{}", heading.trim(), "a\n\n".repeat(3_500));
    let map = TextMap::build(&source);
    let file = PathForms {
        windows: Some(format!(r"S:\Notes\My Vault\{}.md", "deep\\".repeat(200))),
        wsl: Some(format!("/mnt/s/Notes/My Vault/{}.md", "deep/".repeat(200))),
    };
    let mut from = None;
    let mut text = String::new();
    let mut pages = 0;
    loop {
        let answer = read_page(file.clone(), &map, true, None, from, None);
        let size = serde_json::to_vec(&answer).unwrap().len();
        assert!(size <= READ_BYTES, "page {pages}: {size} bytes");
        // The heading once, however many blocks are under it.
        assert!(
            answer.read.headings.len() <= 1,
            "{:?}",
            answer.read.headings.len()
        );
        pages += 1;
        text.push_str(&answer.read.text);
        if pages == 1 {
            // Cut sooner than the 8,000 characters asked for, by its size.
            assert!(answer.read.truncated);
            assert!(answer.read.text.chars().count() < READ_DEFAULT as usize);
        }
        match answer.read.next {
            Some(next) => {
                assert_eq!(
                    next as usize,
                    from.unwrap_or(0) as usize + answer.read.text.chars().count()
                );
                from = Some(next);
            }
            None => break,
        }
    }
    assert!(pages > 1);
    assert_eq!(text, map.text());
}

#[test]
fn a_page_reads_without_lines_its_blocks_under_their_headings() {
    let blocks = [
        PageBlock {
            text: "Report".to_owned(),
            heading_level: Some(1),
        },
        PageBlock {
            text: "Made  by a script.".to_owned(),
            heading_level: None,
        },
    ];
    let map = TextMap::from_blocks(&blocks, "");
    let read = read_text(&map, false, None, None);
    assert_eq!(read.text, "Report Made by a script.");
    assert_eq!(read.headings, ["Report"]);
    assert_eq!(
        read.blocks,
        [block(0, None, Some(0)), block(7, None, Some(0))]
    );
}

#[test]
fn read_answers_the_file_and_its_page_of_text() {
    let file = PathForms {
        windows: Some(r"S:\Notes\My Vault\plan.md".to_owned()),
        wsl: None,
    };
    let answer = read_page(
        file,
        &TextMap::build("# Plan\n\nText.\n"),
        true,
        None,
        None,
        Some(4),
    );
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({
            "file": {"windows": r"S:\Notes\My Vault\plan.md", "wsl": null},
            "text": "Plan",
            "headings": ["Plan"],
            "blocks": [{"start": 0, "line": 1, "heading": 0}],
            "truncated": true,
            "next": 4,
        })
    );
    let page = read_page(
        no_file(),
        &TextMap::from_blocks(&[], ""),
        false,
        None,
        None,
        None,
    );
    assert_eq!(
        serde_json::to_value(&page).unwrap(),
        json!({
            "file": {"windows": null, "wsl": null},
            "text": "",
            "headings": [],
            "blocks": [],
            "truncated": false,
        })
    );
}

/// A page whose bridge stopped its text at its limit reads as cut short: never as all of it, the
/// last page `truncated` all the same, with nowhere further to go, and every page saying why.
#[test]
fn a_page_cut_short_by_its_bridge_reads_as_incomplete() {
    assert_eq!(PAGE_TEXT_CUT, "page text over 2 MiB");
    let blocks = [
        PageBlock {
            text: "Report".to_owned(),
            heading_level: Some(1),
        },
        PageBlock {
            text: "Rows and rows.".to_owned(),
            heading_level: None,
        },
    ];
    let map = TextMap::from_blocks(&blocks, "");
    let whole = read_page(no_file(), &map, false, Some(PAGE_TEXT_CUT), None, None);
    assert_eq!(whole.read.text, "Report Rows and rows.");
    assert!(whole.read.truncated);
    assert_eq!(whole.read.next, None);
    assert_eq!(whole.read.incomplete.as_deref(), Some(PAGE_TEXT_CUT));
    let json = serde_json::to_value(&whole).unwrap();
    assert_eq!(json["incomplete"], "page text over 2 MiB");
    assert_eq!(json["truncated"], true);
    // A first page goes on to the next as any does.
    let first = read_page(no_file(), &map, false, Some(PAGE_TEXT_CUT), None, Some(6));
    assert_eq!((first.read.truncated, first.read.next), (true, Some(6)));
    assert_eq!(first.read.incomplete.as_deref(), Some(PAGE_TEXT_CUT));
    // A page read whole says nothing of it.
    let all = read_page(no_file(), &map, false, None, None, None);
    assert!(!all.read.truncated);
    assert!(serde_json::to_value(&all)
        .unwrap()
        .get("incomplete")
        .is_none());
}
