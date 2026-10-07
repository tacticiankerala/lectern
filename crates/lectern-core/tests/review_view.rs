//! The review payload the UI shows: comments in file order with their place in the note now,
//! sections that couldn't be read, and each entry rendered to sanitised HTML.

use std::path::Path;

use lectern_core::library::pathmap::PathMapper;
use lectern_core::render::{render, RenderContext};
use lectern_core::review::{
    anchor::{self, AnchorState},
    fingerprint, format,
    ops::{self, NewAnchor, OpContext, ReviewOp, StatusChange},
    text::TextMap,
    view::{build_payload, PayloadInput, ReviewPayload},
    ClaudeKind, CommentStatus, Entry, EntryAuthor, Item, Review,
};

const NOTE: &str = "# Tide sync\n\n## Batching\n\nThe client uploads readings in batches of at most 50.\n\n## Retries\n\nRetry with jitter after a failed upload.\n";
const NOTE_PATH: &str = "/home/me/notes/tide.md";
const SIDECAR_PATH: &str = "/home/me/notes/tide.review.md";

fn apply(r: &mut Review, op: ReviewOp) -> u32 {
    let tm = TextMap::build(NOTE);
    let fp = fingerprint(NOTE);
    let ctx = OpContext {
        text: &tm,
        fingerprint: &fp,
        now: "2026-10-06T10:00:00Z",
    };
    ops::apply(r, &op, &ctx).unwrap()
}

fn add(r: &mut Review, line: u32, quote: &str, text: &str) -> u32 {
    let anchor = NewAnchor {
        start_line: line,
        end_line: line,
        quote: quote.into(),
        prefix: String::new(),
    };
    apply(
        r,
        ReviewOp::Add {
            anchor,
            text: text.into(),
        },
    )
}

/// Stands in for the note's render pipeline, so the tests can see which text went through it.
fn plain(md: &str) -> String {
    format!("<p>{md}</p>")
}

/// The payload for `review` against the note `source`, resolved as the app resolves it. With
/// `with_text` false the note's text map isn't passed.
fn payload_for(review: Option<&Review>, source: &str, with_text: bool) -> ReviewPayload {
    let tm = TextMap::build(source);
    let resolved = review
        .map(|r| anchor::resolve_all(r, &tm, &fingerprint(source)))
        .unwrap_or_default();
    let input = PayloadInput {
        note: Path::new(NOTE_PATH),
        sidecar: Path::new(SIDECAR_PATH),
        review,
        resolved: &resolved,
        text: with_text.then_some(&tm),
        read_only: None,
        note_wsl: None,
        sidecar_wsl: None,
    };
    build_payload(input, &plain)
}

