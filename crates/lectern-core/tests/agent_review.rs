//! The review tools, in core: where an agent's comment goes in a note or a text file (a quote, a
//! heading or a line becomes the comment's quote), the operations its tools make, saved through the
//! sidecar store as the agent's own entries, the threads `comments` lists, and the error for a
//! thread that isn't there.

use std::fs;

use lectern_core::agent::request::{EntryKind, Target, ThreadFilter};
use lectern_core::agent::review::{
    comment_anchor, comment_op, list_threads, no_such_thread, reply_op, resolve_op, thread_id,
    thread_number, threads_page, CommentAnswer, ThreadAnswer, ThreadState, COMMENTS_BYTES,
};
use lectern_core::agent::status::AgentReply;
use lectern_core::ipc::PathForms;
use lectern_core::review::ops::{AgentAuthor, NewAnchor, ReviewOp};
use lectern_core::review::store::{self, StoreError};
use lectern_core::review::text::{NoteText, TextMap};
use lectern_core::review::{format, sidecar_path, ClaudeKind, CommentStatus, MAX_SIDECAR_BYTES};
use serde_json::json;

/// Plan, with a passage on line 5 that has a Markdown mark, and line 2 blank. Its text as shown is
/// "Plan Rollout Roll back the migration first. Then ship."
const PLAN: &str = "# Plan\n\n## Rollout\n\nRoll back the migration **first**.\n\nThen ship.\n";

/// A text file: two paragraphs, on lines 1–2 and 4–5.
const README: &str = "Readme for the vault.\nEvery word is invented.\n\nThe lanternfish survey is plain text,\nso search reads it.\n";

const NOW: &str = "2026-10-16T10:00:00Z";

fn quote(text: &str) -> Target {
    Target::Quote {
        quote: text.to_owned(),
        prefix: None,
        suffix: None,
    }
}

fn anchor(lines: (u32, u32), quote: &str, prefix: &str) -> NewAnchor {
    NewAnchor {
        start_line: lines.0,
        end_line: lines.1,
        quote: quote.to_owned(),
        prefix: prefix.to_owned(),
        text_start: None,
    }
}

/// `anchor`, and where it starts in the text as shown, in UTF-16 code units.
fn anchor_at(lines: (u32, u32), quote: &str, prefix: &str, text_start: u32) -> NewAnchor {
    NewAnchor {
        text_start: Some(text_start),
        ..anchor(lines, quote, prefix)
    }
}

/// Where a comment on `target` in plan.md (`PLAN`) goes.
fn on_plan(target: &Target) -> Result<NewAnchor, String> {
    let text = TextMap::of_note("plan.md", PLAN).unwrap();
    comment_anchor(&text, PLAN, "plan.md", target)
}

#[test]
fn a_quote_a_heading_or_a_line_becomes_the_comments_quote() {
    assert_eq!(
        on_plan(&quote("the  migration")),
        Ok(anchor_at(
            (5, 5),
            "the migration",
            "Plan Rollout Roll back ",
            23
        ))
    );
    let heading = Target::Heading {
        heading: "rollout".to_owned(),
    };
    assert_eq!(
        on_plan(&heading),
        Ok(anchor_at((3, 3), "Rollout", "Plan ", 5))
    );
    // A line: the block holding it, whole.
    assert_eq!(
        on_plan(&Target::Line { line: 5 }),
        Ok(anchor_at(
            (5, 5),
            "Roll back the migration first.",
            "Plan Rollout ",
            13
        ))
    );
    // Quoted with its Markdown marks, as the source has it: the block on its lines, whole.
    assert_eq!(
        on_plan(&quote("migration **first**")),
        Ok(anchor_at(
            (5, 5),
            "Roll back the migration first.",
            "Plan Rollout ",
            13
        ))
    );
    assert_eq!(
        on_plan(&Target::Line { line: 2 }),
        Err("Line 2 of plan.md has no text to comment on.".to_owned())
    );
    assert_eq!(
        on_plan(&quote("roll forward")),
        Err("That passage isn't in plan.md.".to_owned())
    );
    assert_eq!(
        on_plan(&Target::Line { line: 99 }),
        Err("plan.md has 7 lines; line 99 is past its end.".to_owned())
    );
}

