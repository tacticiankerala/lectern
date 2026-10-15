//! Loading and saving a note's sidecar: the first comment creates it beside the note, every save
//! replaces it atomically, and a sidecar Lectern mustn't change is never written.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::thread;

use lectern_core::review::{
    anchor::{resolve_all, AnchorState},
    fingerprint, format,
    ops::{NewAnchor, ReviewOp},
    sidecar_path,
    store::{self, StoreError},
    text::{NoteText, PageBlock},
    view::{build_payload, PayloadInput},
    CommentStatus, EntryAuthor, HARD_READ_CAP, MAX_SIDECAR_BYTES,
};
use tempfile::TempDir;

const NOTE: &str = "# Tide sync\n\n## Batching\n\nThe client uploads readings in batches of at most 50.\n\n## Retries\n\nRetry with jitter after a failed upload.\n";
const NOW: &str = "2026-10-06T10:00:00Z";

/// A folder holding the note `tide.md`, and the paths of the note and its sidecar.
fn tide() -> (TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("tide.md");
    fs::write(&note, NOTE).unwrap();
    let sidecar = sidecar_path(&note);
    (dir, note, sidecar)
}

fn add_op() -> ReviewOp {
    add_saying("Why 50?")
}

fn add_saying(text: &str) -> ReviewOp {
    ReviewOp::Add {
        anchor: NewAnchor {
            start_line: 5,
            end_line: 5,
            quote: "batches of at most 50".into(),
            prefix: String::new(),
        },
        text: text.into(),
    }
}

fn reply(id: u32, text: &str) -> ReviewOp {
    ReviewOp::Reply {
        id,
        text: text.into(),
    }
}

/// The text from the first `from` up to the next `to`, or to the end.
fn between<'t>(text: &'t str, from: &str, to: Option<&str>) -> &'t str {
    let start = text.find(from).unwrap();
    let end = to
        .and_then(|to| text[start..].find(to))
        .map_or(text.len(), |len| start + len);
    &text[start..end]
}

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/review")
        .join(name);
    fs::read_to_string(path).unwrap()
}

