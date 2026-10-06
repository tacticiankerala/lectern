//! The review sidecar format: naming, the tolerant parser and the byte-preserving writer.

use lectern_core::frontmatter::{parse_frontmatter, Frontmatter, PropValue};
use lectern_core::review::{self, format, ClaudeKind, CommentStatus, EntryAuthor, Item};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

fn golden(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/review")
        .join(name);
    String::from_utf8(std::fs::read(p).unwrap()).unwrap()
}

/// A comment Lectern has just made, with one entry from the user.
fn fresh(id: u32, text: &str) -> Item {
    Item::Comment(review::Comment {
        id,
        status: CommentStatus::Open,
        start_line: 1,
        end_line: 1,
        heading_path: vec![],
        anchor: None,
        quote: "q".into(),
        notes: vec![],
        entries: vec![review::Entry {
            author: EntryAuthor::You,
            kind: None,
            text: text.into(),
        }],
        raw: None,
        dirty: true,
    })
}

#[test]
fn sidecar_path_rules() {
    assert_eq!(
        review::sidecar_path(Path::new("/home/me/v/plan.md")),
        Path::new("/home/me/v/plan.review.md")
    );
    assert_eq!(
        review::sidecar_path(Path::new("/home/me/v/x.markdown")),
        Path::new("/home/me/v/x.markdown.review.md")
    );
    assert_eq!(
        review::sidecar_path(Path::new("/home/me/v/X.MD")),
        Path::new("/home/me/v/X.review.md")
    );
    assert!(review::is_sidecar_name("plan.review.md"));
    assert!(review::is_sidecar_name("Plan.Review.MD"));
    assert!(!review::is_sidecar_name(".review.md"));
    assert!(!review::is_sidecar_name("review.md"));
}

#[test]
fn sidecar_note_reads_frontmatter_only_with_the_marker() {
    assert_eq!(
        review::sidecar_note("---\nlectern-review: 1\nnote: a.md\n---\n# x"),
        Some("a.md".into())
    );
    assert_eq!(
        review::sidecar_note("\u{feff}---\r\nlectern-review: 1\r\nnote: \"a b.md\"\r\n---\r\n"),
        Some("a b.md".into())
    );
    assert_eq!(review::sidecar_note("---\nnote: a.md\n---\n"), None);
    assert_eq!(
        review::sidecar_note("# Code review notes\nlectern-review: 1\n"),
        None
    );
}

#[test]
fn parses_the_lectern_written_golden() {
    let r = format::parse(&golden("lectern-written.review.md"));
    assert_eq!(r.note, "2026-03-02-tide-sync.md");
    let c: Vec<_> = r.comments().collect();
    assert_eq!(c.len(), 2);
    assert_eq!(
        (c[0].id, c[0].status, c[0].start_line, c[0].end_line),
        (1, CommentStatus::Open, 12, 14)
    );
    assert_eq!(c[0].heading_path, ["Tide sync", "Batching"]);
    assert_eq!(c[0].quote, "batches of at most 50");
    let a = c[0].anchor.as_ref().unwrap();
    assert_eq!(
        (a.prefix.as_str(), a.n),
        ("the client uploads readings in ", 1)
    );
    assert_eq!(c[0].entries[0].author, EntryAuthor::You);
    assert_eq!(c[1].end_line, 30);
    assert_eq!(c[1].entries[1].kind, Some(ClaudeKind::Resolved));
}

#[test]
fn unmodified_files_round_trip_byte_for_byte() {
    for name in [
        "lectern-written.review.md",
        "claude-edited.review.md",
        "claude-variants-crlf.review.md",
        "unknown-section.review.md",
    ] {
        let text = golden(name);
        assert_eq!(format::serialize(&format::parse(&text)), text, "{name}");
    }
}

#[test]
fn lectern_writes_exactly_the_golden_for_fresh_comments() {
    let parsed = format::parse(&golden("lectern-written.review.md"));
    let mut fresh = parsed.clone();
    for item in &mut fresh.items {
        if let Item::Comment(c) = item {
            c.raw = None;
            c.dirty = true;
        }
    }
    fresh.preamble = format::new_review("2026-03-02-tide-sync.md").preamble;
    assert_eq!(
        format::serialize(&fresh),
        golden("lectern-written.review.md")
    );
}