/// The operations the tools make: the agent's entry, of the kind asked for, else the tool's own.
#[test]
fn the_tools_make_the_agents_entries() {
    let claude = |kind| {
        Some(AgentAuthor {
            name: "Claude".to_owned(),
            kind,
        })
    };
    let at = anchor((5, 5), "the migration", "");
    assert_eq!(
        comment_op(at.clone(), "Why first?".to_owned(), "Claude", None),
        ReviewOp::Add {
            anchor: at.clone(),
            text: "Why first?".to_owned(),
            author: claude(ClaudeKind::Question),
        }
    );
    assert_eq!(
        comment_op(
            at.clone(),
            "Not first.".to_owned(),
            "Claude",
            Some(EntryKind::Pushback)
        ),
        ReviewOp::Add {
            anchor: at,
            text: "Not first.".to_owned(),
            author: claude(ClaudeKind::Pushback),
        }
    );
    assert_eq!(
        reply_op(3, "Per station.".to_owned(), "Claude", None),
        ReviewOp::Reply {
            id: 3,
            text: "Per station.".to_owned(),
            author: claude(ClaudeKind::Reply),
        }
    );
    assert_eq!(
        reply_op(
            3,
            "Which one?".to_owned(),
            "Claude",
            Some(EntryKind::Question)
        ),
        ReviewOp::Reply {
            id: 3,
            text: "Which one?".to_owned(),
            author: claude(ClaudeKind::Question),
        }
    );
    assert_eq!(
        resolve_op(3, None, "Claude"),
        ReviewOp::Reply {
            id: 3,
            text: "Resolved.".to_owned(),
            author: claude(ClaudeKind::Resolved),
        }
    );
    assert_eq!(
        resolve_op(3, Some("Fixed in the plan.".to_owned()), "Claude"),
        ReviewOp::Reply {
            id: 3,
            text: "Fixed in the plan.".to_owned(),
            author: claude(ClaudeKind::Resolved),
        }
    );
    assert_eq!(thread_id(3), "C3");
    assert_eq!(thread_number("C12"), Some(12));
    assert_eq!(thread_number("12"), None);
    assert_eq!(thread_number("C0"), None);
}

/// An agent comment on a text file no window shows: the sidecar gains a thread headed `open`, with
/// the quote and the agent's entry, never yours; a second is C2. The text file is never written.
#[test]
fn an_agent_comment_on_a_text_file_is_saved_as_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let readme = dir.path().join("readme.txt");
    fs::write(&readme, README).unwrap();
    let sidecar = sidecar_path(&readme);
    let text = TextMap::of_note("readme.txt", README).unwrap();
    let save = |target: &Target, said: &str, kind| {
        let at = comment_anchor(&text, README, "readme.txt", target).unwrap();
        let op = comment_op(at, said.to_owned(), "Claude", kind);
        store::apply_op(&sidecar, "readme.txt", README, &op, NOW).map(|(_, id)| id)
    };

    assert_eq!(
        save(&quote("lanternfish survey"), "Which survey?", None).unwrap(),
        1
    );
    // On the line its words are on, of the paragraph's two.
    let saved = fs::read_to_string(&sidecar).unwrap();
    assert!(saved.contains("\n## C1 · open · L4\n"), "{saved}");
    assert!(
        saved.contains("\n> lanternfish survey\n\n**Claude (question):** Which survey?\n"),
        "{saved}"
    );
    assert!(!saved.contains("**You:**"), "{saved}");

    assert_eq!(
        save(
            &Target::Line { line: 1 },
            "Which vault?",
            Some(EntryKind::Pushback)
        )
        .unwrap(),
        2
    );
    // A line of a paragraph over two: that line alone.
    let saved = fs::read_to_string(&sidecar).unwrap();
    assert!(saved.contains("\n## C2 · open · L1\n"), "{saved}");
    assert!(
        saved.contains("\n> Readme for the vault.\n\n**Claude (pushback):** Which vault?\n"),
        "{saved}"
    );
    assert_eq!(fs::read_to_string(&readme).unwrap(), README);
}