fn append(path: &Path, text: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// `text` with its first `<!-- lectern:` block, through its `-->`, replaced by `block`.
fn with_block(text: &str, block: &str) -> String {
    let start = text.find("<!-- lectern:").unwrap();
    let end = start + text[start..].find("-->").unwrap() + "-->".len();
    format!("{}{block}{}", &text[..start], &text[end..])
}

fn temp_file(sidecar: &Path) -> PathBuf {
    let name = sidecar.file_name().unwrap().to_string_lossy();
    sidecar.with_file_name(format!(".{name}.lectern.tmp"))
}

#[test]
fn first_add_creates_the_sidecar_beside_the_note() {
    let (_dir, note, sidecar) = tide();
    let note_before = fs::read(&note).unwrap();

    let (review, id) = store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap();

    assert_eq!(id, 1);
    assert_eq!(review.comments().count(), 1);
    assert_eq!(sidecar.parent(), note.parent());
    assert_eq!(sidecar.file_name().unwrap(), "tide.review.md");
    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(
        content.starts_with("---\nlectern-review: 1\nnote: tide.md\n---\n"),
        "{content}"
    );
    assert_eq!(content.matches("\n## ").count(), 1, "{content}");
    assert!(content.contains("\n## C1 · open · "), "{content}");
    assert!(content.contains("**You:** Why 50?"), "{content}");
    assert_eq!(
        fs::read(&note).unwrap(),
        note_before,
        "the note is unchanged"
    );
    assert!(!temp_file(&sidecar).exists());
}

#[test]
fn a_stale_temp_file_from_a_crash_is_replaced() {
    let (_dir, _note, sidecar) = tide();
    let tmp = temp_file(&sidecar);
    fs::write(&tmp, "half-written by a crashed save").unwrap();

    store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap();

    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(content.contains("## C1 · open"), "{content}");
    assert!(!content.contains("half-written"), "{content}");
    assert!(!tmp.exists());
}

#[test]
fn oversized_non_utf8_and_foreign_sidecars_are_read_only_and_never_written() {
    let valid = format::serialize(&format::new_review("tide.md"));
    let mut oversized = valid.clone().into_bytes();
    oversized.extend_from_slice(b"## Summary\n\n");
    oversized.resize(usize::try_from(MAX_SIDECAR_BYTES).unwrap() + 1, b'x');
    let mut not_utf8 = valid.clone().into_bytes();
    not_utf8.extend_from_slice(b"## Summary\n\nA stray \xff byte.\n");
    let foreign = format::serialize(&format::new_review("other.md")).into_bytes();

    for (bytes, message) in [
        (
            oversized,
            "The comments file is over 2 MB, so Lectern shows it read-only.",
        ),
        (
            not_utf8,
            "The comments file isn't valid UTF-8, so Lectern won't change it.",
        ),
        (
            foreign,
            "This comments file belongs to another note (other.md).",
        ),
    ] {
        let (_dir, note, sidecar) = tide();
        let note_before = fs::read(&note).unwrap();
        fs::write(&sidecar, &bytes).unwrap();

        let err = store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap_err();

        assert!(matches!(err, StoreError::ReadOnly(_)), "{err:?}");
        assert_eq!(err.to_string(), message);
        assert!(fs::read(&sidecar).unwrap() == bytes, "{message}: rewritten");
        assert_eq!(fs::read(&note).unwrap(), note_before, "{message}");
        assert!(!temp_file(&sidecar).exists(), "{message}");
    }
}

#[test]
fn a_file_that_is_not_a_lectern_sidecar_is_read_only_and_never_written() {
    const NOT_OURS: &str = "This file isn't a Lectern comments file, so Lectern won't change it.";
    let checklist = "# Code review checklist\n\n## Correctness\n\n- Empty input is handled.\n\n## C2 rollout\n\n- Staged behind a flag.\n";
    let valid = format::serialize(&format::new_review("code.md"));
    // Cut off in the middle of its frontmatter, as an outside write caught halfway leaves it.
    let truncated = &valid[..20];
    for (what, bytes) in [
        ("an ordinary note", checklist.as_bytes()),
        ("an empty file", b"".as_slice()),
        ("a truncated sidecar", truncated.as_bytes()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("code.md");
        fs::write(&note, NOTE).unwrap();
        let sidecar = sidecar_path(&note);
        assert_eq!(sidecar.file_name().unwrap(), "code.review.md");
        fs::write(&sidecar, bytes).unwrap();

        let loaded = store::load(&sidecar, "code.md").unwrap();
        assert_eq!(loaded.read_only.as_deref(), Some(NOT_OURS), "{what}");

        let err = store::apply_op(&sidecar, "code.md", NOTE, &add_op(), NOW).unwrap_err();
        assert!(matches!(err, StoreError::ReadOnly(_)), "{what}: {err:?}");
        assert_eq!(err.to_string(), NOT_OURS, "{what}");
        assert_eq!(fs::read(&sidecar).unwrap(), bytes, "{what}: rewritten");
        assert!(!temp_file(&sidecar).exists(), "{what}");
    }
}

#[test]
fn a_save_that_would_pass_the_size_cap_is_refused_and_writes_nothing() {
    let (_dir, _note, sidecar) = tide();
    let mut near_cap = format::serialize(&format::new_review("tide.md")).into_bytes();
    near_cap.extend_from_slice(b"## Summary\n\n");
    near_cap.resize(usize::try_from(MAX_SIDECAR_BYTES).unwrap() - 100, b'x');
    near_cap.push(b'\n');
    fs::write(&sidecar, &near_cap).unwrap();

    // Not `unwrap_err`: that would print the whole 2 MB review.
    let Err(err) = store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW) else {
        panic!("saved a sidecar past the cap");
    };

    assert!(matches!(err, StoreError::ReadOnly(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "Couldn't save the comment: the comments file would grow past 2 MB."
    );
    assert!(
        fs::read(&sidecar).unwrap() == near_cap,
        "the file is unchanged"
    );
    assert!(!temp_file(&sidecar).exists());
}

#[test]
fn load_reads_a_sidecar_and_says_when_it_is_read_only() {
    let (_dir, _note, sidecar) = tide();
    let missing = store::load(&sidecar, "tide.md").unwrap();
    assert!(missing.review.is_none() && missing.read_only.is_none());

    store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap();
    let loaded = store::load(&sidecar, "tide.md").unwrap();
    assert_eq!(loaded.review.unwrap().comments().count(), 1);
    assert_eq!(loaded.read_only, None);

    let foreign = store::load(&sidecar, "other.md").unwrap();
    assert_eq!(foreign.review.unwrap().comments().count(), 1);
    assert_eq!(
        foreign.read_only.as_deref(),
        Some("This comments file belongs to another note (tide.md).")
    );

    let mut not_utf8 = fs::read(&sidecar).unwrap();
    not_utf8.extend_from_slice(b"\n## Summary\n\nA stray \xff byte.\n");
    fs::write(&sidecar, &not_utf8).unwrap();
    let lossy = store::load(&sidecar, "tide.md").unwrap();
    let review = lossy.review.unwrap();
    assert_eq!(review.comments().count(), 1, "read lossily, not refused");
    assert_eq!(
        lossy.read_only.as_deref(),
        Some("The comments file isn't valid UTF-8, so Lectern won't change it.")
    );

    let mut oversized = format::serialize(&format::new_review("tide.md")).into_bytes();
    oversized.extend_from_slice(b"## Summary\n\n");
    oversized.resize(usize::try_from(MAX_SIDECAR_BYTES).unwrap() + 1, b'x');
    fs::write(&sidecar, &oversized).unwrap();
    let big = store::load(&sidecar, "tide.md").unwrap();
    assert!(big.review.is_some(), "still shown");
    assert_eq!(
        big.read_only.as_deref(),
        Some("The comments file is over 2 MB, so Lectern shows it read-only.")
    );

    fs::File::create(&sidecar)
        .unwrap()
        .set_len(HARD_READ_CAP + 1)
        .unwrap();
    let huge = store::load(&sidecar, "tide.md").unwrap();
    assert!(huge.review.is_none(), "not read at all");
    assert_eq!(
        huge.read_only.as_deref(),
        Some("The comments file is over 16 MB, so Lectern won't open it.")
    );
}

#[test]
fn overlapping_saves_to_one_sidecar_keep_both_comments() {
    let texts = ["From the north station.", "From the south station."];
    for round in 0..20 {
        let (_dir, _note, sidecar) = tide();
        let barrier = Barrier::new(texts.len());
        let (sidecar, barrier) = (&sidecar, &barrier);
        let results: Vec<_> = thread::scope(|s| {
            let saves: Vec<_> = texts
                .iter()
                .map(|text| {
                    s.spawn(move || {
                        barrier.wait();
                        store::apply_op(sidecar, "tide.md", NOTE, &add_saying(text), NOW)
                    })
                })
                .collect();
            saves.into_iter().map(|save| save.join().unwrap()).collect()
        });

        for result in &results {
            assert!(result.is_ok(), "round {round}: {result:?}");
        }
        let content = fs::read_to_string(sidecar).unwrap();
        for text in texts {
            assert_eq!(
                content.matches(text).count(),
                1,
                "round {round}:\n{content}"
            );
        }
        assert!(!temp_file(sidecar).exists(), "round {round}");
    }
}

#[test]
fn a_note_name_in_another_case_still_owns_the_sidecar() {
    let (_dir, _note, sidecar) = tide();
    fs::write(&sidecar, format::serialize(&format::new_review("Tide.md"))).unwrap();

    let loaded = store::load(&sidecar, "tide.md").unwrap();
    assert!(loaded.review.is_some());
    assert_eq!(loaded.read_only, None, "Windows file names ignore case");

    let (_, id) = store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap();
    assert_eq!(id, 1);
    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(content.contains("## C1 · open"), "{content}");
}

#[cfg(unix)]
#[test]
fn read_only_folder_reports_io_and_leaves_no_temp() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, note, sidecar) = tide();
    let note_before = fs::read(&note).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    // Root, or anyone else who can write to a read-only folder, can't run this test.
    let probe = dir.path().join("probe");
    if fs::write(&probe, "").is_ok() {
        fs::remove_file(&probe).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: the read-only folder is writable here (running as root?)");
        return;
    }

    let result = store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.unwrap_err();
    assert!(matches!(err, StoreError::Io(_)), "{err:?}");
    assert!(
        err.to_string().starts_with("Couldn't save the comment: "),
        "{err}"
    );
    assert!(!sidecar.exists());
    assert!(!temp_file(&sidecar).exists());
    assert_eq!(fs::read(&note).unwrap(), note_before);
}

#[test]
fn untouched_comments_survive_byte_for_byte_through_an_op() {
    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/review/claude-edited.review.md");
    let before = fs::read_to_string(golden).unwrap();
    let dir = tempfile::tempdir().unwrap();
    // The note the golden's fingerprints were taken from, so every comment stays where it is.
    let source = "# Plan\n";
    let note = dir.path().join("2026-03-02-tide-sync.md");
    fs::write(&note, source).unwrap();
    let sidecar = sidecar_path(&note);
    fs::write(&sidecar, &before).unwrap();

    let (_, id) = store::apply_op(
        &sidecar,
        "2026-03-02-tide-sync.md",
        source,
        &reply(2, "Thanks, that reads well."),
        NOW,
    )
    .unwrap();

    assert_eq!(id, 2);
    let after = fs::read_to_string(&sidecar).unwrap();
    // The instructions block is refreshed (see `instructions_block_is_refreshed_on_save`).
    assert_eq!(
        between(&after, "---", Some("<!-- lectern:")),
        between(&before, "---", Some("<!-- lectern:")),
        "preamble"
    );
    assert_eq!(
        between(&after, "## C1", Some("## C2")),
        between(&before, "## C1", Some("## C2")),
        "C1"
    );
    assert_eq!(
        between(&after, "## Summary", None),
        between(&before, "## Summary", None),
        "summary"
    );
    assert!(
        after.contains("**You:** Thanks, that reads well."),
        "{after}"
    );
}

#[test]
fn agent_started_comment_parses_and_continues() {
    let golden = golden("agent-started.review.md");
    let r = format::parse(&golden);
    let c3 = r.comments().find(|c| c.id == 3).unwrap();
    assert!(c3.anchor.is_none(), "the agent wrote no anchor line");
    assert_eq!(c3.quote, "after a failed upload");
    let first = &c3.entries[0];
    assert_eq!(
        (first.author, first.name.as_str()),
        (EntryAuthor::Agent, "Claude")
    );
    assert_eq!(c3.effective_status(), CommentStatus::Question);
    assert_eq!(r.next_id(), 4);

    // You reply: the thread goes on, open, and the save anchors it.
    let (_dir, _note, sidecar) = tide();
    fs::write(&sidecar, &golden).unwrap();
    let now = "2026-10-07T09:30:00Z";
    let (saved, id) = store::apply_op(
        &sidecar,
        "tide.md",
        NOTE,
        &reply(3, "Only after a timeout."),
        now,
    )
    .unwrap();
    assert_eq!(id, 3);
    let c3 = saved.comments().find(|c| c.id == 3).unwrap();
    assert_eq!(c3.effective_status(), CommentStatus::Open);
    let anchor = c3.anchor.as_ref().unwrap();
    assert_eq!(anchor.n, 2);
    assert_eq!(
        anchor.created, now,
        "the save that adds the anchor line dates it"
    );
    let after = fs::read_to_string(&sidecar).unwrap();
    assert_eq!(
        between(&after, "---", Some("## C3")),
        between(&golden, "---", Some("## C3")),
        "the preamble, C1 and C2 are untouched"
    );
    let c3_text = between(&after, "## C3", None);
    assert!(
        c3_text.starts_with(
            "## C3 · open · L9 · Tide sync › Retries\n\
             <!-- anchor prefix=\"t 50. Retries Retry with jitter \" suffix=\".\" \
             fp=\"fnv1a64:592e4f21891a6582\" n=2 created=\"2026-10-07T09:30:00Z\" -->\n"
        ),
        "{c3_text}"
    );
    assert!(
        c3_text.ends_with(
            "**Claude (question):** After every failed upload, or only after a timeout? \
             A rejected batch will fail again however long it waits.\n\n\
             **You:** Only after a timeout.\n"
        ),
        "{c3_text}"
    );

    // A later agent entry sets the status again.
    append(
        &sidecar,
        "\n**Codex (pushback):** A timeout can hide a rejected batch too.\n",
    );
    let loaded = store::load(&sidecar, "tide.md").unwrap();
    assert_eq!(loaded.read_only, None);
    let review = loaded.review.unwrap();
    let c3 = review.comments().find(|c| c.id == 3).unwrap();
    let last = c3.entries.last().unwrap();
    assert_eq!(
        (last.author, last.name.as_str()),
        (EntryAuthor::Agent, "Codex")
    );
    assert_eq!(c3.entries.len(), 3);
    assert_eq!(c3.effective_status(), CommentStatus::Pushback);
    assert_eq!(review.next_id(), 4);
}

#[test]
fn instructions_block_is_refreshed_on_save() {
    let fresh = format::INSTRUCTIONS.trim_end_matches('\n');
    // A v0.2.0 sidecar: the old block, here with a line of the owner's own after it.
    let before = golden("claude-edited.review.md").replacen(
        "-->\n\n## C1",
        "-->\n\nKeep this line.\n\n## C1",
        1,
    );
    assert!(!before.contains(fresh));
    assert_eq!(
        format::serialize(&format::parse(&before)),
        before,
        "reading and writing alone change nothing"
    );
    let dir = tempfile::tempdir().unwrap();
    // The note the golden's fingerprints were taken from, so every comment stays where it is.
    let source = "# Plan\n";
    let note = dir.path().join("2026-03-02-tide-sync.md");
    fs::write(&note, source).unwrap();
    let sidecar = sidecar_path(&note);
    let name = "2026-03-02-tide-sync.md";
    fs::write(&sidecar, &before).unwrap();

    let (saved, _) = store::apply_op(&sidecar, name, source, &reply(2, "Thanks."), NOW).unwrap();

    let after = fs::read_to_string(&sidecar).unwrap();
    let preamble = with_block(between(&before, "---", Some("## C1")), fresh);
    assert_eq!(between(&after, "---", Some("## C1")), preamble);
    assert!(
        preamble.contains("-->\n\nKeep this line.\n\n"),
        "{preamble}"
    );
    assert_eq!(
        saved.preamble, preamble,
        "the review returned is the one saved"
    );
    assert_eq!(
        between(&after, "## C1", Some("## C2")),
        between(&before, "## C1", Some("## C2")),
        "C1"
    );

    // In a CRLF file the new block takes CRLF.
    let crlf = golden("claude-variants-crlf.review.md");
    fs::write(&sidecar, &crlf).unwrap();
    store::apply_op(&sidecar, name, source, &reply(1, "Thanks."), NOW).unwrap();
    let after = fs::read_to_string(&sidecar).unwrap();
    assert!(after.starts_with('\u{feff}'));
    assert!(after.contains(&fresh.replace('\n', "\r\n")), "{after}");
    assert!(!after.replace("\r\n", "").contains('\n'), "{after:?}");
    assert_eq!(
        between(&after, "---", Some("## C1")),
        with_block(
            between(&crlf, "---", Some("## C1")),
            &fresh.replace('\n', "\r\n")
        )
    );

    // A sidecar without a Lectern block gets none.
    let plain =
        "---\nlectern-review: 1\nnote: tide.md\n---\n# My review notes\n\n<!-- my own note -->\n\n";
    let (_dir, _note, sidecar) = tide();
    fs::write(&sidecar, plain).unwrap();
    store::apply_op(&sidecar, "tide.md", NOTE, &add_op(), NOW).unwrap();
    let after = fs::read_to_string(&sidecar).unwrap();
    assert!(after.starts_with(plain), "{after}");
    assert!(!after.contains("<!-- lectern:"), "{after}");
}

/// A text file's comments anchor against its paragraphs, where a `#` line is text, not a heading.
#[test]
fn a_text_files_comment_anchors_against_its_paragraphs() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("tide.txt");
    let source = "# Tide sync\n\nThe client uploads readings\nin batches of at most 50.\n";
    fs::write(&note, source).unwrap();
    let sidecar = sidecar_path(&note);
    assert_eq!(sidecar.file_name().unwrap(), "tide.txt.review.md");
    let op = ReviewOp::Add {
        anchor: NewAnchor {
            start_line: 1,
            end_line: 1,
            quote: "# Tide sync".into(),
            prefix: String::new(),
        },
        text: "Rename it?".into(),
    };

    let (review, _) = store::apply_op(&sidecar, "tide.txt", source, &op, NOW).unwrap();

    let comment = review.comments().next().unwrap();
    assert_eq!((comment.start_line, comment.end_line), (1, 1));
    assert!(comment.heading_path.is_empty());
    let anchor = comment.anchor.as_ref().unwrap();
    assert_eq!(anchor.fp, fingerprint(source), "the quote was found");
    assert_eq!(anchor.suffix, " The client uploads readings in ");
    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(content.contains("note: tide.txt\n"), "{content}");
    assert!(content.contains("\n> # Tide sync\n"), "{content}");
}