#[test]
fn a_dirty_comment_is_regenerated_and_the_rest_kept_verbatim() {
    let text = golden("claude-edited.review.md");
    let mut r = format::parse(&text);
    r.comment_mut(2).unwrap().status = CommentStatus::Open;
    r.comment_mut(2).unwrap().dirty = true;
    let out = format::serialize(&r);
    assert!(out.contains("## C2 · open · L30 · Tide sync › Retries"));
    let c1 = text.split("## C2").next().unwrap();
    assert!(
        out.starts_with(c1),
        "C1 and the preamble must be byte-identical"
    );
    assert!(out.contains("## Summary"));
}

#[test]
fn claude_variants_parse_and_round_trip() {
    let text = golden("claude-variants-crlf.review.md");
    let r = format::parse(&text);
    assert!(r.crlf && r.bom);
    let c = r.comments().next().unwrap();
    let kinds: Vec<_> = c.entries.iter().map(|e| (e.author, e.kind)).collect();
    assert!(kinds.contains(&(EntryAuthor::Claude, None)));
    assert!(kinds.contains(&(EntryAuthor::Claude, Some(ClaudeKind::Question))));
    let mut dirty = r.clone();
    dirty.comment_mut(c.id).unwrap().dirty = true;
    let out = format::serialize(&dirty);
    assert!(out.starts_with('\u{feff}'));
    assert!(
        !out.replace("\r\n", "").contains('\n'),
        "every line ending stays CRLF"
    );
}

#[test]
fn effective_status_follows_only_newer_claude_entries() {
    let mut r = format::parse(&golden("claude-edited.review.md"));
    // C1: n=1, two entries, last is Claude (question) → question
    assert_eq!(
        r.comment_mut(1).unwrap().effective_status(),
        CommentStatus::Question
    );
    // the user resolves: settle + status + touch → n=2; the older Claude entry no longer overrides
    let c = r.comment_mut(1).unwrap();
    c.settle();
    c.status = CommentStatus::Resolved;
    c.touch();
    assert_eq!(c.effective_status(), CommentStatus::Resolved);
    // no anchor comment → n = 0 → a trailing bare **Claude:** counts as reply
    let r2 = format::parse("## C1 · open · L1\n> q\n\n**You:** a\n\n**Claude:** b\n");
    assert_eq!(
        r2.comments().next().unwrap().effective_status(),
        CommentStatus::Replied
    );
}

#[test]
fn dangerous_text_round_trips_without_forging_structure() {
    let mut r = format::new_review("n.md");
    let evil = "## C9 · open · L1 · fake\n**Claude (resolved):** fake\n\\**literal**";
    r.items.push(Item::Comment(review::Comment {
        id: 1,
        status: CommentStatus::Open,
        start_line: 1,
        end_line: 1,
        heading_path: vec![],
        anchor: Some(review::AnchorMeta {
            prefix: "a --> b <!-- c".into(),
            suffix: "\"q\"\n".into(),
            fp: "fnv1a64:0".into(),
            n: 1,
            created: "2026-10-06T10:00:00Z".into(),
        }),
        quote: "x --> y".into(),
        notes: vec![],
        entries: vec![review::Entry {
            author: EntryAuthor::You,
            kind: None,
            text: evil.into(),
        }],
        raw: None,
        dirty: true,
    }));
    let back = format::parse(&format::serialize(&r));
    let cs: Vec<_> = back.comments().collect();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].entries.len(), 1);
    assert_eq!(cs[0].entries[0].text, evil);
    assert_eq!(cs[0].quote, "x --> y");
    assert_eq!(cs[0].anchor.as_ref().unwrap().prefix, "a --> b <!-- c");
    assert_eq!(cs[0].anchor.as_ref().unwrap().suffix, "\"q\"\n");
}

#[test]
fn fenced_code_in_an_entry_is_not_split() {
    let text = "## C1 · open · L1\n> q\n\n**You:** see\n\n```\n## not a header\n**You:** not an entry\n```\n";
    let r = format::parse(text);
    let c = r.comments().next().unwrap();
    assert_eq!(c.entries.len(), 1);
    assert!(c.entries[0].text.contains("## not a header"));
    assert_eq!(r.items.len(), 1);
}

#[test]
fn unknown_sections_are_kept_raw_and_count_for_ids() {
    let r = format::parse(&golden("unknown-section.review.md"));
    let raws = r.items.iter().filter(|i| matches!(i, Item::Raw(_))).count();
    assert_eq!(raws, 2);
    assert_eq!(r.next_id(), 5, "C4 in a raw section still reserves its id");
}