/// A comment on a line of a long paragraph (a text file with no blank line in 300) is on that
/// line, quoting it alone, and stays there as saved; a code fence has no text to comment on.
#[test]
fn a_comment_on_a_line_of_a_long_paragraph_is_on_that_line() {
    let survey: String = (1..=300)
        .map(|n| format!("Survey entry {n:03}: the lanternfish count.\n"))
        .collect();
    let text = TextMap::of_note("survey.txt", &survey).unwrap();
    let line = "Survey entry 250: the lanternfish count.";
    let start = text.text().find(line).unwrap();
    let at = comment_anchor(&text, &survey, "survey.txt", &Target::Line { line: 250 }).unwrap();
    assert_eq!(
        at,
        anchor_at(
            (250, 250),
            line,
            &text.text()[start - 32..start],
            u32::try_from(start).unwrap()
        )
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("survey.txt");
    fs::write(&path, &survey).unwrap();
    let op = comment_op(at, "Why so few?".to_owned(), "Claude", None);
    let (review, _) =
        store::apply_op(&sidecar_path(&path), "survey.txt", &survey, &op, NOW).unwrap();
    let c1 = review.comments().next().unwrap();
    assert_eq!((c1.start_line, c1.end_line), (250, 250));
    let saved = fs::read_to_string(sidecar_path(&path)).unwrap();
    assert!(saved.contains("\n## C1 · open · L250\n"), "{saved}");
    assert!(saved.contains(&format!("\n> {line}\n")), "{saved}");

    let note = "# Plan\n\n```sh\ncargo test\ncargo build\n```\n";
    let text = TextMap::of_note("plan.md", note).unwrap();
    assert_eq!(
        comment_anchor(&text, note, "plan.md", &Target::Line { line: 4 }).map(|at| (
            at.start_line,
            at.end_line,
            at.quote
        )),
        Ok((4, 4, "cargo test".to_owned()))
    );
    assert_eq!(
        comment_anchor(&text, note, "plan.md", &Target::Line { line: 3 }),
        Err("Line 3 of plan.md has no text to comment on.".to_owned())
    );
}

/// Plan, with a three-line paragraph on lines 3 to 5.
const STEPS: &str = "# Plan\n\nFreeze the writes.\nRoll back the migration.\nThaw the writes.\n";

/// The comments' lines, in file order.
fn comment_lines(review: &lectern_core::review::Review) -> Vec<(u32, u32)> {
    review
        .comments()
        .map(|c| (c.start_line, c.end_line))
        .collect()
}

/// `STEPS` with a paragraph added at its end, and a comment on it saved: the save finds every
/// comment again in the changed note.
fn add_a_paragraph_and_save(sidecar: &std::path::Path) -> lectern_core::review::Review {
    let changed = format!("{STEPS}\nA new paragraph, unrelated.\n");
    let op = ReviewOp::Add {
        anchor: anchor((7, 7), "unrelated", "A new paragraph, "),
        text: "New?".to_owned(),
        author: None,
    };
    store::apply_op(sidecar, "plan.md", &changed, &op, NOW)
        .unwrap()
        .0
}

/// A comment on the second line of a three-line paragraph stays on that line once the note
/// changes elsewhere: the next save finds it again where its quote sits, line by line.
#[test]
fn a_comment_on_a_line_of_a_paragraph_keeps_its_line_when_the_note_changes() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = sidecar_path(&dir.path().join("plan.md"));
    let text = TextMap::of_note("plan.md", STEPS).unwrap();
    let at = comment_anchor(&text, STEPS, "plan.md", &Target::Line { line: 4 }).unwrap();
    let op = comment_op(at, "Why roll back?".to_owned(), "Claude", None);
    store::apply_op(&sidecar, "plan.md", STEPS, &op, NOW).unwrap();
    let review = add_a_paragraph_and_save(&sidecar);
    assert_eq!(comment_lines(&review), [(4, 4), (7, 7)]);
    let saved = fs::read_to_string(&sidecar).unwrap();
    assert!(saved.contains("\n## C1 · open · L4 · Plan\n"), "{saved}");
}