/// An HTML page's comments go on the text its bridge reported (`apply_op_on`): its source alone is
/// refused, before anything is written, rather than read as a note.
#[test]
fn a_page_without_the_text_its_bridge_reported_takes_no_comment() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("report.html");
    fs::write(&page, PAGE).unwrap();
    let sidecar = sidecar_path(&page);
    for name in ["report.html", "REPORT.HTM"] {
        let Err(err) = store::apply_op(&sidecar, name, PAGE, &add_op(), NOW) else {
            panic!("{name}: a page took a comment on its source");
        };
        assert!(matches!(err, StoreError::NoPageText), "{name}: {err:?}");
    }
    assert!(!sidecar.exists());
    assert!(NoteText::of_note("report.html", PAGE).is_none());
}

/// An invented page, its sidecar's path in a temporary folder, and the page's text as its bridge
/// reports it: a heading on line 4, and the same phrase on lines 6 and 8.
const PAGE: &str = "<!doctype html>\n<title>Weekly report</title>\n<body>\n<h1>Weekly report</h1>\n<h2>Results</h2>\n<p>Sign-ups rose, but conversion dipped in week 3.</p>\n<h2>Next steps</h2>\n<p>Ask why conversion dipped in week 3.</p>\n";

fn report_page() -> (TempDir, PathBuf, NoteText) {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("report.html");
    fs::write(&page, PAGE).unwrap();
    let sidecar = sidecar_path(&page);
    assert_eq!(sidecar.file_name().unwrap(), "report.html.review.md");
    let block = |text: &str, level: Option<u8>| PageBlock {
        text: text.into(),
        heading_level: level,
    };
    let blocks = [
        block("Weekly report", Some(1)),
        block("Results", Some(2)),
        block("Sign-ups rose, but conversion dipped in week 3.", None),
        block("Next steps", Some(2)),
        block("Ask why conversion dipped in week 3.", None),
    ];
    (dir, sidecar, NoteText::of_page(&blocks, PAGE))
}

