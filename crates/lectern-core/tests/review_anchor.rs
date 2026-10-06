//! The text map and re-anchoring: a comment follows its quote through edits to the note, and is
//! detached, never lost, when the quote is gone.

use lectern_core::review::{
    anchor::{self, AnchorState},
    fingerprint, format,
    ops::{self, NewAnchor, OpContext, ReviewOp},
    text::{normalize, TextMap},
    Item, Review,
};

const NOTE: &str = "---\ntitle: Tide sync\n---\n# Tide sync\n\nIntro line.\n\n## Batching\n\nThe client uploads readings in batches of at most 50 so a slow link stays responsive.\n\n- [ ] Reject readings older than a week\n- [x] Keep **bold** and `code` text[^1]\n\n```rust\nlet batch = 50;\n```\n\n| Station | Batch |\n|---|---|\n| North | 50 |\n\n## Retries\n\nRetry with jitter after a failed upload.\n\n[^1]: A footnote body.\n";

fn add(src: &str, quote: &str, start: u32, end: u32) -> Review {
    add_after(src, "", quote, start, end)
}

/// A review with one comment on `quote`, selected right after the visible text `prefix`.
fn add_after(src: &str, prefix: &str, quote: &str, start: u32, end: u32) -> Review {
    let tm = TextMap::build(src);
    let fp = fingerprint(src);
    let mut r = format::new_review("tide.md");
    ops::apply(
        &mut r,
        &ReviewOp::Add {
            anchor: NewAnchor {
                start_line: start,
                end_line: end,
                quote: quote.into(),
                prefix: prefix.into(),
            },
            text: "why?".into(),
        },
        &OpContext {
            text: &tm,
            fingerprint: &fp,
            now: "2026-10-06T10:00:00Z",
        },
    )
    .unwrap();
    r
}

fn state(r: &Review, src: &str) -> anchor::Resolved {
    anchor::resolve_all(r, &TextMap::build(src), &fingerprint(src)).remove(0)
}

#[test]
fn text_map_matches_the_visible_text_rules() {
    let tm = TextMap::build(NOTE);
    let t = tm.text();
    assert!(t.starts_with("Tide sync Intro line. Batching The client uploads"));
    assert!(
        t.contains("Keep bold and code text"),
        "inline markup stripped, footnote ref skipped: {t}"
    );
    assert!(t.contains("let batch = 50;"), "code block content kept");
    assert!(t.contains("North 50"), "table cells joined by a space");
    assert!(!t.contains("title:"), "frontmatter is not text");
    assert_eq!(tm.heading_path_at(10), ["Tide sync", "Batching"]);
    let b = tm
        .blocks()
        .iter()
        .find(|b| tm.block_text(b).starts_with("The client"))
        .unwrap();
    assert_eq!(
        (b.start_line, b.end_line),
        (10, 10),
        "lines count from the top of the file, frontmatter included"
    );
    assert_eq!(normalize("  a \n\t b  "), "a b");
}

#[test]
fn unchanged_note_keeps_the_stored_lines() {
    let r = add(NOTE, "batches of at most 50", 10, 10);
    let c = r.comments().next().unwrap();
    assert_eq!(c.heading_path, ["Tide sync", "Batching"]);
    let prefix = &c.anchor.as_ref().unwrap().prefix;
    assert_eq!(
        prefix, " The client uploads readings in ",
        "32 chars of visible text before the quote"
    );
    assert_eq!(state(&r, NOTE).state, AnchorState::Anchored);
}

#[test]
fn a_moved_block_is_followed() {
    let r = add(NOTE, "batches of at most 50", 10, 10);
    let shifted = NOTE.replace(
        "Intro line.\n",
        "Intro line.\n\nA new paragraph.\n\nAnother.\n",
    );
    let s = state(&r, &shifted);
    assert_eq!((s.state, s.start_line), (AnchorState::Anchored, 14));
}

#[test]
fn a_reworded_passage_is_moved_with_its_current_text() {
    let r = add(
        NOTE,
        "The client uploads readings in batches of at most 50 so a slow link stays responsive.",
        10,
        10,
    );
    let edited = NOTE.replace("batches of at most 50", "batches of at most 80");
    let s = state(&r, &edited);
    assert_eq!(s.state, AnchorState::Moved);
    assert!(s.current_text.unwrap().contains("at most 80"));
}

