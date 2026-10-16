//! Targets and reading, in core: where a heading, a line or a quote is in a note or a text file
//! (in the text as shown, else, for a quote with Markdown marks, in the source), which targets each
//! kind of file takes, what a window's UI is sent and answers for them, and `read`'s pages of the
//! rendered text.

use lectern_core::agent::read::{
    read_page, ReadBlock, ReadText, PAGE_TEXT_CUT, READ_BYTES, READ_DEFAULT,
};
use lectern_core::agent::request::{HighlightArgs, Target};
use lectern_core::agent::status::{AgentCall, AgentReply};
use lectern_core::agent::target::{
    check_target, resolve_target, HighlightAnswer, ShowAt, TextPlace,
};
use lectern_core::agent::tools::READ_MAX;
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

/// The place of the text `found` in the note's text, at its `nth` occurrence (from 0), counted as
/// the UI counts it, in UTF-16 code units, on the lines given.
fn at(source: &str, found: &str, nth: usize, lines: (u32, u32)) -> TextPlace {
    let text = TextMap::build(source).text().to_owned();
    let start = text.match_indices(found).nth(nth).expect("in the text").0;
    let units = |bytes: usize| u32::try_from(text[..bytes].encode_utf16().count()).unwrap();
    TextPlace {
        start_line: lines.0,
        end_line: lines.1,
        text_start: Some(units(start)),
        text_end: Some(units(start + found.len())),
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
fn a_line_is_its_block_in_the_text() {
    assert_eq!(
        place(PLAN, &Target::Line { line: 7 }),
        Ok(TextPlace {
            start_line: 7,
            end_line: 7,
            ..at(PLAN, "Roll back the migration first.", 0, (7, 7))
        })
    );
    // A paragraph over two lines, from its second.
    let source = "First line\nsecond line.\n";
    assert_eq!(
        place(source, &Target::Line { line: 2 }),
        Ok(TextPlace {
            start_line: 2,
            end_line: 2,
            ..at(source, "First line second line.", 0, (2, 2))
        })
    );
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

#[test]
fn a_text_file_has_its_paragraphs_and_no_headings() {
    let source = "Notes\n\nRoll back first.\nThen call.\n";
    let text = TextMap::from_plain(source);
    assert_eq!(
        resolve_target(&text, source, "notes.txt", &quote("Then call", None, None))
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

// What a window's UI is sent and answers.

#[test]
fn a_window_is_sent_the_places_core_found_and_answers_how_many_highlights_it_shows() {
    let place = TextPlace {
        start_line: 7,
        end_line: 8,
        text_start: Some(24),
        text_end: Some(53),
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
        json!([{"startLine": 7, "endLine": 8, "textStart": 24, "textEnd": 53}])
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