/// A comment on `quote` selected in a page, as the UI sends it: no lines, and the text before it.
fn add_on_page(quote: &str, prefix: &str) -> ReviewOp {
    ReviewOp::Add {
        anchor: NewAnchor {
            start_line: 0,
            end_line: 0,
            quote: quote.into(),
            prefix: prefix.into(),
        },
        text: "Weekly, not daily.".into(),
    }
}

/// An HTML page's comments anchor against the text its bridge reported: a selection carries no
/// lines, so the text before it tells a phrase found twice apart, and the lines are the page
/// source's, where each block was found.
#[test]
fn a_pages_comment_anchors_against_the_text_its_bridge_reported() {
    let (dir, sidecar, text) = report_page();
    let page = dir.path().join("report.html");
    let source = PAGE;
    let add = |prefix: &str| add_on_page("conversion dipped in week 3", prefix);

    let fp = text.fingerprint.clone();
    let save = |op: &ReviewOp| store::apply_op_on(&sidecar, "report.html", &text, op, NOW);
    let (review, _) = save(&add("Results Sign-ups rose, but ")).unwrap();
    let (review2, _) = save(&add("Next steps Ask why ")).unwrap();

    let first = review.comments().next().unwrap();
    assert_eq!((first.start_line, first.end_line), (6, 6));
    assert_eq!(first.heading_path, ["Weekly report", "Results"]);
    assert_eq!(first.anchor.as_ref().unwrap().fp, fp, "the quote was found");
    let second = review2.comments().nth(1).unwrap();
    assert_eq!((second.start_line, second.end_line), (8, 8));
    assert_eq!(second.heading_path, ["Weekly report", "Next steps"]);
    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(content.contains("note: report.html\n"), "{content}");
    assert!(
        content.contains("\n## C1 · open · L6 · Weekly report › Results\n"),
        "{content}"
    );
    assert!(
        content.contains("\n> conversion dipped in week 3\n"),
        "{content}"
    );
    assert!(
        content.contains("\n## C2 · open · L8 · Weekly report › Next steps\n"),
        "{content}"
    );
    assert_eq!(
        fs::read_to_string(&page).unwrap(),
        source,
        "the page changed"
    );
}