#[test]
fn a_deleted_passage_is_detached_and_pinned_to_its_heading() {
    let r = add(
        NOTE,
        "batches of at most 50 so a slow link stays responsive",
        10,
        10,
    );
    let edited = NOTE.replace(
        "The client uploads readings in batches of at most 50 so a slow link stays responsive.\n",
        "",
    );
    let s = state(&r, &edited);
    assert_eq!(s.state, AnchorState::Detached);
    assert_eq!(s.pinned_heading.unwrap().0, "Batching");
}

#[test]
fn a_whole_document_rewrite_detaches_every_comment_and_loses_none() {
    let mut r = add(NOTE, "batches of at most 50", 10, 10);
    let tm = TextMap::build(NOTE);
    let fp = fingerprint(NOTE);
    let ctx = OpContext {
        text: &tm,
        fingerprint: &fp,
        now: "2026-10-06T10:01:00Z",
    };
    ops::apply(
        &mut r,
        &ReviewOp::Add {
            anchor: NewAnchor {
                start_line: 25,
                end_line: 25,
                quote: "Retry with jitter".into(),
                prefix: String::new(),
            },
            text: "formula?".into(),
        },
        &ctx,
    )
    .unwrap();
    let rewritten = "# Something else\n\nEntirely different words here about kelp farming.\n";
    let all = anchor::resolve_all(&r, &TextMap::build(rewritten), &fingerprint(rewritten));
    assert_eq!(all.len(), 2);
    assert!(all
        .iter()
        .all(|s| s.state == AnchorState::Detached && s.pinned_heading.is_none()));
    let back = format::parse(&format::serialize(&r));
    assert_eq!(back.comments().count(), 2);
}

#[test]
fn duplicate_quotes_are_resolved_by_context() {
    let src = "# A\n\nFirst: check the gauge before you start.\n\nSecond: check the gauge after you finish.\n";
    let r = add(src, "check the gauge", 5, 5);
    assert_eq!(
        r.comments().next().unwrap().start_line,
        5,
        "the occurrence inside the given lines wins"
    );
    let shifted = src.replace("# A\n", "# A\n\nPreface.\n");
    assert_eq!(state(&r, &shifted).start_line, 7);
}

#[test]
fn a_phrase_repeated_in_one_block_anchors_to_the_occurrence_selected() {
    let src = "# A\n\nBefore: check the gauge now. After: check the gauge later.\n";
    let quote = "check the gauge";
    // The UI sends the 32 characters of visible text before the selection.
    let r = add_after(src, "re: check the gauge now. After: ", quote, 3, 3);
    let c = r.comments().next().unwrap();
    let meta = c.anchor.as_ref().unwrap();
    assert_eq!(meta.prefix, "re: check the gauge now. After: ");
    assert_eq!(meta.suffix, " later.", "the second occurrence's context");

    // An edit above it: still the second occurrence.
    let shifted = src.replace("# A\n", "# A\n\nPreface.\n");
    let second = TextMap::build(&shifted).text().rfind(quote).unwrap();
    let s = state(&r, &shifted);
    assert_eq!(s.state, AnchorState::Anchored);
    assert_eq!(s.span, Some((second, second + quote.len())));

    // The first, selected after its own text, or with no text before it given, stays the first.
    for prefix in ["A Before: ", ""] {
        let r = add_after(src, prefix, quote, 3, 3);
        let meta = r.comments().next().unwrap().anchor.clone().unwrap();
        assert_eq!(
            meta.suffix, " now. After: check the gauge lat",
            "{prefix:?}"
        );
    }
}

#[test]
fn a_capped_quote_still_anchors_exactly() {
    let long = "word ".repeat(200);
    let src = format!("# A\n\n{long}\n");
    let r = add(&src, &long, 3, 3);
    let q = &r.comments().next().unwrap().quote;
    assert!(q.ends_with('…') && q.chars().count() == 501);
    let shifted = src.replace("# A\n", "# A\n\nPreface.\n");
    assert_eq!(state(&r, &shifted).state, AnchorState::Anchored);
}

#[test]
fn add_with_stale_lines_finds_the_quote_or_leaves_fp_empty() {
    let r = add(NOTE, "Retry with jitter", 2, 2); // stale lines, quote elsewhere
    let c = r.comments().next().unwrap();
    assert_eq!(c.start_line, 25);
    let r = add(NOTE, "text the note does not contain", 10, 10);
    assert_eq!(r.comments().next().unwrap().anchor.as_ref().unwrap().fp, "");
    assert_eq!(
        state(&r, NOTE).state,
        AnchorState::Detached,
        "never trusted just because lines were given"
    );
}

