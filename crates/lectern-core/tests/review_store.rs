//! Loading and saving a note's sidecar: the first comment creates it beside the note, every save
//! replaces it atomically, and a sidecar Lectern mustn't change is never written.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::thread;

use lectern_core::review::{
    format,
    ops::{NewAnchor, ReviewOp},
    sidecar_path,
    store::{self, StoreError},
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