/// A page's selection carries no lines, so a quote that isn't in the page's text (the page changed
/// under the reader) is pinned as a detached comment is: to the deepest heading of its path still
/// there, else to the page's first block, else to line 1. Never to line 0.
#[test]
fn a_page_comment_whose_quote_isnt_there_is_never_on_line_0() {
    let (_dir, sidecar, text) = report_page();
    let save = |op: &ReviewOp| store::apply_op_on(&sidecar, "report.html", &text, op, NOW);
    save(&add_on_page("words the page no longer has", "")).unwrap();
    save(&add_on_page("Ask why", "Next steps ")).unwrap();
    let reattach = ReviewOp::Reattach {
        id: 2,
        anchor: NewAnchor {
            start_line: 0,
            end_line: 0,
            quote: "gone from the page".into(),
            prefix: String::new(),
        },
    };
    let (review, _) = save(&reattach).unwrap();

    let content = fs::read_to_string(&sidecar).unwrap();
    assert!(
        content.contains("\n## C1 · open · L4 · Weekly report\n"),
        "{content}"
    );
    assert!(
        content.contains("\n## C2 · open · L7 · Weekly report › Next steps\n"),
        "{content}"
    );
    assert!(!content.contains(" · L0"), "{content}");
    // Not found, so never trusted: looked for by their quote again on every load.
    for c in review.comments() {
        assert_eq!(c.anchor.as_ref().unwrap().fp, "", "C{}", c.id);
    }

    // A page with no text at all: line 1.
    let empty = NoteText::of_page(&[], PAGE);
    let (review, _) =
        store::apply_op_on(&sidecar, "report.html", &empty, &add_on_page("x", ""), NOW).unwrap();
    let c3 = review.comments().nth(2).unwrap();
    assert_eq!((c3.start_line, c3.end_line), (1, 1));
}