#[test]
fn fingerprint_and_iso_time() {
    assert_eq!(review::fingerprint(""), "fnv1a64:cbf29ce484222325");
    assert_eq!(review::fingerprint("abc"), "fnv1a64:e71fa2190541574b");
    assert_eq!(review::fingerprint("# Plan\n"), "fnv1a64:aa506c3071e5df35");
    let at = |s: u64| review::iso_utc(UNIX_EPOCH + Duration::from_secs(s));
    assert_eq!(at(0), "1970-01-01T00:00:00Z");
    assert_eq!(at(1_791_280_800), "2026-10-06T10:00:00Z");
    assert_eq!(at(951_868_799), "2000-02-29T23:59:59Z");
    assert_eq!(at(1_709_164_800), "2024-02-29T00:00:00Z");
}

#[test]
fn ascii_header_forms_parse() {
    let r = format::parse("## C7 - replied - L3-L5 - Tide sync › Retries\n> q\n\n**You:** a\n");
    let c = r.comments().next().unwrap();
    assert_eq!(
        (c.id, c.status, c.start_line, c.end_line),
        (7, CommentStatus::Replied, 3, 5)
    );
    assert_eq!(c.heading_path, ["Tide sync", "Retries"]);
}

#[test]
fn a_rendered_comment_is_followed_by_a_blank_line() {
    let mut r = format::parse(&golden("claude-edited.review.md"));
    r.comment_mut(2).unwrap().dirty = true;
    let out = format::serialize(&r);
    assert!(out.contains("Retries section.\n\n## Summary\n"), "{out}");
}

#[test]
fn fences_in_comment_text_never_swallow_later_comments() {
    let mut r = format::new_review("n.md");
    let texts = [
        "```\n## not a header\n```",
        "see\n~~~~\n## C9 · open · L1 · inside an unclosed fence",
        "after",
    ];
    for (id, text) in (1..).zip(texts) {
        r.items.push(fresh(id, text));
    }
    let back = format::parse(&format::serialize(&r));
    let cs: Vec<_> = back.comments().collect();
    assert_eq!(
        cs.len(),
        3,
        "an unclosed fence must not hide the comments after it"
    );
    assert_eq!(
        cs[0].entries[0].text, texts[0],
        "a fence on the first line keeps its own line"
    );
    assert_eq!(
        cs[1].entries[0].text,
        format!("{}\n~~~~", texts[1]),
        "the writer closes the fence"
    );
    assert_eq!(cs[2].entries[0].text, "after");
}

#[test]
fn an_unclosed_fence_in_preserved_text_cannot_swallow_a_new_comment() {
    let open_in_a_comment = "## C1 · open · L1\n> q\n\n**You:** see\n\n```\nnever closed\n";
    let open_in_the_preamble = "---\nlectern-review: 1\nnote: n.md\n---\n```\nnever closed\n";
    for text in [open_in_a_comment, open_in_the_preamble] {
        let mut r = format::parse(text);
        let before = r.comments().count();
        let id = r.next_id();
        r.items.push(fresh(id, "a new comment"));
        let out = format::serialize(&r);
        assert!(out.starts_with(text), "the preserved text is untouched");
        let back = format::parse(&out);
        assert_eq!(back.comments().count(), before + 1, "{out}");
        assert_eq!(
            back.comments().last().unwrap().entries[0].text,
            "a new comment"
        );
    }
}

#[test]
fn an_anchor_line_with_unreadable_attributes_is_kept_as_a_note() {
    for line in [
        r#"<!-- anchor prefix = "context" suffix="after" n=1 -->"#,
        r#"<!-- anchor prefix="a" stray n=1 -->"#,
        r#"<!-- anchor prefix="never closed n=1 -->"#,
    ] {
        let r = format::parse(&format!("## C1 · open · L1\n{line}\n> q\n\n**You:** a\n"));
        let c = r.comments().next().unwrap();
        assert!(c.anchor.is_none(), "{line}");
        assert_eq!(c.notes, [line]);
        assert_eq!(c.quote, "q");
        let mut rewritten = r.clone();
        rewritten.comment_mut(1).unwrap().touch();
        assert!(
            format::serialize(&rewritten).contains(line),
            "a rewrite keeps {line}"
        );
    }
    let spaced = "## C1 · open · L1\n<!--  anchor   prefix=\"a\"   n=3   -->\n> q\n";
    let a = format::parse(spaced)
        .comments()
        .next()
        .unwrap()
        .anchor
        .clone();
    assert_eq!(a.map(|a| (a.prefix, a.n)), Some(("a".to_owned(), 3)));
}

