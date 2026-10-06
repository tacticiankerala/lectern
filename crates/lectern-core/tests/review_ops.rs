//! The operations the UI sends: add, reply, set status, reattach and edit.

use lectern_core::review::{
    fingerprint, format,
    ops::{self, NewAnchor, OpContext, OpError, ReviewOp, StatusChange},
    text::TextMap,
    ClaudeKind, CommentStatus, EntryAuthor, Item, Review, MAX_COMMENT_CHARS,
};

const NOTE: &str = "# Tide sync\n\n## Batching\n\nThe client uploads readings in batches of at most 50.\n\n## Retries\n\nRetry with jitter after a failed upload.\n";

const T1: &str = "2026-10-06T10:00:00Z";
const T2: &str = "2026-10-06T11:30:00Z";

/// Applies `op` to `r` against `NOTE`, at time `now`.
fn apply_at(r: &mut Review, op: ReviewOp, now: &str) -> Result<u32, OpError> {
    let tm = TextMap::build(NOTE);
    let fp = fingerprint(NOTE);
    let ctx = OpContext {
        text: &tm,
        fingerprint: &fp,
        now,
    };
    ops::apply(r, &op, &ctx)
}

fn apply(r: &mut Review, op: ReviewOp) -> Result<u32, OpError> {
    apply_at(r, op, T1)
}

fn new_anchor(line: u32, quote: &str) -> NewAnchor {
    NewAnchor {
        start_line: line,
        end_line: line,
        quote: quote.into(),
        prefix: String::new(),
    }
}

fn add_op(line: u32, quote: &str, text: &str) -> ReviewOp {
    ReviewOp::Add {
        anchor: new_anchor(line, quote),
        text: text.into(),
    }
}

fn reply(id: u32, text: &str) -> ReviewOp {
    ReviewOp::Reply {
        id,
        text: text.into(),
    }
}

fn set_status(id: u32, change: StatusChange) -> ReviewOp {
    ReviewOp::SetStatus { id, change }
}

fn comment(r: &Review, id: u32) -> &lectern_core::review::Comment {
    r.comments().find(|c| c.id == id).unwrap()
}

/// A review with one comment made by Lectern on "batches of at most 50".
fn one_comment() -> Review {
    let mut r = format::new_review("tide.md");
    assert_eq!(
        apply(&mut r, add_op(5, "batches of at most 50", "Why 50?")),
        Ok(1)
    );
    r
}

#[test]
fn add_assigns_increasing_ids_and_n_is_one() {
    let mut r = one_comment();
    assert_eq!(
        apply(&mut r, add_op(9, "Retry with jitter", "Which formula?")),
        Ok(2)
    );
    let ids: Vec<u32> = r.comments().map(|c| c.id).collect();
    assert_eq!(ids, [1, 2], "new comments go last");

    let c = comment(&r, 2);
    assert_eq!(c.status, CommentStatus::Open);
    assert_eq!((c.start_line, c.end_line), (9, 9));
    assert_eq!(c.heading_path, ["Tide sync", "Retries"]);
    assert_eq!(c.quote, "Retry with jitter");
    assert_eq!(c.entries.len(), 1);
    assert_eq!(c.entries[0].author, EntryAuthor::You);
    assert_eq!(c.entries[0].text, "Which formula?");
    assert!(c.dirty && c.raw.is_none());
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(a.n, 1);
    assert_eq!(a.created, T1);
    assert_eq!(a.fp, fingerprint(NOTE));
    assert_eq!(a.suffix, " after a failed upload.");

    // Ids count raw `## C<n>` sections too, so none is ever reused.
    let mut r = format::parse("## C7 - not a status - L1\nkept as is\n");
    assert!(matches!(r.items[0], Item::Raw(_)));
    assert_eq!(apply(&mut r, add_op(9, "Retry", "x")), Ok(8));
}

#[test]
fn reply_reopens_and_counts_entries() {
    let mut r = one_comment();
    apply(&mut r, set_status(1, StatusChange::Resolve)).unwrap();
    assert_eq!(comment(&r, 1).status, CommentStatus::Resolved);

    assert_eq!(apply(&mut r, reply(1, "Still unsure.")), Ok(1));
    let c = comment(&r, 1);
    assert_eq!(c.status, CommentStatus::Open);
    assert_eq!(c.entries.len(), 2);
    assert_eq!(c.anchor.as_ref().unwrap().n as usize, c.entries.len());
    let last = c.entries.last().unwrap();
    assert_eq!(
        (last.author, last.kind, last.text.as_str()),
        (EntryAuthor::You, None, "Still unsure.")
    );
    assert!(c.dirty);
}

