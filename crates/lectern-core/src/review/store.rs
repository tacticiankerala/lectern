//! Loading a note's sidecar, and saving a change to it without losing anyone else's.
//!
//! A save reads the sidecar, applies the change, and writes the result to a temporary file beside
//! it. Just before renaming that over the sidecar it reads the sidecar again: if the bytes differ,
//! someone (Claude) wrote to it meanwhile, and the save starts over from what they wrote. Changes
//! are applied by comment id, so both are kept.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use super::anchor::{refresh_anchored, resolve_all};
use super::format::{new_review, parse, refresh_instructions, serialize};
use super::ops::{self, OpContext, OpError, ReviewOp};
use super::text::TextMap;
use super::{fingerprint, Item, Review, HARD_READ_CAP, MAX_SIDECAR_BYTES};

/// How many times a save starts over because the sidecar changed under it.
pub(crate) const WRITE_RETRIES: usize = 3;
/// How many times a rename refused for a moment (see [`transient`]) is tried again.
const RENAME_RETRIES: u32 = 3;
const RENAME_PAUSE: Duration = Duration::from_millis(50);
/// Saves in this process take turns, as every save of a sidecar uses the same temporary file.
/// Lectern runs one instance, so no other Lectern process writes the sidecar.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

const NOT_A_SIDECAR: &str = "This file isn't a Lectern comments file, so Lectern won't change it.";
const OVER_READ_ONLY_CAP: &str = "The comments file is over 2 MB, so Lectern shows it read-only.";
const NOT_UTF8: &str = "The comments file isn't valid UTF-8, so Lectern won't change it.";
const OVER_HARD_CAP: &str = "The comments file is over 16 MB, so Lectern won't open it.";
const WOULD_PASS_CAP: &str = "Couldn't save the comment: the comments file would grow past 2 MB.";

/// A sidecar loaded to be shown.
#[derive(Debug)]
pub struct Loaded {
    /// `None` when there's no sidecar, or it's too large to read.
    pub review: Option<Review>,
    /// Why the sidecar is shown read-only, or not at all, if it is.
    pub read_only: Option<String>,
}

/// Reads the sidecar at `sidecar` of the note whose file name is `note_name`.
///
/// No sidecar gives neither a review nor a message. One larger than [`HARD_READ_CAP`] isn't read:
/// no review, and a message. One without a sidecar's frontmatter (an ordinary note named like a
/// sidecar, or an empty or truncated file), larger than [`MAX_SIDECAR_BYTES`], not UTF-8 (it's
/// read lossily), or whose `note:` names another note is read, with a message saying why Lectern
/// won't change it.
pub fn load(sidecar: &Path, note_name: &str) -> io::Result<Loaded> {
    Ok(match read_capped(sidecar, HARD_READ_CAP)? {
        Contents::Missing => Loaded {
            review: None,
            read_only: None,
        },
        Contents::Over => Loaded {
            review: None,
            read_only: Some(OVER_HARD_CAP.to_owned()),
        },
        Contents::Bytes(bytes) => {
            let (review, read_only) = read_review(&bytes, note_name);
            Loaded {
                review: Some(review),
                read_only,
            }
        }
    })
}