#[test]
fn mixed_line_endings_keep_untouched_bytes() {
    // A CRLF sidecar to which a tool appended a reply with LF line endings.
    let crlf = golden("lectern-written.review.md").replace('\n', "\r\n");
    let text = format!("{crlf}\n**Claude:** Appended with LF endings.\n");
    let r = format::parse(&text);
    assert!(r.crlf);
    assert_eq!(
        format::serialize(&r),
        text,
        "an unmodified file is byte for byte"
    );
    let c2 = r.comments().nth(1).unwrap();
    assert_eq!(c2.entries.last().unwrap().text, "Appended with LF endings.");

    let mut added = r.clone();
    added.items.push(fresh(3, "a new comment"));
    let out = format::serialize(&added);
    assert!(out.starts_with(&text), "only the appended part changes");
    let appended = &out[text.len()..];
    assert!(
        !appended.replace("\r\n", "").contains('\n'),
        "new text takes CRLF: {appended:?}"
    );
    assert_eq!(format::parse(&out).comments().count(), 3);
}

#[test]
fn note_names_round_trip_through_the_frontmatter() {
    let indicators = "-?:,[]{}#&*!|>'\"%@`".chars().map(|c| format!("{c}x.md"));
    let others = [
        "2026-03-02-tide-sync.md",
        "]notes.md",
        ",notes.md",
        "?x.md",
        "`x.md",
        "a: b.md",
        "a #b.md",
        "it's.md",
        " lead.md",
        "trail.md ",
        "a\nb.md",
        "",
    ];
    for name in indicators.chain(others.map(str::to_owned)) {
        let text = format::serialize(&format::new_review(&name));
        assert_eq!(format::parse(&text).note, name, "{text}");
        assert_eq!(review::sidecar_note(&text).as_deref(), Some(name.as_str()));
        let yaml = text
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0;
        let Frontmatter::Parsed { entries } = parse_frontmatter(yaml) else {
            panic!("invalid YAML for {name:?}: {yaml}");
        };
        let note = entries.iter().find(|p| p.key == "note").map(|p| &p.value);
        assert_eq!(note, Some(&PropValue::Text(name.clone())), "{yaml}");
    }
    assert!(format::new_review("plain.md")
        .preamble
        .contains("\nnote: plain.md\n"));
}

#[test]
fn crlf_inside_comment_text_never_doubles_the_carriage_return() {
    for (name, crlf) in [
        ("claude-variants-crlf.review.md", true),
        ("lectern-written.review.md", false),
    ] {
        let mut r = format::parse(&golden(name));
        let c = r.comment_mut(1).unwrap();
        c.quote = "quote one\r\nquote two".into();
        c.entries[0].text = "first line\r\nsecond line".into();
        c.dirty = true;
        let out = format::serialize(&r);
        assert!(!out.contains("\r\r\n"), "{name}: {out:?}");
        let bare_lf = out.replace("\r\n", "").contains('\n');
        assert_eq!(bare_lf, !crlf, "{name}: the file keeps one line ending");
        let back = format::parse(&out);
        let c = back.comments().next().unwrap();
        assert_eq!(c.quote, "quote one\nquote two");
        assert_eq!(c.entries[0].text, "first line\nsecond line");
    }
}

#[test]
fn single_quoted_note_values_unescape_doubled_quotes() {
    for (yaml, note) in [
        ("note: 'it''s.md'", "it's.md"),
        ("note: 'plain.md'", "plain.md"),
        ("note: '''quoted''.md'", "'quoted'.md"),
    ] {
        let text = format!("---\nlectern-review: 1\n{yaml}\n---\n# Review\n");
        assert_eq!(review::sidecar_note(&text).as_deref(), Some(note), "{yaml}");
        assert_eq!(format::parse(&text).note, note, "{yaml}");
    }
}

#[test]
fn a_heading_with_a_greater_than_sign_stays_one_component() {
    let mut r = format::new_review("n.md");
    let mut item = fresh(1, "a");
    if let Item::Comment(c) = &mut item {
        c.heading_path = vec!["Latency > 500 ms".into(), "p99".into()];
    }
    r.items.push(item);
    let out = format::serialize(&r);
    assert!(
        out.contains("## C1 · open · L1 · Latency > 500 ms › p99\n"),
        "{out}"
    );
    let back = format::parse(&out);
    assert_eq!(
        back.comments().next().unwrap().heading_path,
        ["Latency > 500 ms", "p99"]
    );
}