#[test]
fn refresh_rewrites_only_anchored_comments_that_moved() {
    let mut r = add(NOTE, "batches of at most 50", 10, 10);
    let shifted = NOTE.replace("Intro line.\n", "Intro line.\n\nNew.\n");
    let tm = TextMap::build(&shifted);
    let fp = fingerprint(&shifted);
    for item in &mut r.items {
        if let Item::Comment(c) = item {
            c.dirty = false;
            c.raw = Some(String::new());
        }
    }
    let res = anchor::resolve_all(&r, &tm, &fp);
    anchor::refresh_anchored(&mut r, &res, &tm, &fp);
    let c = r.comments().next().unwrap();
    assert!(c.dirty);
    assert_eq!(c.start_line, 12);
    assert_eq!(c.anchor.as_ref().unwrap().fp, fp);
}

#[test]
fn text_map_leaves_out_images_raw_html_and_alert_titles() {
    let src = "# Notes\n\nSee ![a chart](c.png) and <kbd>Ctrl</kbd> keys, [[Other note|the other note]].\n\n<div>raw block</div>\n\n> [!NOTE]\n> Alert body.\n";
    let tm = TextMap::build(src);
    assert_eq!(
        tm.text(),
        "Notes See and Ctrl keys, the other note. Alert body."
    );
    let lines: Vec<_> = tm
        .blocks()
        .iter()
        .map(|b| (b.start_line, b.end_line))
        .collect();
    assert_eq!(lines, [(1, 1), (3, 3), (8, 8)]);
}

#[test]
fn heading_paths_follow_the_heading_levels() {
    let src = "# A\n## B\n### C\n## D\ntext\n#\nmore\n# E\n";
    let tm = TextMap::build(src);
    assert_eq!(tm.heading_path_at(3), ["A", "B", "C"]);
    assert_eq!(
        tm.heading_path_at(5),
        ["A", "D"],
        "an h2 closes the h2 and h3 before it"
    );
    assert_eq!(
        tm.heading_path_at(7),
        Vec::<String>::new(),
        "an empty h1 has no text"
    );
    assert_eq!(tm.heading_path_at(8), ["E"]);
    let text_block = &tm.blocks()[4];
    assert_eq!(tm.block_text(text_block), "text");
    assert_eq!(text_block.heading, Some(3), "the last heading before it, D");
    assert_eq!(
        tm.blocks()[3].heading,
        Some(3),
        "a heading points at itself"
    );
}

#[test]
fn a_separator_space_belongs_to_the_next_block() {
    let tm = TextMap::build("First.\n\nSecond.\n");
    assert_eq!(tm.text(), "First. Second.");
    assert_eq!(tm.lines_of(0, 6), (1, 1));
    assert_eq!(
        tm.lines_of(6, 14),
        (3, 3),
        "the space is the start of Second."
    );
    assert_eq!(tm.lines_of(0, 7), (1, 3));
    assert_eq!(tm.before(7, 32), "First. ");
    assert_eq!(tm.after(7, 3), "Sec");
}

#[test]
fn a_detached_comment_falls_back_to_the_deepest_surviving_heading() {
    let r = add(
        NOTE,
        "batches of at most 50 so a slow link stays responsive",
        10,
        10,
    );
    let edited = NOTE.replace(
        "## Batching\n\nThe client uploads readings in batches of at most 50 so a slow link stays responsive.\n",
        "",
    );
    let s = state(&r, &edited);
    assert_eq!(s.state, AnchorState::Detached);
    assert_eq!(
        (s.start_line, s.end_line),
        (10, 10),
        "the stored lines are kept"
    );
    assert_eq!(s.pinned_heading, Some(("Tide sync".to_owned(), 4)));
}

#[test]
fn equal_matches_go_to_the_one_nearest_the_stored_line() {
    // No anchor line, so no context to tell the two apart.
    let r = format::parse("## C1 · open · L5\n> check the gauge\n\n**You:** x\n");
    let src = "Check: check the gauge.\n\nCheck: check the gauge.\n\nCheck: check the gauge.\n";
    assert_eq!(state(&r, src).start_line, 5);

    // Reworded twice the same way: the passage nearest the stored line wins.
    let src =
        "one two three four six\n\nfiller words here\n\nmore filler\n\none two three four six\n";
    for line in [1, 7] {
        let r = format::parse(&format!(
            "## C1 · open · L{line}\n> one two three four five\n\n**You:** x\n"
        ));
        let s = state(&r, src);
        assert_eq!((s.state, s.start_line), (AnchorState::Moved, line));
        assert_eq!(s.current_text.as_deref(), Some("one two three four six"));
    }
}