/// The reader's own selection within one line of a longer paragraph is on that line, as saved and
/// once the note changes.
#[test]
fn the_readers_selection_within_a_line_of_a_paragraph_is_on_that_line() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = sidecar_path(&dir.path().join("plan.md"));
    // As the UI sends it: the paragraph's lines, and the words chosen.
    let yours = ReviewOp::Add {
        anchor: anchor((3, 5), "the migration", "Roll back "),
        text: "Which one?".to_owned(),
        author: None,
    };
    let (review, _) = store::apply_op(&sidecar, "plan.md", STEPS, &yours, NOW).unwrap();
    assert_eq!(comment_lines(&review), [(4, 4)]);
    let review = add_a_paragraph_and_save(&sidecar);
    assert_eq!(comment_lines(&review), [(4, 4), (7, 7)]);
}

/// A code span that runs over a line break: the line it ends on shows text, so a line target there
/// is never "no text"; the paragraph, whole, when its lines can't be told apart.
#[test]
fn a_line_a_code_span_runs_onto_has_text() {
    let note = "a `foo\nbar`\nlast\n";
    let text = TextMap::of_note("plan.md", note).unwrap();
    let at = comment_anchor(&text, note, "plan.md", &Target::Line { line: 2 }).unwrap();
    assert!(at.quote.contains("bar"), "{at:?}");
}

/// A file named as the note's sidecar that Lectern didn't make (no frontmatter) is refused, and the
/// sidecar cap holds as for your own comments: nothing is written either way.
#[test]
fn a_foreign_sidecar_and_the_size_cap_refuse_an_agents_comment() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("x.md");
    fs::write(&note, PLAN).unwrap();
    let sidecar = sidecar_path(&note);
    let own = "# My own notes\n\n## C1 · open · L1\n> Plan\n";
    fs::write(&sidecar, own).unwrap();
    let text = TextMap::of_note("x.md", PLAN).unwrap();
    let at = comment_anchor(&text, PLAN, "x.md", &quote("Then ship")).unwrap();
    let op = comment_op(at, "Ship when?".to_owned(), "Claude", None);
    let Err(refused) = store::apply_op(&sidecar, "x.md", PLAN, &op, NOW) else {
        panic!("wrote a foreign sidecar");
    };
    assert!(matches!(refused, StoreError::ReadOnly(_)), "{refused:?}");
    assert_eq!(
        refused.to_string(),
        "This file isn't a Lectern comments file, so Lectern won't change it."
    );
    assert_eq!(fs::read_to_string(&sidecar).unwrap(), own);

    let mut near_cap = format::serialize(&format::new_review("x.md")).into_bytes();
    near_cap.extend_from_slice(b"## Summary\n\n");
    near_cap.resize(usize::try_from(MAX_SIDECAR_BYTES).unwrap() - 100, b'x');
    near_cap.push(b'\n');
    fs::write(&sidecar, &near_cap).unwrap();
    // Not `unwrap_err`: that would print the whole 2 MB review.
    let Err(capped) = store::apply_op(&sidecar, "x.md", PLAN, &op, NOW) else {
        panic!("saved a sidecar past the cap");
    };
    assert_eq!(
        capped.to_string(),
        "Couldn't save the comment: the comments file would grow past 2 MB."
    );
    assert!(fs::read(&sidecar).unwrap() == near_cap, "the file changed");
}

/// plan.md's sidecar: C1 yours, open; C2 Claude's question; C3 pushed back; C4 resolved; C5
/// dismissed; C6 replied. C1 quotes a passage that isn't in the note any more.
const THREADS: &str = "---\nlectern-review: 1\nnote: plan.md\n---\n\n\
## C1 · open · L5 · Plan › Rollout\n> roll forward\n\n**You:** Why not?\n\n\
## C2 · open · L5 · Plan › Rollout\n> the migration\n\n**Claude (question):** Which one?\n\n\
## C3 · open · L7\n> Then ship.\n\n**You:** Ship it.\n\n**Codex (pushback):** Not yet.\n\n\
## C4 · resolved · L3\n> Rollout\n\n**You:** Rename?\n\n\
## C5 · dismissed · L3\n> Rollout\n\n**You:** Typo.\n\n\
## C6 · open · L1\n> Plan\n\n**You:** Title?\n\n**Claude:** Kept.\n";