#[test]
fn payload_orders_and_counts() {
    let mut r = format::new_review("tide.md");
    add(&mut r, 5, "batches of at most 50", "Why 50?");
    add(&mut r, 9, "Retry with jitter", "Which formula?");
    add(&mut r, 5, "The client uploads readings", "From where?");
    apply(
        &mut r,
        ReviewOp::SetStatus {
            id: 2,
            change: StatusChange::Resolve,
        },
    );
    // Claude asked a question on C3 since Lectern last wrote it.
    let Some(Item::Comment(c3)) = r.items.get_mut(2) else {
        panic!("C3 is the third item")
    };
    c3.entries.push(Entry {
        author: EntryAuthor::Agent,
        name: "Claude".into(),
        kind: Some(ClaudeKind::Question),
        text: "The **station** or the server?".into(),
    });
    r.items.swap(0, 1);
    r.items
        .push(Item::Raw("## Summary\n\nOne resolved.\n".into()));

    let tm = TextMap::build(NOTE);
    let resolved = anchor::resolve_all(&r, &tm, &fingerprint(NOTE));
    let p = build_payload(
        PayloadInput {
            note: Path::new(NOTE_PATH),
            sidecar: Path::new(SIDECAR_PATH),
            review: Some(&r),
            resolved: &resolved,
            text: Some(&tm),
            read_only: Some("Shown read-only.".into()),
            note_wsl: Some("/home/me/notes/tide.md".into()),
            sidecar_wsl: Some("/home/me/notes/tide.review.md".into()),
        },
        &plain,
    );

    assert!(p.exists);
    assert_eq!(p.note_path, NOTE_PATH);
    assert_eq!(p.sidecar_path, SIDECAR_PATH);
    assert_eq!(p.note_wsl_path.as_deref(), Some("/home/me/notes/tide.md"));
    assert_eq!(
        p.sidecar_wsl_path.as_deref(),
        Some("/home/me/notes/tide.review.md")
    );
    assert_eq!(p.read_only.as_deref(), Some("Shown read-only."));
    let ids: Vec<u32> = p.comments.iter().map(|c| c.id).collect();
    assert_eq!(ids, [2, 1, 3], "file order, not id order");
    let statuses: Vec<CommentStatus> = p.comments.iter().map(|c| c.status).collect();
    assert_eq!(
        statuses,
        [
            CommentStatus::Resolved,
            CommentStatus::Open,
            CommentStatus::Question
        ],
        "the effective status"
    );
    assert_eq!(p.open_count, 2);
    assert!(p.unreadable.is_empty(), "a summary isn't a comment");

    let c1 = &p.comments[1];
    assert_eq!(c1.state, AnchorState::Anchored);
    assert_eq!((c1.start_line, c1.end_line), (5, 5));
    assert_eq!(c1.jump_line, Some(5));
    assert_eq!(c1.heading_path, ["Tide sync", "Batching"]);
    assert_eq!(c1.pinned_heading, None);
    assert_eq!(c1.quote, "batches of at most 50");
    // The note is as it was: found at its stored place, then looked for there for the UI.
    assert_eq!((c1.text_start, c1.text_end), (Some(50), Some(71)));
    assert_eq!(c1.current_text, None);
    assert_eq!(c1.entries.len(), 1);
    assert_eq!(c1.entries[0].author, EntryAuthor::You);
    assert_eq!(c1.entries[0].name, "You");
    assert_eq!(c1.entries[0].kind, None);
    assert_eq!(c1.entries[0].text, "Why 50?");
    assert_eq!(c1.entries[0].html, "<p>Why 50?</p>");
    let claude = &p.comments[2].entries[1];
    assert_eq!(claude.author, EntryAuthor::Agent);
    assert_eq!(claude.name, "Claude");
    assert_eq!(claude.kind, Some(ClaudeKind::Question));
    assert_eq!(claude.html, "<p>The **station** or the server?</p>");

    let none = payload_for(None, NOTE, false);
    assert!(!none.exists);
    assert!(none.comments.is_empty() && none.unreadable.is_empty());
    assert_eq!(none.open_count, 0);
}

#[test]
fn anchored_and_moved_comments_take_their_place_from_the_note_now() {
    let mut r = format::new_review("tide.md");
    add(&mut r, 5, "batches of at most 50", "Why 50?");
    let moved_down = NOTE.replace(
        "The client uploads readings in batches of at most 50.\n\n## Retries\n\n",
        "Nothing here yet.\n\n## Retries\n\nThe client uploads readings in batches of at most 50.\n\n",
    );

    let p = payload_for(Some(&r), &moved_down, true);
    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Anchored);
    assert_eq!((c.start_line, c.end_line), (9, 9));
    assert_eq!(c.jump_line, Some(9));
    assert_eq!(c.heading_path, ["Tide sync", "Retries"], "from the note");

    let p = payload_for(Some(&r), &moved_down, false);
    let c = &p.comments[0];
    assert_eq!((c.start_line, c.jump_line), (9, Some(9)));
    assert_eq!(
        c.heading_path,
        ["Tide sync", "Batching"],
        "stored, without the note's text"
    );

    let reworded = NOTE.replace("at most 50", "at most 64");
    let p = payload_for(Some(&r), &reworded, true);
    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Moved);
    assert_eq!(c.jump_line, Some(c.start_line));
    let current = c.current_text.as_deref().unwrap();
    let (start, end) = (c.text_start.unwrap(), c.text_end.unwrap());
    let text = TextMap::build(&reworded).text().to_owned();
    assert_eq!(
        &text[start as usize..end as usize],
        current,
        "the passage it became"
    );
    assert_eq!(c.quote, "batches of at most 50", "the quote is kept");
}

#[test]
fn detached_comment_carries_stored_place_and_pinned_heading() {
    let mut r = format::new_review("tide.md");
    add(&mut r, 5, "batches of at most 50", "Why 50?");
    let deleted = NOTE.replace(
        "The client uploads readings in batches of at most 50.\n\n",
        "",
    );

    let p = payload_for(Some(&r), &deleted, true);

    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Detached);
    assert_eq!((c.start_line, c.end_line), (5, 5), "the stored lines");
    assert_eq!(c.heading_path, ["Tide sync", "Batching"], "the stored path");
    assert_eq!(c.pinned_heading.as_deref(), Some("Batching"));
    assert_eq!(c.jump_line, Some(3), "the pinned heading's line");
    assert_eq!(c.quote, "batches of at most 50");
    assert_eq!(c.current_text, None);
    assert_eq!(p.open_count, 1);
}