/// A minified page has its headings on one line, so a page's heading paths go by the order of its
/// blocks, not by source lines: a paragraph is under the heading before it.
#[test]
fn a_minified_pages_headings_go_by_block_order() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("min.html");
    let source = "<body>\n<h2>Results</h2><p>First paragraph here.</p><h2>Next steps</h2><p>Second paragraph here.</p>\n";
    fs::write(&page, source).unwrap();
    let sidecar = sidecar_path(&page);
    let block = |text: &str, level: Option<u8>| PageBlock {
        text: text.into(),
        heading_level: level,
    };
    let blocks = [
        block("Results", Some(2)),
        block("First paragraph here.", None),
        block("Next steps", Some(2)),
        block("Second paragraph here.", None),
    ];
    let note = NoteText::of_page(&blocks, source);
    let save = |quote: &str| {
        store::apply_op_on(&sidecar, "min.html", &note, &add_on_page(quote, ""), NOW).unwrap()
    };
    save("First paragraph");
    let (review, _) = save("Second paragraph");
    let paths: Vec<(u32, Vec<String>)> = review
        .comments()
        .map(|c| (c.start_line, c.heading_path.clone()))
        .collect();
    assert_eq!(
        paths,
        [
            (2, vec!["Results".to_owned()]),
            (2, vec!["Next steps".to_owned()])
        ]
    );
    // As the payload shows them, found in the page again.
    let resolved = resolve_all(&review, &note.text, &note.fingerprint);
    let payload = build_payload(
        PayloadInput {
            note: &page,
            sidecar: &sidecar,
            review: Some(&review),
            resolved: &resolved,
            text: Some(&note.text),
            read_only: None,
            note_wsl: None,
            sidecar_wsl: None,
        },
        &|md: &str| md.to_owned(),
    );
    let shown: Vec<&[String]> = payload
        .comments
        .iter()
        .map(|c| c.heading_path.as_slice())
        .collect();
    assert_eq!(shown, [["Results"], ["Next steps"]]);
}