/// The ids `filter` lists of `THREADS`, against plan.md's text.
fn listed(filter: Option<ThreadFilter>) -> Vec<String> {
    let review = format::parse(THREADS);
    let note = NoteText::of_note("plan.md", PLAN).unwrap();
    list_threads(&review, Some(&note), filter)
        .into_iter()
        .map(|t| t.id)
        .collect()
}

#[test]
fn comments_lists_the_threads_with_the_status_asked_for() {
    assert_eq!(listed(None), ["C1", "C2", "C3", "C6"], "none closed");
    assert_eq!(listed(Some(ThreadFilter::Open)), ["C1"]);
    assert_eq!(listed(Some(ThreadFilter::Question)), ["C2"]);
    assert_eq!(listed(Some(ThreadFilter::Pushback)), ["C3"]);
    assert_eq!(listed(Some(ThreadFilter::Resolved)), ["C4"]);
    assert_eq!(
        listed(Some(ThreadFilter::All)),
        ["C1", "C2", "C3", "C4", "C5", "C6"]
    );
}

/// Each thread as the Comments tab shows it: where it is anchored now, and its entries as written.
#[test]
fn a_thread_is_given_as_the_comments_tab_shows_it() {
    let review = format::parse(THREADS);
    let note = NoteText::of_note("plan.md", PLAN).unwrap();
    let threads = list_threads(&review, Some(&note), Some(ThreadFilter::All));
    let answer = serde_json::to_value(threads_page(threads, None, None)).unwrap();
    assert_eq!(answer["total"], 6);
    assert_eq!(answer.get("next"), None, "all of them fit");
    assert_eq!(
        answer["threads"][1],
        json!({
            "id": "C2",
            "status": "question",
            "state": "anchored",
            "startLine": 5,
            "endLine": 5,
            "headingPath": ["Plan", "Rollout"],
            "quote": "the migration",
            "entries": [
                {"author": "agent", "name": "Claude", "kind": "question", "text": "Which one?"},
            ],
        })
    );
    assert_eq!(answer["threads"][0]["state"], "detached");
    assert_eq!(
        answer["threads"][0]["entries"][0],
        json!({"author": "you", "name": "You", "text": "Why not?"})
    );
    assert_eq!(answer["threads"][5]["status"], "replied");
    assert_eq!(answer.get("readOnly"), None);
}

/// A page's threads when its text isn't known (no window shows it): at their stored places, in an
/// unknown state.
#[test]
fn threads_without_the_notes_text_are_unknown_at_their_stored_places() {
    let review = format::parse(THREADS);
    let threads = list_threads(&review, None, None);
    let c2 = &threads[1];
    assert_eq!(serde_json::to_value(c2).unwrap()["state"], json!("unknown"));
    assert_eq!((c2.start_line, c2.end_line), (5, 5));
    assert_eq!(c2.heading_path, ["Plan", "Rollout"]);
    assert_eq!(c2.status, CommentStatus::Question);
}

#[test]
fn a_thread_that_isnt_there_is_an_error_naming_those_that_are() {
    assert_eq!(
        no_such_thread(9, "plan.md", &[1, 2, 4]),
        "There's no thread C9 in plan.md. Its threads are C1, C2 and C4."
    );
    assert_eq!(
        no_such_thread(9, "plan.md", &[3]),
        "There's no thread C9 in plan.md. Its only thread is C3."
    );
    assert_eq!(
        no_such_thread(9, "plan.md", &[]),
        "There's no thread C9 in plan.md: it has no comments yet."
    );
}