#[test]
fn set_status_overrides_an_older_claude_entry() {
    let text = "## C1 · open · L5 · Tide sync › Batching\n\
                <!-- anchor prefix=\"\" suffix=\"\" fp=\"\" n=1 created=\"2026-10-06T09:00:00Z\" -->\n\
                > batches of at most 50\n\n\
                **You:** Why 50?\n\n\
                **Claude (question):** Do you mean the batch size or the retry cap?\n";
    let mut r = format::parse(text);
    assert_eq!(comment(&r, 1).effective_status(), CommentStatus::Question);

    apply(&mut r, set_status(1, StatusChange::Resolve)).unwrap();
    let c = comment(&r, 1);
    assert_eq!(c.status, CommentStatus::Resolved);
    assert_eq!(c.effective_status(), CommentStatus::Resolved);
    assert_eq!(
        c.anchor.as_ref().unwrap().n,
        2,
        "Claude's entry is now seen"
    );
    assert_eq!(c.entries[1].kind, Some(ClaudeKind::Question));

    let back = format::parse(&format::serialize(&r));
    assert_eq!(
        comment(&back, 1).effective_status(),
        CommentStatus::Resolved,
        "the resolve survives a save"
    );

    apply(&mut r, set_status(1, StatusChange::Reopen)).unwrap();
    assert_eq!(comment(&r, 1).status, CommentStatus::Open);
    apply(&mut r, set_status(1, StatusChange::Dismiss)).unwrap();
    assert_eq!(comment(&r, 1).status, CommentStatus::Dismissed);
}

#[test]
fn edit_only_your_own_entries() {
    let text = "## C1 · open · L5\n> batches of at most 50\n\n\
                **You:** Why 50?\n\n\
                **Claude (reply):** It keeps each request under a second.\n";
    let mut r = format::parse(text);
    let edit = |entry: u32, text: &str| ReviewOp::Edit {
        id: 1,
        entry,
        text: text.into(),
    };

    assert_eq!(
        apply(&mut r, edit(1, "Rewritten")),
        Err(OpError::NotYourEntry)
    );
    assert_eq!(
        apply(&mut r, edit(2, "Rewritten")),
        Err(OpError::NoSuchEntry)
    );
    let c = comment(&r, 1);
    assert_eq!(c.entries[1].text, "It keeps each request under a second.");
    assert!(!c.dirty, "a refused edit changes nothing");

    assert_eq!(apply(&mut r, edit(0, "  Why 50, not 100?\r\n")), Ok(1));
    let c = comment(&r, 1);
    assert_eq!(c.entries[0].text, "Why 50, not 100?");
    assert_eq!(c.entries.len(), 2);
    assert!(c.dirty);
    assert_eq!(c.anchor.as_ref().unwrap().n, 2);
}

#[test]
fn empty_and_too_long_text_rejected() {
    let mut r = format::new_review("tide.md");
    assert_eq!(
        apply(&mut r, add_op(5, "batches", " \r\n\t ")),
        Err(OpError::Empty)
    );
    assert_eq!(r.comments().count(), 0, "nothing is added on an error");

    let mut r = one_comment();
    assert_eq!(apply(&mut r, reply(1, "")), Err(OpError::Empty));
    let too_long = "é".repeat(MAX_COMMENT_CHARS + 1);
    assert_eq!(apply(&mut r, reply(1, &too_long)), Err(OpError::TooLong));
    assert_eq!(comment(&r, 1).entries.len(), 1);

    let longest = "é".repeat(MAX_COMMENT_CHARS);
    assert_eq!(apply(&mut r, reply(1, &longest)), Ok(1));
    assert_eq!(apply(&mut r, reply(1, "  one\r\ntwo  ")), Ok(1));
    assert_eq!(comment(&r, 1).entries.last().unwrap().text, "one\ntwo");

    assert_eq!(OpError::Empty.to_string(), "Write something first.");
    assert_eq!(
        OpError::TooLong.to_string(),
        "That comment is too long (over 20,000 characters)."
    );
}

#[test]
fn reattach_keeps_created_and_status() {
    let mut r = one_comment();
    apply(&mut r, set_status(1, StatusChange::Resolve)).unwrap();
    let op = ReviewOp::Reattach {
        id: 1,
        anchor: new_anchor(9, "after a failed upload"),
    };
    assert_eq!(apply_at(&mut r, op, T2), Ok(1));

    let c = comment(&r, 1);
    assert_eq!(c.status, CommentStatus::Resolved);
    assert_eq!(c.quote, "after a failed upload");
    assert_eq!((c.start_line, c.end_line), (9, 9));
    assert_eq!(c.heading_path, ["Tide sync", "Retries"]);
    assert_eq!(c.entries.len(), 1, "the thread is untouched");
    assert!(c.dirty);
    let a = c.anchor.as_ref().unwrap();
    assert_eq!(a.created, T1, "created is kept");
    assert_eq!(a.prefix, "t 50. Retries Retry with jitter ");
    assert_eq!(a.suffix, ".");
    assert_eq!(a.fp, fingerprint(NOTE));
    assert_eq!(a.n, 1);
}

#[test]
fn ops_on_a_missing_comment_error() {
    let mut r = one_comment();
    let before = r.clone();
    let ops = [
        reply(99, "hello"),
        set_status(99, StatusChange::Dismiss),
        ReviewOp::Reattach {
            id: 99,
            anchor: new_anchor(5, "batches"),
        },
        ReviewOp::Edit {
            id: 99,
            entry: 0,
            text: "hello".into(),
        },
    ];
    for op in ops {
        assert_eq!(
            apply(&mut r, op.clone()),
            Err(OpError::NoSuchComment(99)),
            "{op:?}"
        );
    }
    assert_eq!(r, before, "a failed op changes nothing");
    assert_eq!(
        OpError::NoSuchComment(99).to_string(),
        "That comment no longer exists."
    );
    assert_eq!(
        OpError::NoSuchEntry.to_string(),
        "That reply no longer exists."
    );
    assert_eq!(
        OpError::NotYourEntry.to_string(),
        "Only your own replies can be edited."
    );
}
