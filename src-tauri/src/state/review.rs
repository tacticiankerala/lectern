//! Review comments on the open note: loading its sidecar, and saving the changes the UI asks for.
//!
//! Only the sidecar of the note on screen is ever touched, at the path derived from the note's,
//! and only while the feature is on. The note itself is read, never written.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use lectern_core::library::is_markdown;
use lectern_core::review::anchor::resolve_all;
use lectern_core::review::ops::ReviewOp;
use lectern_core::review::text::TextMap;
use lectern_core::review::view::{build_payload, PayloadInput, ReviewPayload};
use lectern_core::review::{fingerprint, is_sidecar_name, iso_utc, sidecar_path, store, Review};

use super::doc::{read_text, render_text};
use super::sync::{lock, read};
use super::{trust, AppState};

const FEATURE_OFF: &str = "Review comments are turned off in Preferences.";
const NOT_OPEN: &str = "Comments can only be loaded or saved for the open note.";
const NOT_A_NOTE: &str = "Comments can only be added to Markdown notes.";

/// The note a review command works on, and its sidecar.
struct Target {
    note: PathBuf,
    sidecar: PathBuf,
    /// The note's file name, which the sidecar's `note:` must match.
    note_name: String,
}

impl Target {
    /// Whether the note can have comments: it's Markdown, and not a sidecar itself. Anything else
    /// would get a sidecar the library never hides (`notes.txt.review.md`), or a sidecar's sidecar
    /// (`plan.review.review.md`).
    fn takes_comments(&self) -> bool {
        is_markdown(&self.note_name) && !is_sidecar_name(&self.note_name)
    }
}

impl AppState {
    /// The open note's comments. Without a sidecar the note isn't read. A file that can't take
    /// comments (see `review_target`) shows none, read-only, and nothing is read.
    pub fn load_review(&self, path: &str) -> Result<ReviewPayload, String> {
        let target = self.open_target(path)?;
        if !target.takes_comments() {
            return Ok(self.review_payload(&target, None, Some(NOT_A_NOTE.to_owned())));
        }
        if let Err(e) = fs::metadata(&target.sidecar) {
            if e.kind() == io::ErrorKind::NotFound {
                return Ok(self.review_payload(&target, None, None));
            }
        }
        let loaded = store::load(&target.sidecar, &target.note_name)
            .map_err(|e| format!("Couldn't read the comments: {e}"))?;
        let Some(review) = loaded.review else {
            return Ok(self.review_payload(&target, None, loaded.read_only));
        };
        let source = read_text(&target.note).map_err(|e| e.message)?.text;
        Ok(self.review_payload(&target, Some((&review, &source)), loaded.read_only))
    }

    /// Applies `op` to the open note's sidecar, creating it if need be, and returns the comments
    /// as saved.
    pub fn review_op(&self, path: &str, op: ReviewOp) -> Result<ReviewPayload, String> {
        let target = self.review_target(path)?;
        let source = read_text(&target.note).map_err(|e| e.message)?.text;
        let now = iso_utc(SystemTime::now());
        let saved = store::apply_op(&target.sidecar, &target.note_name, &source, &op, &now);
        let (review, _) = saved.map_err(|e| {
            log::warn!(
                "couldn't save a review comment to {}: {e}",
                target.sidecar.display()
            );
            e.to_string()
        })?;
        Ok(self.review_payload(&target, Some((&review, &source)), None))
    }

    /// Whether review comments are on.
    pub(super) fn reviews_on(&self) -> bool {
        read(&self.settings).review_comments
    }

    /// `path` as a note a review command may change the sidecar of: the open note (see
    /// `open_target`), and a Markdown note rather than a sidecar itself.
    fn review_target(&self, path: &str) -> Result<Target, String> {
        let target = self.open_target(path)?;
        if !target.takes_comments() {
            return Err(NOT_A_NOTE.to_owned());
        }
        Ok(target)
    }