/// Why a change wasn't saved. Its text is shown to the reader as it is.
#[derive(Debug)]
pub enum StoreError {
    /// The operation was refused.
    Op(OpError),
    /// The sidecar kept changing while Lectern tried to save.
    Conflict,
    /// Lectern won't change this sidecar; the text says why.
    ReadOnly(String),
    Io(io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Op(e) => e.fmt(f),
            Self::Conflict => f.write_str("Couldn't save the comment: the file kept changing."),
            Self::ReadOnly(message) => f.write_str(message),
            Self::Io(e) => write!(f, "Couldn't save the comment: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Applies `op` to the sidecar at `sidecar` and saves it, creating the sidecar if there's none: a
/// file that is already there is changed only when [`load`] would show it writable.
/// `source` is the note's text and `now` the time to record on a new comment.
///
/// The sidecar is read, `op` applied, anchor lines it gained dated `now`, the instructions block
/// brought up to date (see [`refresh_instructions`]), and the result written to a temporary file
/// that replaces the sidecar only if the sidecar's bytes are still the ones read, checked before
/// every rename attempt; otherwise it starts over, up to 3 times. Saves in this process take
/// turns. A save that would take the sidecar past [`MAX_SIDECAR_BYTES`] is refused before anything
/// is written.
/// The temporary file is removed again whenever the save fails.
/// Returns the review as saved and the id of the comment `op` changed.
pub fn apply_op(
    sidecar: &Path,
    note_name: &str,
    source: &str,
    op: &ReviewOp,
    now: &str,
) -> Result<(Review, u32), StoreError> {
    apply_op_with_hooks(
        sidecar,
        note_name,
        source,
        op,
        now,
        &mut || {},
        &mut |from, to| fs::rename(from, to),
    )
}

/// [`apply_op`] with the places the tests step in: `before_rename` is called once the temporary
/// file is written and before the sidecar is read again, and `rename` replaces the sidecar.
pub(crate) fn apply_op_with_hooks(
    sidecar: &Path,
    note_name: &str,
    source: &str,
    op: &ReviewOp,
    now: &str,
    before_rename: &mut dyn FnMut(),
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(Review, u32), StoreError> {
    // The lock guards no data, so a save that panicked holding it left nothing this one needs.
    let _turn = SAVE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let text = TextMap::build(source);
    let fp = fingerprint(source);
    let ctx = OpContext {
        text: &text,
        fingerprint: &fp,
        now,
    };
    let tmp = temp_path(sidecar);
    for _ in 0..=WRITE_RETRIES {
        let before = read_capped(sidecar, MAX_SIDECAR_BYTES).map_err(StoreError::Io)?;
        let mut review = match &before {
            Contents::Missing => new_review(note_name),
            Contents::Over => return Err(StoreError::ReadOnly(OVER_READ_ONLY_CAP.to_owned())),
            Contents::Bytes(bytes) => match read_review(bytes, note_name) {
                (review, None) => review,
                (_, Some(reason)) => return Err(StoreError::ReadOnly(reason)),
            },
        };
        let resolved = resolve_all(&review, &text, &fp);
        refresh_anchored(&mut review, &resolved, &text, &fp);
        let id = ops::apply(&mut review, op, &ctx).map_err(StoreError::Op)?;
        date_new_anchors(&mut review, now);
        refresh_instructions(&mut review);
        let out = serialize(&review);
        // Saved past the cap, the sidecar would turn read-only and refuse every later change.
        if u64::try_from(out.len()).unwrap_or(u64::MAX) > MAX_SIDECAR_BYTES {
            return Err(StoreError::ReadOnly(WOULD_PASS_CAP.to_owned()));
        }

        match fs::remove_file(&tmp) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(StoreError::Io(e)),
            _ => {}
        }
        write_new(&tmp, out.as_bytes()).map_err(StoreError::Io)?;
        before_rename();
        match replace_if_unchanged(&tmp, sidecar, &before, rename) {
            Ok(true) => {
                log::info!("saved review comment C{id} to {}", sidecar.display());
                return Ok((review, id));
            }
            // Written to meanwhile: start over from what's there now.
            Ok(false) => {
                let _ = fs::remove_file(&tmp);
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(StoreError::Io(e));
            }
        }
    }
    Err(StoreError::Conflict)
}

/// Gives `now` as the creation time to every anchor line without one: those this save added to
/// comments that had none, such as a comment an agent started.
fn date_new_anchors(review: &mut Review, now: &str) {
    for item in &mut review.items {
        if let Item::Comment(c) = item {
            if let Some(anchor) = c.anchor.as_mut().filter(|a| a.created.is_empty()) {
                anchor.created = now.to_owned();
                c.dirty = true;
            }
        }
    }
}

/// What reading a sidecar found.
#[derive(PartialEq)]
enum Contents {
    Missing,
    /// Larger than the reader's cap, so not kept.
    Over,
    Bytes(Vec<u8>),
}

/// Reads the file at `path` unless it's larger than `cap` bytes. Never reads more than `cap + 1`
/// bytes, even from a file that grows while it's read.
fn read_capped(path: &Path, cap: u64) -> io::Result<Contents> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Contents::Missing),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() > cap {
        return Ok(Contents::Over);
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok(if len > cap {
        Contents::Over
    } else {
        Contents::Bytes(bytes)
    })
}

/// Parses a sidecar, lossily if it isn't UTF-8, and says why Lectern mustn't change it, if it
/// mustn't: it has no sidecar frontmatter (`lectern-review:` and `note:`), so it isn't one Lectern
/// made; it's over [`MAX_SIDECAR_BYTES`]; it isn't UTF-8; or it names a note other than
/// `note_name` (ignoring case, as Windows file names do).
fn read_review(bytes: &[u8], note_name: &str) -> (Review, Option<String>) {
    let (text, utf8) = match std::str::from_utf8(bytes) {
        Ok(text) => (Cow::Borrowed(text), true),
        Err(_) => (String::from_utf8_lossy(bytes), false),
    };
    let review = parse(&text);
    let over = u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SIDECAR_BYTES;
    let reason = if review.note.is_empty() {
        Some(NOT_A_SIDECAR.to_owned())
    } else if over {
        Some(OVER_READ_ONLY_CAP.to_owned())
    } else if !utf8 {
        Some(NOT_UTF8.to_owned())
    } else if review.note.to_lowercase() != note_name.to_lowercase() {
        Some(format!(
            "This comments file belongs to another note ({}).",
            review.note
        ))
    } else {
        None
    };
    (review, reason)
}

/// `.<sidecar name>.lectern.tmp`, beside the sidecar.
fn temp_path(sidecar: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(sidecar.file_name().unwrap_or_default());
    name.push(".lectern.tmp");
    sidecar.with_file_name(name)
}

/// Writes `bytes` to a new file at `path` and flushes it to disk. If the write fails, the file is
/// removed again.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        // Windows won't remove a file that is still open.
        drop(file);
        let _ = fs::remove_file(path);
    }
    written
}