#[test]
fn refresh_leaves_moved_detached_and_unchanged_comments_alone() {
    let mut r = add(NOTE, "batches of at most 50", 10, 10);
    let tm = TextMap::build(NOTE);
    let fp = fingerprint(NOTE);
    let ctx = OpContext {
        text: &tm,
        fingerprint: &fp,
        now: "2026-10-06T10:01:00Z",
    };
    for (line, quote) in [
        (12, "Reject readings older than a week"),
        (25, "Retry with jitter after a failed upload"),
    ] {
        let anchor = NewAnchor {
            start_line: line,
            end_line: line,
            quote: quote.into(),
            prefix: String::new(),
        };
        let op = ReviewOp::Add {
            anchor,
            text: "x".into(),
        };
        ops::apply(&mut r, &op, &ctx).unwrap();
    }
    let clean = |r: &mut Review| {
        for item in &mut r.items {
            if let Item::Comment(c) = item {
                c.dirty = false;
            }
        }
    };
    clean(&mut r);
    // The first quote stays put, the second is reworded, the third is deleted.
    let edited = NOTE
        .replace("older than a week", "older than a fortnight")
        .replace("Retry with jitter after a failed upload.\n", "");
    let tm = TextMap::build(&edited);
    let fp = fingerprint(&edited);
    let res = anchor::resolve_all(&r, &tm, &fp);
    let states: Vec<_> = res.iter().map(|s| s.state).collect();
    assert_eq!(
        states,
        [
            AnchorState::Anchored,
            AnchorState::Moved,
            AnchorState::Detached
        ]
    );
    let before = r.clone();
    anchor::refresh_anchored(&mut r, &res, &tm, &fp);
    let first = r.comments().next().unwrap();
    assert!(first.dirty, "the new fingerprint is stored");
    assert_eq!(first.anchor.as_ref().unwrap().fp, fp);
    assert_eq!(first.start_line, 10);
    assert_eq!(
        r.comments().skip(1).collect::<Vec<_>>(),
        before.comments().skip(1).collect::<Vec<_>>(),
        "moved and detached comments keep their stored anchor"
    );

    // Saved again with nothing changed: the first is trusted by its fingerprint and left alone.
    clean(&mut r);
    let res = anchor::resolve_all(&r, &tm, &fp);
    assert_eq!(res[0].span, None);
    anchor::refresh_anchored(&mut r, &res, &tm, &fp);
    assert!(r.comments().all(|c| !c.dirty));
}

#[test]
fn words_match_whatever_their_case() {
    // A final sigma lowercases the same way in the quote and in the note, whichever is in capitals.
    let upper = "ΟΔΟΣ ΤΗΣ ΘΑΛΑΣΣΑΣ ΚΑΙ ΤΟΥ ΛΙΜΕΝΟΣ";
    let lower = "Οδος της θαλασσας και του λιμενος";
    for (quote, note) in [(upper, lower), (lower, upper)] {
        let r = format::parse(&format!("## C1 · open · L1\n> {quote}\n\n**You:** x\n"));
        let s = state(&r, &format!("{note}\n"));
        assert_eq!(s.state, AnchorState::Moved, "{quote} in {note}");
        assert_eq!(s.current_text.as_deref(), Some(note));
    }
}

#[test]
fn raw_html_text_follows_the_html_policy() {
    // Tags off the allowlist are shown as the text they are, inline or as a block, so they count.
    // An allowed raw block stays HTML: its own markup text is left out, the Markdown inside it isn't.
    let src = "# Policy\n\nRun <script>alert(1)</script> now, in a <tone>calm</tone> voice.\n\n<script>alert(2)</script>\n\n<details>\n<summary>More</summary>\n\nBody text.\n\n</details>\n";
    let tm = TextMap::build(src);
    assert_eq!(
        tm.text(),
        "Policy Run <script>alert(1)</script> now, in a <tone>calm</tone> voice. <script>alert(2)</script> Body text."
    );
    assert!(!tm.text().contains("More"));
    let lines: Vec<_> = tm
        .blocks()
        .iter()
        .map(|b| (b.start_line, b.end_line))
        .collect();
    assert_eq!(lines, [(1, 1), (3, 3), (5, 5), (10, 10)]);
}