    /// `path` as a note a review command may look at: the feature is on, the path is trusted, and
    /// it is the open note. Every check is in memory.
    fn open_target(&self, path: &str) -> Result<Target, String> {
        if !self.reviews_on() {
            return Err(FEATURE_OFF.to_owned());
        }
        if !self.trusts(path) {
            return Err(trust::refusal(path));
        }
        let note = PathBuf::from(path);
        let open = note.is_absolute()
            && lock(&self.current)
                .as_ref()
                .is_some_and(|current| current.path == note);
        if !open {
            return Err(NOT_OPEN.to_owned());
        }
        let note_name = note
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Target {
            sidecar: sidecar_path(&note),
            note,
            note_name,
        })
    }

    /// The payload for `found`, a review and the note's text, with each comment found in the
    /// text again. Comment bodies render as the note does, so their links follow its rules.
    fn review_payload(
        &self,
        target: &Target,
        found: Option<(&Review, &str)>,
        read_only: Option<String>,
    ) -> ReviewPayload {
        let (text, resolved) = match found {
            Some((review, source)) => {
                let text = TextMap::build(source);
                let resolved = resolve_all(review, &text, &fingerprint(source));
                (Some(text), resolved)
            }
            None => (None, Vec::new()),
        };
        let index = self.index();
        let mapper = self.mapper();
        let hosts = read(&self.trust).hosts();
        let with_index = (!index.roots.is_empty()).then_some(&*index);
        let render = |md: &str| render_text(&target.note, md, with_index, &mapper, &hosts).html;
        build_payload(
            PayloadInput {
                note: &target.note,
                sidecar: &target.sidecar,
                review: found.map(|(review, _)| review),
                resolved: &resolved,
                text: text.as_ref(),
                read_only,
                note_wsl: mapper.to_wsl(&target.note),
                sidecar_wsl: mapper.to_wsl(&target.sidecar),
            },
            &render,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use lectern_core::ipc::SettingsPatch;
    use lectern_core::library::pathmap::asset_url;
    use lectern_core::review::anchor::AnchorState;
    use lectern_core::review::ops::NewAnchor;
    use lectern_core::watch::WatchEvent;

    use super::*;
    use crate::state::doc::ASSET_BASE;
    use crate::state::paths::path_string;
    use crate::state::test_support::*;
    use crate::state::trust;

    const NOTE: &str =
        "# Tide sync\n\n## Batching\n\nThe client uploads readings in batches of at most 50.\n";

    /// A fixture with `notes/tide.md` open.
    fn with_open_note() -> (Fixture, PathBuf) {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.file("notes/tide.md", NOTE);
        opened_path(&f.state.open_document(&path_string(&note)));
        (f, note)
    }

    fn add(text: &str) -> ReviewOp {
        ReviewOp::Add {
            anchor: NewAnchor {
                start_line: 5,
                end_line: 5,
                quote: "batches of at most 50".to_owned(),
                prefix: String::new(),
            },
            text: text.to_owned(),
        }
    }

    fn set_reviews(f: &Fixture, on: bool) {
        f.state.set_settings(SettingsPatch {
            review_comments: Some(on),
            ..SettingsPatch::default()
        });
    }

    #[test]
    fn load_review_without_a_sidecar_is_empty_and_reads_nothing() {
        let (f, note) = with_open_note();
        // Gone since it opened: a load that read the note would fail.
        fs::remove_file(&note).unwrap();
        let payload = f.state.load_review(&path_string(&note)).unwrap();
        let sidecar = note.with_file_name("tide.review.md");
        assert!(!payload.exists);
        assert!(payload.comments.is_empty());
        assert!(payload.unreadable.is_empty());
        assert_eq!(payload.open_count, 0);
        assert_eq!(payload.read_only, None);
        assert_eq!(payload.note_path, path_string(&note));
        assert_eq!(payload.sidecar_path, path_string(&sidecar));
        assert!(!sidecar.exists());
    }

    #[test]
    fn review_op_add_creates_the_sidecar_beside_the_open_note() {
        let (f, note) = with_open_note();
        let path = path_string(&note);
        let payload = f
            .state
            .review_op(&path, add("Why 50? See ![chart](chart.png)."))
            .unwrap();
        let sidecar = note.with_file_name("tide.review.md");
        assert!(sidecar.is_file());
        assert_eq!(fs::read_to_string(&note).unwrap(), NOTE, "the note changed");
        assert!(payload.exists);
        assert_eq!(payload.sidecar_path, path_string(&sidecar));
        // The temp folder is on a drive, which WSL sees under /mnt.
        let wsl = |p: &Option<String>, name: &str| {
            p.as_deref()
                .is_some_and(|p| p.starts_with("/mnt/") && p.ends_with(name))
        };
        assert!(
            wsl(&payload.note_wsl_path, "/notes/tide.md"),
            "{:?}",
            payload.note_wsl_path
        );
        assert!(
            wsl(&payload.sidecar_wsl_path, "/notes/tide.review.md"),
            "{:?}",
            payload.sidecar_wsl_path
        );
        assert_eq!(payload.open_count, 1);
        let [comment] = payload.comments.as_slice() else {
            panic!("{:?}", payload.comments);
        };
        assert_eq!(comment.id, 1);
        assert_eq!(comment.state, AnchorState::Anchored);
        assert_eq!((comment.start_line, comment.end_line), (5, 5));
        assert_eq!(comment.heading_path, ["Tide sync", "Batching"]);
        assert_eq!(comment.quote, "batches of at most 50");
        // The body renders as the note does: its images are found beside the note.
        let chart = asset_url(ASSET_BASE, &note.with_file_name("chart.png"));
        let html = &comment.entries[0].html;
        assert!(html.contains(&format!(r#"src="{chart}""#)), "{html}");

        let loaded = f.state.load_review(&path).unwrap();
        assert!(loaded.exists);
        let [again] = loaded.comments.as_slice() else {
            panic!("{:?}", loaded.comments);
        };
        assert_eq!(again.state, AnchorState::Anchored);
        assert_eq!(again.entries[0].text, comment.entries[0].text);
        assert_eq!(&again.entries[0].html, html);
    }

    #[test]
    fn review_op_reports_a_refused_change_in_words() {
        let (f, note) = with_open_note();
        let reply = ReviewOp::Reply {
            id: 9,
            text: "Agreed.".to_owned(),
        };
        assert_eq!(
            f.state.review_op(&path_string(&note), reply).unwrap_err(),
            "That comment no longer exists."
        );
        assert!(!note.with_file_name("tide.review.md").exists());
    }

    #[test]
    fn refuses_a_note_that_is_not_open() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.file("notes/tide.md", NOTE);
        let other = f.dir.file("notes/other.md", NOTE);
        assert_eq!(
            f.state.load_review(&path_string(&note)).unwrap_err(),
            NOT_OPEN,
            "nothing is open yet"
        );
        opened_path(&f.state.open_document(&path_string(&note)));
        assert_eq!(
            f.state.load_review(&path_string(&other)).unwrap_err(),
            NOT_OPEN
        );
        assert_eq!(
            f.state
                .review_op(&path_string(&other), add("Why 50?"))
                .unwrap_err(),
            NOT_OPEN
        );
        assert!(!other.with_file_name("other.review.md").exists());
        // The open note's name, but not its absolute path.
        assert_eq!(f.state.load_review("tide.md").unwrap_err(), NOT_OPEN);
        assert_eq!(
            f.state.review_op("tide.md", add("Why 50?")).unwrap_err(),
            NOT_OPEN
        );
        assert!(!note.with_file_name("tide.review.md").exists());
        // A relative path is refused even when it is the current one, which no open makes it.
        f.state
            .make_current(f.state.next_seq(), Path::new("tide.md"), None);
        assert_eq!(f.state.load_review("tide.md").unwrap_err(), NOT_OPEN);
        assert_eq!(
            f.state.review_op("tide.md", add("Why 50?")).unwrap_err(),
            NOT_OPEN
        );
    }

    #[test]
    fn refuses_a_note_that_is_not_markdown_or_is_a_sidecar() {
        let f = fixture(profile(&[]), FakeHost::default());
        for name in ["a/tide.txt", "b/plan.review.md", "c/PLAN.Review.MD"] {
            let doc = f.dir.file(name, NOTE);
            let path = path_string(&doc);
            opened_path(&f.state.open_document(&path));
            assert_eq!(
                f.state.review_op(&path, add("Why 50?")).unwrap_err(),
                NOT_A_NOTE,
                "{name}"
            );
            assert!(!sidecar_path(&doc).exists(), "{name}");
            // Shown read-only with the reason, so nothing offers to add one.
            let loaded = f.state.load_review(&path).unwrap();
            assert!(!loaded.exists, "{name}");
            assert!(loaded.comments.is_empty(), "{name}");
            assert_eq!(loaded.read_only.as_deref(), Some(NOT_A_NOTE), "{name}");
        }
    }

    #[test]
    fn refuses_when_the_feature_is_off() {
        let (f, note) = with_open_note();
        let path = path_string(&note);
        let sidecar = note.with_file_name("tide.review.md");
        f.state.review_op(&path, add("Why 50?")).unwrap();
        let saved = fs::read(&sidecar).unwrap();
        set_reviews(&f, false);
        assert_eq!(f.state.load_review(&path).unwrap_err(), FEATURE_OFF);
        assert_eq!(
            f.state
                .review_op(&path, add("And why not 100?"))
                .unwrap_err(),
            FEATURE_OFF
        );
        assert_eq!(fs::read(&sidecar).unwrap(), saved);
        // Checked first: before trust and before the open note.
        let unc = r"\\lectern-untrusted.invalid\share\a.md";
        assert_eq!(f.state.load_review(unc).unwrap_err(), FEATURE_OFF);
        set_reviews(&f, true);
        assert!(f.state.load_review(&path).unwrap().exists);
    }

    #[test]
    fn refuses_an_untrusted_unc_note() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = r"\\lectern-untrusted.invalid\share\a.md";
        let refusal = trust::refusal(doc);
        // Current as a failed open would leave it, so only the trust check stands in the way.
        f.state
            .make_current(f.state.next_seq(), Path::new(doc), None);
        let started = Instant::now();
        assert_eq!(f.state.load_review(doc).unwrap_err(), refusal);
        assert_eq!(f.state.review_op(doc, add("Why 50?")).unwrap_err(), refusal);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn review_changed_reaches_the_ui() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.0.join("notes").join("tide.md");
        f.state
            .on_watch_event(WatchEvent::ReviewChanged(note.clone()));
        assert_eq!(f.host.review_changes(), [note]);
    }

    /// Whether a scan of any root is running or asked for.
    fn scanning(f: &Fixture) -> bool {
        lock(&f.state.library).roots.iter().any(|r| r.scanning)
    }

    /// On a share without change notifications the poll that saw the sidecar change is all there
    /// is, so it brings the note's comment count in the library up to date.
    #[test]
    fn review_changed_rescans_the_library_root_holding_the_note() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let note = dir.file("vault/notes/tide.md", NOTE);
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.state.window_shown();
        wait_until("the root is indexed", || settled(&f, &root));
        let path = path_string(&note);
        opened_path(&f.state.open_document(&path));
        f.state.review_op(&path, add("Why 50?")).unwrap();
        let count = |f: &Fixture| {
            f.state
                .index()
                .root_for(&note)
                .and_then(|r| r.comment_count("notes/tide.md"))
        };
        assert_eq!(count(&f), None, "no watcher here, so no scan yet");

        f.state
            .on_watch_event(WatchEvent::ReviewChanged(note.clone()));

        wait_until("the count is in", || count(&f) == Some(1));
        assert_eq!(f.host.review_changes(), [note]);
    }

    /// Notes outside every library root, and their folder's ad-hoc root, aren't scanned again, nor
    /// is anything while the feature is off.
    #[test]
    fn review_changed_rescans_nothing_outside_the_user_roots_or_while_off() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let inside = dir.file("vault/tide.md", NOTE);
        let outside = dir.file("elsewhere/tide.md", NOTE);
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.state.window_shown();
        opened_path(&f.state.open_document(&path_string(&outside)));
        let adhoc = outside.parent().unwrap().to_path_buf();
        wait_until("both roots are indexed", || {
            settled(&f, &root) && settled(&f, &adhoc)
        });

        f.state
            .on_watch_event(WatchEvent::ReviewChanged(outside.clone()));
        assert!(!scanning(&f), "the ad-hoc root isn't the user's");

        set_reviews(&f, false);
        f.state
            .on_watch_event(WatchEvent::ReviewChanged(inside.clone()));
        assert!(!scanning(&f), "the feature is off");
        assert_eq!(f.host.review_changes(), [outside]);
    }

    /// The watcher stats the sidecar whatever the settings say; with the feature off the UI hears
    /// nothing of it.
    #[test]
    fn review_changed_is_dropped_while_the_feature_is_off() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.0.join("notes").join("tide.md");
        set_reviews(&f, false);
        f.state
            .on_watch_event(WatchEvent::ReviewChanged(note.clone()));
        assert!(f.host.review_changes().is_empty());
        set_reviews(&f, true);
        f.state
            .on_watch_event(WatchEvent::ReviewChanged(note.clone()));
        assert_eq!(f.host.review_changes(), [note]);
    }
}