#[test]
fn detached_comment_with_no_surviving_heading_jumps_to_the_top() {
    let mut r = format::new_review("tide.md");
    add(&mut r, 5, "batches of at most 50", "Why 50?");
    let rewritten = "Everything here was written again from scratch.\n";

    let p = payload_for(Some(&r), rewritten, true);

    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Detached);
    assert_eq!(c.pinned_heading, None);
    assert_eq!(c.jump_line, Some(1), "the document top");
    assert_eq!((c.text_start, c.text_end), (None, None));
}

#[test]
fn the_top_a_detached_comment_jumps_to_is_the_first_line_with_text() {
    let mut r = format::new_review("tide.md");
    add(&mut r, 5, "batches of at most 50", "Why 50?");
    // Rewritten with frontmatter and blank lines first: line 1 holds no block to go to.
    let rewritten = "---\ntitle: Tide\n---\n\n\nEverything here was written again.\n";

    let p = payload_for(Some(&r), rewritten, true);

    let c = &p.comments[0];
    assert_eq!(
        (c.state, c.pinned_heading.as_deref()),
        (AnchorState::Detached, None)
    );
    assert_eq!(c.jump_line, Some(6), "the first block's line");

    // A note with no text at all: line 1.
    let p = payload_for(Some(&r), "---\ntitle: Tide\n---\n", true);
    assert_eq!(p.comments[0].jump_line, Some(1));
}

/// A review of `src` with a comment on each of `quotes` on line `line`, selected after `prefix`.
fn added(src: &str, prefix: &str, quotes: &[&str], line: u32) -> Review {
    let tm = TextMap::build(src);
    let fp = fingerprint(src);
    let ctx = OpContext {
        text: &tm,
        fingerprint: &fp,
        now: "2026-10-06T10:00:00Z",
    };
    let mut r = format::new_review("tide.md");
    for quote in quotes {
        let anchor = NewAnchor {
            start_line: line,
            end_line: line,
            quote: (*quote).into(),
            prefix: prefix.into(),
        };
        let op = ReviewOp::Add {
            anchor,
            text: "Why?".into(),
        };
        ops::apply(&mut r, &op, &ctx).unwrap();
    }
    r
}

#[test]
fn text_offsets_count_utf16_where_it_was_and_after_an_edit() {
    let src = "# Café ☕\n\nThe naïve 🚀 client sends readings in batches of at most 50.\n";
    let r = added(src, "", &["batches of at most 50"], 3);

    // Unchanged: found at its stored place. "batches" is 51 bytes in, but 45 UTF-16 units.
    let p = payload_for(Some(&r), src, true);
    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Anchored);
    assert_eq!((c.text_start, c.text_end), (Some(45), Some(66)));

    // Edited above it: looked for, and found 9 units further on.
    let shifted = src.replace("☕\n", "☕\n\nPreface.\n");
    let p = payload_for(Some(&r), &shifted, true);
    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Anchored);
    assert_eq!((c.text_start, c.text_end), (Some(54), Some(75)));

    // Without the note's text, there's nothing to count in.
    let p = payload_for(Some(&r), src, false);
    assert_eq!(
        (p.comments[0].text_start, p.comments[0].text_end),
        (None, None)
    );
}

#[test]
fn an_unchanged_note_gives_the_occurrence_each_comment_was_made_on() {
    let src = "# A\n\ncheck the gauge now. check the gauge later.\n";
    let quote = "check the gauge";
    // As the UI sends them: the text before each selection.
    let first = added(src, "A ", &[quote], 3);
    let second = added(src, "A check the gauge now. ", &[quote], 3);

    let p = payload_for(Some(&first), src, true);
    assert_eq!(
        (p.comments[0].text_start, p.comments[0].text_end),
        (Some(2), Some(17))
    );
    let p = payload_for(Some(&second), src, true);
    assert_eq!(
        (p.comments[0].text_start, p.comments[0].text_end),
        (Some(23), Some(38))
    );
}

#[test]
fn offsets_follow_the_occurrence_the_note_now_has_the_quote_at() {
    let src = "# A\n\nBefore: check the gauge now and log the reading in the book. After: check the gauge later.\n";
    let r = added(src, "A Before: ", &["check the gauge"], 3);
    // Edited outside Lectern: the first one's "Before:" became "Today:", and the second has it.
    let edited = "# A\n\nToday: check the gauge now and log the reading in the book. Before: check the gauge later.\n";

    let p = payload_for(Some(&r), edited, true);

    let c = &p.comments[0];
    assert_eq!(c.state, AnchorState::Anchored);
    // What follows it still says it's the first.
    assert_eq!((c.text_start, c.text_end), (Some(9), Some(24)));
}