#[test]
fn overlapping_occurrences_are_all_candidates() {
    let src = "intro\n\na a suffix\n";
    let r = add(src, "a a", 3, 3);
    let a = r.comments().next().unwrap().anchor.clone().unwrap();
    assert_eq!(
        (a.prefix.as_str(), a.suffix.as_str()),
        ("intro ", " suffix")
    );

    // "a a a suffix": the match at 0 hides the one at 2, which has the stored context.
    let changed = src.replace("intro", "a");
    let s = state(&r, &changed);
    assert_eq!(
        (s.state, s.start_line, s.end_line),
        (AnchorState::Anchored, 3, 3)
    );

    // Anchoring on the changed note picks the occurrence inside the given line too.
    let r = add(&changed, "a a", 3, 3);
    let c = r.comments().next().unwrap();
    assert_eq!((c.start_line, c.end_line), (3, 3));
    let a = c.anchor.as_ref().unwrap();
    assert_eq!((a.prefix.as_str(), a.suffix.as_str()), ("a ", " suffix"));
}

/// A footnote definition holding a heading, before a later body heading. comrak moves footnotes
/// to the end, so the AST meets the headings out of source order.
const FOOTNOTED: &str = "# Guide\n\nIntro with a note[^n].\n\n[^n]: The note.\n\n    ### Aside\n\n    alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango\n\n## Setup\n\nInstall the pump first.\n";

#[test]
fn a_heading_in_a_footnote_keeps_heading_paths_in_source_order() {
    let tm = TextMap::build(FOOTNOTED);
    let lines: Vec<u32> = tm.headings().iter().map(|h| h.line).collect();
    assert_eq!(lines, [1, 7, 11], "headings in source order");
    assert!(
        tm.text().ends_with("Aside alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango"),
        "the text keeps the page order, footnotes last: {}",
        tm.text()
    );
    assert_eq!(tm.heading_path_at(13), ["Guide", "Setup"]);
    assert_eq!(tm.heading_path_at(9), ["Guide", "Aside"]);
    let heading_of = |start: &str| {
        let b = tm
            .blocks()
            .iter()
            .find(|b| tm.block_text(b).starts_with(start))
            .unwrap();
        tm.headings()[b.heading.unwrap()].text.clone()
    };
    assert_eq!(heading_of("Install"), "Setup");
    assert_eq!(heading_of("alpha"), "Aside");
    assert_eq!(heading_of("The note"), "Guide");

    let r = add(FOOTNOTED, "Install the pump first.", 13, 13);
    assert_eq!(
        r.comments().next().unwrap().heading_path,
        ["Guide", "Setup"]
    );

    let gone = format::parse("## C1 · open · L13 · Guide › Setup\n> no such words\n\n**You:** x\n");
    assert_eq!(
        state(&gone, FOOTNOTED).pinned_heading,
        Some(("Setup".to_owned(), 11))
    );
}

#[test]
fn the_heading_bonus_follows_source_order() {
    // 11 of the 20 words survive: 0.55, which reaches 0.6 only with the bonus for being under
    // the comment's heading path, here the heading inside the footnote.
    let quote = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo one two three four five six seven eight nine";
    let r = format::parse(&format!(
        "## C1 · open · L9 · Guide › Aside\n> {quote}\n\n**You:** x\n"
    ));
    let s = state(&r, FOOTNOTED);
    assert_eq!((s.state, s.start_line), (AnchorState::Moved, 9));
    assert!(s.current_text.unwrap().ends_with("sierra tango"));

    // Under no heading of the note, the same words fall short.
    let r = format::parse(&format!(
        "## C1 · open · L9 · Guide › Elsewhere\n> {quote}\n\n**You:** x\n"
    ));
    assert_eq!(state(&r, FOOTNOTED).state, AnchorState::Detached);
}

/// A note whose first paragraph alone is longer than the quote cap.
fn long_note() -> (String, String) {
    let long: Vec<String> = (0..60).map(|i| format!("reading{i}")).collect();
    let long = long.join(" ");
    let src = format!("# A\n\n{long}\n\nSecond paragraph.\n\nThird.\n");
    (src, long)
}