/// A page's comments record the fingerprint of the text its bridge reported as well as of its file:
/// the same HTML shows other text when a script writes something else or scripts are off, so a
/// quote gone from what it shows is looked for, not kept where it was.
#[test]
fn a_pages_comments_record_the_fingerprint_of_its_text() {
    let (_dir, sidecar, page) = report_page();
    let (review, _) = store::apply_op_on(
        &sidecar,
        "report.html",
        &page,
        &add_on_page("Ask why", "Next steps "),
        NOW,
    )
    .unwrap();
    let fp = &review
        .comments()
        .next()
        .unwrap()
        .anchor
        .as_ref()
        .unwrap()
        .fp;
    // The text it showed, then its HTML.
    assert_eq!(fp, &fingerprint(&format!("{}\0{PAGE}", page.text.text())));
    assert_eq!(
        resolve_all(&review, &page.text, &page.fingerprint)[0].state,
        AnchorState::Anchored
    );
    // The same file, showing only its heading.
    let heading = PageBlock {
        text: "Weekly report".into(),
        heading_level: Some(1),
    };
    let other = NoteText::of_page(&[heading], PAGE);
    assert_eq!(
        resolve_all(&review, &other.text, &other.fingerprint)[0].state,
        AnchorState::Detached
    );
}

/// The blocks of a page, as its bridge reports them: `(text, heading level)`.
fn blocks_of(blocks: &[(&str, Option<u8>)]) -> Vec<PageBlock> {
    blocks
        .iter()
        .map(|&(text, heading_level)| PageBlock {
            text: text.into(),
            heading_level,
        })
        .collect()
}