/// What the tools answer, and the window's answer to a page's thread change.
#[test]
fn the_answers_carry_the_thread_and_its_sidecar_in_both_forms() {
    let sidecar = PathForms {
        windows: Some(r"S:\Notes\My Vault\plan.review.md".to_owned()),
        wsl: Some("/mnt/s/Notes/My Vault/plan.review.md".to_owned()),
    };
    assert_eq!(
        serde_json::to_value(CommentAnswer {
            id: thread_id(1),
            sidecar: sidecar.clone(),
        })
        .unwrap(),
        json!({"id": "C1", "sidecar": {
            "windows": r"S:\Notes\My Vault\plan.review.md",
            "wsl": "/mnt/s/Notes/My Vault/plan.review.md",
        }})
    );
    assert_eq!(
        serde_json::to_value(ThreadAnswer {
            id: thread_id(2),
            status: CommentStatus::Resolved,
            sidecar,
        })
        .unwrap()["status"],
        "resolved"
    );
    assert_eq!(
        serde_json::from_value::<AgentReply>(
            json!({"kind": "commented", "id": 4, "status": "replied"})
        )
        .unwrap(),
        AgentReply::Commented {
            id: 4,
            status: CommentStatus::Replied
        }
    );
}

/// A sidecar of `count` threads on plan.md, each Claude's question of `chars` characters, in
/// `entries` entries.
fn many_threads(count: u32, entries: usize, chars: usize) -> String {
    let mut sidecar = "---\nlectern-review: 1\nnote: plan.md\n---\n".to_owned();
    for id in 1..=count {
        sidecar.push_str(&format!("\n## C{id} · open · L5\n> the migration\n"));
        for _ in 0..entries {
            sidecar.push_str(&format!("\n**Claude (question):** {}\n", "w".repeat(chars)));
        }
    }
    sidecar
}

/// The size of `answer` as JSON.
fn bytes(answer: &impl serde::Serialize) -> usize {
    serde_json::to_vec(answer).unwrap().len()
}

/// A list too long for one answer comes in parts of whole threads, each at most 48 KiB as JSON:
/// `next` is where the next part starts, and the last part has none.
#[test]
fn a_long_list_of_threads_comes_in_parts() {
    let review = format::parse(&many_threads(40, 1, 2_000));
    let note = NoteText::of_note("plan.md", PLAN).unwrap();
    let all = || list_threads(&review, Some(&note), None);
    let first = threads_page(all(), None, None);
    assert_eq!(first.total, 40);
    let held = first.threads.len();
    assert!(held > 1 && held < 40, "{held}");
    assert_eq!(first.next, Some(u32::try_from(held).unwrap()));
    assert!(bytes(&first) <= COMMENTS_BYTES, "{}", bytes(&first));
    assert!(first.threads.iter().all(|t| !t.truncated));
    let second = threads_page(all(), first.next, None);
    assert_eq!(second.total, 40);
    assert_eq!(second.next, None);
    assert!(bytes(&second) <= COMMENTS_BYTES);
    let ids: Vec<String> = first
        .threads
        .iter()
        .chain(&second.threads)
        .map(|t| t.id.clone())
        .collect();
    let every: Vec<String> = (1..=40).map(thread_id).collect();
    assert_eq!(ids, every, "each thread once, in order");
    // Past the end: none, and nothing next.
    let past = threads_page(all(), Some(99), None);
    assert!(past.threads.is_empty());
    assert_eq!((past.total, past.next), (40, None));
    let json = serde_json::to_value(&second).unwrap();
    assert_eq!(json.get("next"), None);
    assert_eq!(json["total"], 40);
}

/// A thread too long for an answer of its own still comes, alone, with its entries cut short and
/// marked `truncated`; the next part goes on from the thread after it.
#[test]
fn a_thread_too_long_for_one_answer_comes_cut_short() {
    let review = format::parse(&many_threads(2, 5, 20_000));
    let note = NoteText::of_note("plan.md", PLAN).unwrap();
    let all = || list_threads(&review, Some(&note), None);
    let first = threads_page(all(), None, None);
    let [cut] = first.threads.as_slice() else {
        panic!("{} threads", first.threads.len());
    };
    assert_eq!(cut.id, "C1");
    assert!(cut.truncated);
    assert_eq!(cut.entries.len(), 5, "every entry, each cut short");
    assert!(cut.entries.iter().all(|e| e.text.ends_with('…')));
    assert!(bytes(&first) <= COMMENTS_BYTES, "{}", bytes(&first));
    assert_eq!(first.next, Some(1));
    assert_eq!(serde_json::to_value(cut).unwrap()["truncated"], true);
    let second = threads_page(all(), first.next, None);
    assert_eq!(second.threads[0].id, "C2");
    assert!(second.threads[0].truncated);
    assert_eq!(second.next, None);
    // A thread that fits isn't marked.
    let small = format::parse(&many_threads(1, 1, 10));
    let page = threads_page(list_threads(&small, Some(&note), None), None, None);
    assert_eq!(
        serde_json::to_value(&page.threads[0])
            .unwrap()
            .get("truncated"),
        None
    );
}