#[test]
fn a_capped_selection_keeps_its_whole_line_range() {
    let (src, long) = long_note();
    assert!(long.chars().count() > 500);
    let selection = format!("{long}\nSecond paragraph.");
    let mut r = add(&src, &selection, 3, 5);
    let c = r.comments().next().unwrap();
    assert!(c.quote.ends_with('…'));
    assert_eq!(
        (c.start_line, c.end_line),
        (3, 5),
        "the selection's lines, not the excerpt's"
    );
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(a.prefix, "A ");
    assert_eq!(a.suffix, " Third.", "after the end of the whole selection");

    // Two lines added above: found again, and the save keeps the range's length.
    let shifted = src.replace("# A\n", "# A\n\nNew.\n");
    let tm = TextMap::build(&shifted);
    let fp = fingerprint(&shifted);
    let res = anchor::resolve_all(&r, &tm, &fp);
    assert_eq!(
        (res[0].state, res[0].start_line, res[0].end_line),
        (AnchorState::Anchored, 5, 7)
    );
    anchor::refresh_anchored(&mut r, &res, &tm, &fp);
    let c = r.comments().next().unwrap();
    assert_eq!((c.start_line, c.end_line), (5, 7));
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(a.fp, fp);
    assert_eq!(a.prefix, "A New. ");
    assert_eq!(
        a.suffix, " Third.",
        "still the text after the whole selection"
    );

    // A selection core can't find whole still anchors on its excerpt.
    let r = add(&src, &format!("{long} Something else"), 3, 5);
    let c = r.comments().next().unwrap();
    assert_eq!((c.start_line, c.end_line), (3, 3));
    assert_eq!(c.anchor.as_ref().unwrap().fp, fingerprint(&src));
}

#[test]
fn a_capped_quote_is_told_apart_by_its_prefix_alone() {
    // The stored suffix follows the whole passage, so it says nothing about what follows the
    // excerpt looked for: that is more of the passage. A shorter draft of the section put in
    // above, sharing the excerpt and the paragraph after it, must not draw the comment away.
    let (src, long) = long_note();
    let r = add(&src, &long, 3, 3);
    let c = r.comments().next().unwrap();
    assert!(c.quote.ends_with('…'));
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(
        (a.prefix.as_str(), a.suffix.as_str()),
        ("A ", " Second paragraph. Third.")
    );

    let excerpt = anchor::match_part(&c.quote).trim_end();
    let edited = format!("# Draft\n\n{excerpt}\n\nSecond paragraph.\n\n{src}");
    let s = state(&r, &edited);
    assert_eq!(
        (s.state, s.start_line, s.end_line),
        (AnchorState::Anchored, 9, 9),
        "the original passage, told apart by its prefix"
    );
}

#[test]
fn lines_of_covers_every_block_the_span_touches() {
    // The footnote is shown last but sits on line 3.
    let src = "Intro[^n].\n\n[^n]: The note.\n\nBody paragraph.\n";
    let tm = TextMap::build(src);
    assert_eq!(tm.text(), "Intro. Body paragraph. The note.");
    let body = tm.text().find("Body").unwrap();
    assert_eq!(tm.lines_of(body, tm.text().len()), (3, 5));
    assert_eq!(tm.lines_of(0, tm.text().len()), (1, 5));
}

#[test]
fn a_long_selection_of_repeated_text_anchors_without_a_quadratic_search() {
    // A 64,000-character selection inside a run of 128,000 of the same character matches at
    // every offset of the first half. Visiting those overlapping occurrences costs the
    // selection's length each, so a selection longer than the quote cap is matched end to end.
    let run = "a".repeat(128_000);
    let src = format!("# Log\n\n```text\n{run}\n```\n\nAfter.\n");
    let started = std::time::Instant::now();
    let r = add(&src, &run[..64_000], 3, 5);
    let took = started.elapsed();
    let c = r.comments().next().unwrap();
    assert_eq!((c.start_line, c.end_line), (3, 5), "the code block's lines");
    assert_eq!(c.quote.chars().count(), 501);
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(a.fp, fingerprint(&src), "found, not the fallback");
    assert_eq!(a.prefix, "Log ", "the first occurrence");
    assert_eq!(a.suffix, "a".repeat(32));
    // A generous ceiling: the overlapping search took seconds in a release build.
    assert!(
        took < std::time::Duration::from_secs(2),
        "anchoring took {took:?}"
    );
}