#[test]
fn unreadable_sections_are_reported_and_summary_is_not() {
    let mut text = format::serialize(&format::new_review("tide.md"));
    text.push_str(
        "## C1 · open · L5 · Tide sync › Batching\n> batches of at most 50\n\n**You:** Why 50?\n\n",
    );
    text.push_str("## C2 · wontfix · L9 · Tide sync › Retries\n> Retry with jitter\n\n**You:** Which formula?\n\n");
    text.push_str("## Changes\n\nNone yet.\n\n");
    text.push_str("## Summary\n\nOne comment, one unreadable.\n");
    let r = format::parse(&text);

    let p = payload_for(Some(&r), NOTE, true);

    let ids: Vec<u32> = p.comments.iter().map(|c| c.id).collect();
    assert_eq!(ids, [1]);
    assert_eq!(
        (p.comments[0].text_start, p.comments[0].text_end),
        (Some(50), Some(71)),
        "found by its quote"
    );
    assert_eq!(p.unreadable.len(), 1, "{:?}", p.unreadable);
    assert!(p.unreadable[0]
        .raw
        .starts_with("## C2 · wontfix · L9 · Tide sync › Retries\n"));
    assert!(p.unreadable[0].raw.contains("**You:** Which formula?"));
    assert_eq!(p.open_count, 1);
}

#[test]
fn a_file_that_is_not_a_lectern_sidecar_has_nothing_unreadable() {
    // An ordinary note that happens to be called like a sidecar, one heading shaped like an id.
    let r = format::parse(
        "# Code review checklist\n\n## Correctness\n\n- Empty input is handled.\n\n## C2 rollout\n\n- Staged behind a flag.\n",
    );
    assert_eq!(r.note, "", "no sidecar frontmatter");

    let p = payload_for(Some(&r), NOTE, true);

    assert!(p.unreadable.is_empty(), "{:?}", p.unreadable);
    assert!(p.comments.is_empty());
}

/// The markup of every tag in `html`: what's between each `<` and the next `>`. Escaped text
/// (`&lt;…&gt;`) is shown, not run, so it isn't markup.
fn tags(html: &str) -> Vec<&str> {
    html.split('<')
        .skip(1)
        .map(|tag| tag.split('>').next().unwrap_or(tag))
        .collect()
}

#[test]
fn entry_html_is_sanitised() {
    // On one line the whole entry is a raw HTML block, which the raw-HTML policy shows as text;
    // as separate paragraphs each part goes through the renderer as markup.
    let one_line =
        "<script>alert(1)</script> [x](javascript:alert(1)) <img src=x onerror=alert(1)>";
    let paragraphs =
        "<script>alert(1)</script>\n\n[x](javascript:alert(1))\n\n<img src=x onerror=alert(1)>";
    let mut r = format::new_review("tide.md");
    let id = add(&mut r, 5, "batches of at most 50", one_line);
    apply(
        &mut r,
        ReviewOp::Reply {
            id,
            text: paragraphs.into(),
        },
    );
    let tm = TextMap::build(NOTE);
    let resolved = anchor::resolve_all(&r, &tm, &fingerprint(NOTE));
    let mapper = PathMapper::default();
    let ctx = RenderContext {
        doc_path: Path::new(NOTE_PATH),
        index: None,
        mapper: &mapper,
        asset_base: "http://lxasset.localhost/",
        trusted_unc_hosts: &[],
    };

    let p = build_payload(
        PayloadInput {
            note: Path::new(NOTE_PATH),
            sidecar: Path::new(SIDECAR_PATH),
            review: Some(&r),
            resolved: &resolved,
            text: Some(&tm),
            read_only: None,
            note_wsl: None,
            sidecar_wsl: None,
        },
        &|md| render(md, &ctx).html,
    );

    let entries = &p.comments[0].entries;
    assert_eq!(entries[0].text, one_line, "the text is kept as written");
    assert_eq!(entries[1].text, paragraphs);
    for entry in entries {
        let html = entry.html.to_ascii_lowercase();
        assert!(!html.contains("<script"), "{html}");
        for tag in tags(&html) {
            assert!(!tag.contains("javascript:"), "{tag} in {html}");
            assert!(!tag.contains("onerror"), "{tag} in {html}");
        }
    }
    let html = &entries[1].html;
    assert!(html.contains(">x</a>"), "the link is still a link: {html}");
    assert!(!html.contains("javascript:"), "{html}");
    assert!(!html.contains("onerror"), "{html}");
}