/// A page's fingerprint covers its HTML as well as its text: an edit that moves its lines but not
/// its text has its comments found again by their quote and its context, not left on lines that
/// now hold another copy of their quote.
#[test]
fn a_page_edited_without_changing_its_text_finds_its_comments_again() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = sidecar_path(&dir.path().join("twice.html"));
    let source = "<body>\n<h1>Report</h1>\n<p>Conversion dipped in week 3.</p>\n<p>Conversion dipped in week 3.</p>\n<p>The end.</p>\n";
    let blocks = blocks_of(&[
        ("Report", Some(1)),
        ("Conversion dipped in week 3.", None),
        ("Conversion dipped in week 3.", None),
        ("The end.", None),
    ]);
    let note = NoteText::of_page(&blocks, source);
    let add = add_on_page(
        "Conversion dipped in week 3.",
        "Report Conversion dipped in week 3. ",
    );
    let (review, _) = store::apply_op_on(&sidecar, "twice.html", &note, &add, NOW).unwrap();
    let c1 = review.comments().next().unwrap();
    assert_eq!((c1.start_line, c1.end_line), (4, 4), "on the second copy");

    // A blank line above both copies: the page shows the same text, its lines have moved.
    let moved_source = source.replace("<h1>Report</h1>\n", "<h1>Report</h1>\n\n");
    let moved = NoteText::of_page(&blocks, &moved_source);
    let resolved = resolve_all(&review, &moved.text, &moved.fingerprint);
    let second = moved.text.text().rfind("Conversion dipped").unwrap();
    assert_eq!(resolved[0].state, AnchorState::Anchored);
    assert_eq!((resolved[0].start_line, resolved[0].end_line), (5, 5));
    assert_eq!(resolved[0].span.map(|(s, _)| s), Some(second));
}

/// A detached page comment is pinned to the deepest heading of its path still on the page, and
/// the payload says which one by where it starts: a page can repeat a heading.
#[test]
fn a_detached_page_comment_is_pinned_to_its_own_heading_of_several() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("twice.html");
    let sidecar = sidecar_path(&page);
    let source = "<body>\n<h1>First</h1>\n<h2>Results</h2>\n<p>Early results here.</p>\n<h1>Second</h1>\n<h2>Results</h2>\n<p>Late figures came in.</p>\n";
    let headings = [
        ("First", Some(1)),
        ("Results", Some(2)),
        ("Early results here.", None),
        ("Second", Some(1)),
        ("Results", Some(2)),
    ];
    let mut blocks = blocks_of(&headings);
    blocks.extend(blocks_of(&[("Late figures came in.", None)]));
    let note = NoteText::of_page(&blocks, source);
    let (review, _) = store::apply_op_on(
        &sidecar,
        "twice.html",
        &note,
        &add_on_page("Late figures came in", "Second Results "),
        NOW,
    )
    .unwrap();
    assert_eq!(
        review.comments().next().unwrap().heading_path,
        ["Second", "Results"]
    );

    // The paragraph is gone: the comment is detached, pinned to the second Results.
    let gone = NoteText::of_page(
        &blocks_of(&headings),
        &source.replace("<p>Late figures came in.</p>\n", ""),
    );
    let resolved = resolve_all(&review, &gone.text, &gone.fingerprint);
    let payload = build_payload(
        PayloadInput {
            note: &page,
            sidecar: &sidecar,
            review: Some(&review),
            resolved: &resolved,
            text: Some(&gone.text),
            read_only: None,
            note_wsl: None,
            sidecar_wsl: None,
        },
        &|md: &str| md.to_owned(),
    );
    let c1 = &payload.comments[0];
    assert_eq!(c1.state, AnchorState::Detached);
    assert_eq!(c1.pinned_heading.as_deref(), Some("Results"));
    let second = gone.text.text().rfind("Results").unwrap();
    assert_eq!(c1.pinned_at, Some(u32::try_from(second).unwrap()));
}