/// Renames `tmp` over `sidecar` if the sidecar still holds `before`, and says whether it did. A
/// rename refused for a moment (see [`transient`]) is tried again, the sidecar checked again
/// first: whoever held it open may have written to it.
fn replace_if_unchanged(
    tmp: &Path,
    sidecar: &Path,
    before: &Contents,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<bool> {
    let mut retries = 0;
    loop {
        // A write landing between this check and the rename is still lost. Closing that window
        // would take a lock every writer honours, and outside writers (Claude, whose own edits are
        // read-modify-write too) take none, so it's accepted, as the design accepts it.
        if read_capped(sidecar, MAX_SIDECAR_BYTES)? != *before {
            return Ok(false);
        }
        match rename(tmp, sidecar) {
            Err(e) if transient(&e) && retries < RENAME_RETRIES => {
                retries += 1;
                thread::sleep(RENAME_PAUSE);
            }
            result => return result.map(|()| true),
        }
    }
}

/// A refusal that passes once another program lets go of the file, as Windows refuses a rename
/// while either file is open elsewhere: "permission denied", or a sharing violation
/// (`ERROR_SHARING_VIOLATION`, 32), which `std` gives no kind of its own. Elsewhere 32 is `EPIPE`,
/// which a rename never returns.
fn transient(e: &io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    use super::{apply_op, apply_op_with_hooks, transient, StoreError, WRITE_RETRIES};
    use crate::review::ops::{NewAnchor, ReviewOp};
    use crate::review::{sidecar_path, EntryAuthor};

    const NOTE: &str =
        "# Tide sync\n\n## Batching\n\nThe client uploads readings in batches of at most 50.\n";
    const NOW: &str = "2026-10-06T10:00:00Z";
    const CLAUDE: &str = "\n**Claude (reply):** ok\n";

    /// A folder holding `tide.md` and a sidecar with one comment, C1, made by Lectern.
    fn tide_with_one_comment() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("tide.md");
        fs::write(&note, NOTE).unwrap();
        let sidecar = sidecar_path(&note);
        let add = ReviewOp::Add {
            anchor: NewAnchor {
                start_line: 5,
                end_line: 5,
                quote: "batches of at most 50".into(),
                prefix: String::new(),
            },
            text: "Why 50?".into(),
        };
        apply_op(&sidecar, "tide.md", NOTE, &add, NOW).unwrap();
        (dir, sidecar)
    }

    fn append(path: &Path, text: &str) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn reply() -> ReviewOp {
        ReviewOp::Reply {
            id: 1,
            text: "Per station.".into(),
        }
    }

    #[test]
    fn a_concurrent_claude_append_is_merged_not_overwritten() {
        let (dir, sidecar) = tide_with_one_comment();
        let mut calls = 0;
        // Claude appends a reply between Lectern's read and its rename, on the first attempt only.
        let mut claude_writes = || {
            calls += 1;
            if calls == 1 {
                append(&sidecar, CLAUDE);
            }
        };

        let (review, id) = apply_op_with_hooks(
            &sidecar,
            "tide.md",
            NOTE,
            &reply(),
            NOW,
            &mut claude_writes,
            &mut fs_rename,
        )
        .unwrap();

        assert_eq!(id, 1);
        assert_eq!(calls, 2, "the save started over once");
        let content = fs::read_to_string(&sidecar).unwrap();
        let claude = content.find("**Claude (reply):** ok").expect(&content);
        let yours = content.find("**You:** Per station.").expect(&content);
        assert!(
            claude < yours,
            "the reply follows Claude's entry:\n{content}"
        );
        let c1 = review.comments().next().unwrap();
        let authors: Vec<EntryAuthor> = c1.entries.iter().map(|e| e.author).collect();
        assert_eq!(
            authors,
            [EntryAuthor::You, EntryAuthor::Agent, EntryAuthor::You]
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp file");
    }

    #[test]
    fn a_file_that_keeps_changing_gives_conflict_and_no_temp_file() {
        let (dir, sidecar) = tide_with_one_comment();
        let before = fs::read_to_string(&sidecar).unwrap();
        let mut calls = 0;
        let mut claude_writes = || {
            calls += 1;
            append(&sidecar, CLAUDE);
        };

        let err = apply_op_with_hooks(
            &sidecar,
            "tide.md",
            NOTE,
            &reply(),
            NOW,
            &mut claude_writes,
            &mut fs_rename,
        )
        .unwrap_err();

        assert!(matches!(err, StoreError::Conflict), "{err:?}");
        assert_eq!(
            err.to_string(),
            "Couldn't save the comment: the file kept changing."
        );
        assert_eq!(calls, WRITE_RETRIES + 1, "the first try and every restart");
        let after = fs::read_to_string(&sidecar).unwrap();
        assert_eq!(
            after,
            before + &CLAUDE.repeat(calls),
            "only Claude's writes landed"
        );
        assert!(!dir.path().join(".tide.review.md.lectern.tmp").exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_refused_rename_is_retried_on_permission_denied_and_sharing_violation() {
        assert!(transient(&io::ErrorKind::PermissionDenied.into()));
        // ERROR_SHARING_VIOLATION: another program has the file open on Windows.
        assert!(transient(&io::Error::from_raw_os_error(32)));
        assert!(!transient(&io::ErrorKind::NotFound.into()));
        assert!(!transient(&io::ErrorKind::AlreadyExists.into()));
    }

    fn fs_rename(from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    #[test]
    fn a_change_during_a_refused_rename_is_merged_not_overwritten() {
        let (dir, sidecar) = tide_with_one_comment();
        let mut renames = 0;
        // The first rename is refused for a moment, as Windows refuses it while another program
        // has the file open, and that program is Claude appending a reply.
        let mut refused_once = |from: &Path, to: &Path| {
            renames += 1;
            if renames == 1 {
                append(&sidecar, CLAUDE);
                return Err(io::Error::from_raw_os_error(32));
            }
            fs::rename(from, to)
        };

        let (review, id) = apply_op_with_hooks(
            &sidecar,
            "tide.md",
            NOTE,
            &reply(),
            NOW,
            &mut || {},
            &mut refused_once,
        )
        .unwrap();

        assert_eq!(id, 1);
        assert_eq!(renames, 2, "the save started over and renamed once more");
        let content = fs::read_to_string(&sidecar).unwrap();
        let claude = content.find("**Claude (reply):** ok").expect(&content);
        let yours = content.find("**You:** Per station.").expect(&content);
        assert!(claude < yours, "both kept, in order:\n{content}");
        let c1 = review.comments().next().unwrap();
        let authors: Vec<EntryAuthor> = c1.entries.iter().map(|e| e.author).collect();
        assert_eq!(
            authors,
            [EntryAuthor::You, EntryAuthor::Agent, EntryAuthor::You]
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp file");
    }
}