/// A passage found twice in one block, after the same 32 characters, told apart by the text after
/// it: the comment is saved on the one the target named, not the first.
#[test]
fn a_quote_told_apart_by_its_suffix_is_saved_where_it_was_found() {
    // "Once the deploy is green, we do " is 32 characters, before both "roll back"s.
    let note = "# Plan\n\n## Rollout\n\nOnce the deploy is green, we do roll back. Once the deploy is green, we do roll back again.\n";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.md");
    fs::write(&path, note).unwrap();
    let text = TextMap::of_note("plan.md", note).unwrap();
    let target = Target::Quote {
        quote: "roll back".to_owned(),
        prefix: None,
        suffix: Some("again".to_owned()),
    };
    let at = comment_anchor(&text, note, "plan.md", &target).unwrap();
    let op = comment_op(at, "Why twice?".to_owned(), "Claude", None);
    let (review, _) = store::apply_op(&sidecar_path(&path), "plan.md", note, &op, NOW).unwrap();
    let c1 = review.comments().next().unwrap();
    let anchor = c1.anchor.as_ref().unwrap();
    assert_eq!(anchor.suffix, " again.", "{anchor:?}");
    // Found there again when listed.
    let saved = format::parse(&fs::read_to_string(sidecar_path(&path)).unwrap());
    let noted = NoteText::of_note("plan.md", note).unwrap();
    let threads = list_threads(&saved, Some(&noted), None);
    assert_eq!(threads[0].state, ThreadState::Anchored);
    let second = text.text().rfind("roll back").unwrap();
    let views = lectern_core::review::view::build_payload(
        lectern_core::review::view::PayloadInput {
            note: &path,
            sidecar: &sidecar_path(&path),
            review: Some(&saved),
            resolved: &lectern_core::review::anchor::resolve_all(
                &saved,
                &noted.text,
                &noted.fingerprint,
            ),
            text: Some(&noted.text),
            read_only: None,
            note_wsl: None,
            sidecar_wsl: None,
        },
        &|_| String::new(),
    );
    assert_eq!(
        views.comments[0].text_start,
        Some(u32::try_from(second).unwrap())
    );
}

/// A hand-edited sidecar whose thread has a huge quote and heading: the answer still fits, the
/// quote and heading cut short with an ellipsis, the thread marked `truncated`.
#[test]
fn a_huge_quote_or_heading_still_fits_one_answer() {
    let sidecar = format!(
        "---\nlectern-review: 1\nnote: plan.md\n---\n\n## C1 · open · L5 · Plan › {}\n> {}\n\n**You:** Why?\n\n## C2 · open · L5\n> roll back\n\n**You:** And this?\n",
        "h".repeat(60_000),
        "q".repeat(1_100_000)
    );
    let review = format::parse(&sidecar);
    let note = NoteText::of_note("plan.md", PLAN).unwrap();
    let page = threads_page(list_threads(&review, Some(&note), None), None, None);
    assert!(bytes(&page) <= COMMENTS_BYTES, "{}", bytes(&page));
    let c1 = &page.threads[0];
    assert!(c1.truncated);
    assert!(c1.quote.ends_with('…') && c1.quote.chars().count() < 1_000);
    assert!(c1.heading_path.iter().all(|h| h.chars().count() < 1_000));
    assert!(c1.heading_path.last().unwrap().ends_with('…'));
    assert_eq!(c1.entries[0].text, "Why?", "short entries stay whole");
    // Cut short, it leaves room for the next.
    assert_eq!(page.threads[1].id, "C2");
    assert!(!page.threads[1].truncated);
    assert_eq!(page.next, None);
}
