//! Review comments on the open note: loading its sidecar, and saving the changes the UI asks for.
//!
//! Only the sidecar of the note on screen is ever touched, at the path derived from the note's,
//! and only while the feature is on. The note itself is read, never written. An HTML page's text
//! is the one its bridge reported as the page ran, which the UI sends with every command; its
//! source is read for the lines and the fingerprint.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use lectern_core::library::{doc_kind, DocKind};
use lectern_core::review::anchor::resolve_all;
use lectern_core::review::ops::ReviewOp;
use lectern_core::review::text::{page_text_len, NoteText, PageBlock, MAX_PAGE_TEXT};
use lectern_core::review::view::{build_payload, PayloadInput, ReviewPayload};
use lectern_core::review::{is_sidecar_name, iso_utc, sidecar_path, store, Review};

use super::doc::{read_text, render_text};
use super::sync::{lock, read};
use super::{trust, WindowState};

const FEATURE_OFF: &str = "Review comments are turned off in Preferences.";
const NOT_OPEN: &str = "Comments can only be loaded or saved for the open note.";
const NOT_A_NOTE: &str = "Comments can only be added to notes, text files and pages.";
const NOT_ON_A_PDF: &str = "PDFs don't take comments.";
const NO_PAGE_TEXT: &str =
    "The page hasn't reported its text yet, so its comments can't be placed.";
const PAGE_TOO_LONG: &str = "The page's text is over 2 MB, too much for comments.";

/// The note a review command works on, and its sidecar.
struct Target {
    note: PathBuf,
    sidecar: PathBuf,
    /// The note's file name, which the sidecar's `note:` must match.
    note_name: String,
}

impl Target {
    /// Why the note can't have comments, or `None` when it can: a Markdown note, a text file or an
    /// HTML page can, unless it is a sidecar itself (`plan.review.review.md` would be a sidecar's
    /// sidecar). A PDF never does.
    fn refusal(&self) -> Option<&'static str> {
        match doc_kind(&self.note_name) {
            Some(DocKind::Pdf) => Some(NOT_ON_A_PDF),
            Some(kind) if kind.takes_comments() && !is_sidecar_name(&self.note_name) => None,
            _ => Some(NOT_A_NOTE),
        }
    }

    /// The page's text a command was sent, checked: an HTML page's text is what its bridge
    /// reported, so it must come, and be no more than the bridge reports (`MAX_PAGE_TEXT`). Any
    /// other note's text is its file's, so what came with it is dropped. In memory only.
    fn page(&self, page: Option<Vec<PageBlock>>) -> Result<Option<Vec<PageBlock>>, String> {
        if doc_kind(&self.note_name) != Some(DocKind::Html) {
            return Ok(None);
        }
        let page = page.ok_or(NO_PAGE_TEXT)?;
        if page_text_len(&page) > MAX_PAGE_TEXT {
            return Err(PAGE_TOO_LONG.to_owned());
        }
        Ok(Some(page))
    }

    /// Reads the note, and its text as the review anchors read it, with its fingerprint: an HTML
    /// page's from the blocks its bridge reported (`page`, see `NoteText::of_page`), any other's
    /// by its kind (`NoteText::of_note`).
    fn read(&self, page: Option<&[PageBlock]>) -> Result<NoteText, String> {
        let source = read_text(&self.note).map_err(|e| e.message)?.text;
        Ok(match page {
            Some(blocks) => NoteText::of_page(blocks, &source),
            None => NoteText::of_note(&self.note_name, &source),
        })
    }
}

impl WindowState {
    /// The open note's comments. Without a sidecar the note isn't read. A file that can't take
    /// comments (see `review_target`) shows none, read-only with the reason, and nothing is read.
    /// An HTML page's come only with its text (`page`, see `Target::page`).
    pub fn load_review(
        &self,
        path: &str,
        page: Option<Vec<PageBlock>>,
    ) -> Result<ReviewPayload, String> {
        let target = self.open_target(path)?;
        if let Some(refusal) = target.refusal() {
            return Ok(self.review_payload(&target, None, Some(refusal.to_owned())));
        }
        let page = target.page(page)?;
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
        let note = target.read(page.as_deref())?;
        Ok(self.review_payload(&target, Some((&review, &note)), loaded.read_only))
    }

    /// Applies `op` to the open note's sidecar, creating it if need be, and returns the comments
    /// as saved. An HTML page's take its text (`page`, see `Target::page`).
    pub fn review_op(
        &self,
        path: &str,
        op: ReviewOp,
        page: Option<Vec<PageBlock>>,
    ) -> Result<ReviewPayload, String> {
        let target = self.review_target(path)?;
        let page = target.page(page)?;
        let note = target.read(page.as_deref())?;
        let now = iso_utc(SystemTime::now());
        let saved = store::apply_op_on(&target.sidecar, &target.note_name, &note, &op, &now);
        let (review, _) = saved.map_err(|e| {
            log::warn!(
                "couldn't save a review comment to {}: {e}",
                target.sidecar.display()
            );
            e.to_string()
        })?;
        Ok(self.review_payload(&target, Some((&review, &note)), None))
    }

    /// Whether review comments are on.
    pub(super) fn reviews_on(&self) -> bool {
        read(&self.app.settings).review_comments
    }

    /// `path` as a note a review command may change the sidecar of: the open note (see
    /// `open_target`), and one that takes comments (`Target::refusal`).
    fn review_target(&self, path: &str) -> Result<Target, String> {
        let target = self.open_target(path)?;
        if let Some(refusal) = target.refusal() {
            return Err(refusal.to_owned());
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

    /// The payload for `found`, a review and the note's text (`Target::read`), with each comment
    /// found in the text again. Comment bodies render as Markdown beside the note, so their links
    /// follow a note's rules.
    fn review_payload(
        &self,
        target: &Target,
        found: Option<(&Review, &NoteText)>,
        read_only: Option<String>,
    ) -> ReviewPayload {
        let resolved = found.map_or_else(Vec::new, |(review, note)| {
            resolve_all(review, &note.text, &note.fingerprint)
        });
        let index = self.index();
        let mapper = self.mapper();
        let hosts = read(&self.app.trust).hosts();
        let with_index = (!index.roots.is_empty()).then_some(&*index);
        let render = |md: &str| render_text(&target.note, md, with_index, &mapper, &hosts).html;
        build_payload(
            PayloadInput {
                note: &target.note,
                sidecar: &target.sidecar,
                review: found.map(|(review, _)| review),
                resolved: &resolved,
                text: found.map(|(_, note)| &note.text),
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
        let payload = f.state.load_review(&path_string(&note), None).unwrap();
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
            .review_op(&path, add("Why 50? See ![chart](chart.png)."), None)
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

        let loaded = f.state.load_review(&path, None).unwrap();
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
            f.state
                .review_op(&path_string(&note), reply, None)
                .unwrap_err(),
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
            f.state.load_review(&path_string(&note), None).unwrap_err(),
            NOT_OPEN,
            "nothing is open yet"
        );
        opened_path(&f.state.open_document(&path_string(&note)));
        assert_eq!(
            f.state.load_review(&path_string(&other), None).unwrap_err(),
            NOT_OPEN
        );
        assert_eq!(
            f.state
                .review_op(&path_string(&other), add("Why 50?"), None)
                .unwrap_err(),
            NOT_OPEN
        );
        assert!(!other.with_file_name("other.review.md").exists());
        // The open note's name, but not its absolute path.
        assert_eq!(f.state.load_review("tide.md", None).unwrap_err(), NOT_OPEN);
        assert_eq!(
            f.state
                .review_op("tide.md", add("Why 50?"), None)
                .unwrap_err(),
            NOT_OPEN
        );
        assert!(!note.with_file_name("tide.review.md").exists());
        // A relative path is refused even when it is the current one, which no open makes it.
        f.state
            .make_current(f.state.next_seq(), Path::new("tide.md"), None);
        assert_eq!(f.state.load_review("tide.md", None).unwrap_err(), NOT_OPEN);
        assert_eq!(
            f.state
                .review_op("tide.md", add("Why 50?"), None)
                .unwrap_err(),
            NOT_OPEN
        );
    }

    /// A PDF never takes comments (and says so), and a sidecar takes none of its own.
    #[test]
    fn refuses_a_pdf_or_a_sidecar() {
        let f = fixture(profile(&[]), FakeHost::default());
        for (name, reason) in [
            ("b/plan.review.md", NOT_A_NOTE),
            ("c/PLAN.Review.MD", NOT_A_NOTE),
            ("d/brief.pdf", "PDFs don't take comments."),
            ("e/BRIEF.PDF", "PDFs don't take comments."),
        ] {
            let doc = f.dir.file(name, NOTE);
            let path = path_string(&doc);
            opened_path(&f.state.open_document(&path));
            assert_eq!(
                f.state.review_op(&path, add("Why 50?"), None).unwrap_err(),
                reason,
                "{name}"
            );
            assert!(!sidecar_path(&doc).exists(), "{name}");
            // Shown read-only with the reason, so nothing offers to add one.
            let loaded = f.state.load_review(&path, None).unwrap();
            assert!(!loaded.exists, "{name}");
            assert!(loaded.comments.is_empty(), "{name}");
            assert_eq!(loaded.read_only.as_deref(), Some(reason), "{name}");
        }
    }

    /// An invented page, laid out as `fixtures/vault/artifacts/report.html` is, whose script builds
    /// the paragraph it adds from words, so that text is nowhere in the source.
    const REPORT: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Weekly report</title>
<link rel="stylesheet" href="style.css">
</head>
<body>
<h1>Weekly report</h1>
<p>An invented report for Lectern's tests: the lanternfish survey, week by week.</p>
<h2>Results</h2>
<p>Sign-ups rose steadily, but conversion dipped in week 3 before recovering.</p>
<table>
<tr><th>Week</th><th>Sign-ups</th></tr>
<tr><td>3</td><td>151</td></tr>
</table>
<h2>Next steps</h2>
<p>Check the week 3 dip against the survey calendar, as the <a href="other.html">other page</a> does.</p>
<script>
  var note = document.createElement("p");
  note.textContent = ["Added", "by", "a", "script"].join(" ");
  document.body.appendChild(note);
</script>
</body>
</html>
"#;

    /// What the bridge reports of `REPORT` as it runs: its blocks, the script's paragraph last.
    fn report_page() -> Vec<PageBlock> {
        let block = |text: &str, heading_level: Option<u8>| PageBlock {
            text: text.to_owned(),
            heading_level,
        };
        vec![
            block("Weekly report", Some(1)),
            block(
                "An invented report for Lectern's tests: the lanternfish survey, week by week.",
                None,
            ),
            block("Results", Some(2)),
            block(
                "Sign-ups rose steadily, but conversion dipped in week 3 before recovering.",
                None,
            ),
            block("Week", None),
            block("Sign-ups", None),
            block("3", None),
            block("151", None),
            block("Next steps", Some(2)),
            block(
                "Check the week 3 dip against the survey calendar, as the other page does.",
                None,
            ),
            block("Added by a script", None),
        ]
    }

    /// A comment on `quote`, selected in a page just after the page's text `prefix`: a page's
    /// selection carries no lines.
    fn add_on_page(quote: &str, prefix: &str, text: &str) -> ReviewOp {
        ReviewOp::Add {
            anchor: NewAnchor {
                start_line: 0,
                end_line: 0,
                quote: quote.to_owned(),
                prefix: prefix.to_owned(),
            },
            text: text.to_owned(),
        }
    }

    /// The page's text from UTF-16 offset `start` up to `end`, as the UI cuts it.
    fn utf16_slice(page: &[PageBlock], start: Option<u32>, end: Option<u32>) -> String {
        let joined: Vec<&str> = page.iter().map(|b| b.text.as_str()).collect();
        let units: Vec<u16> = joined.join(" ").encode_utf16().collect();
        let (start, end) = (start.unwrap() as usize, end.unwrap() as usize);
        String::from_utf16(&units[start..end]).unwrap()
    }

    /// An HTML page takes comments on the text its bridge reported: `<page>.html.review.md` gets
    /// the quote, the line of its paragraph in the page's source and its heading path, and the
    /// comment is found on the same words when the page opens again.
    #[test]
    fn a_page_takes_comments_on_the_text_its_bridge_reported() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        let empty = f.state.load_review(&path, Some(report_page())).unwrap();
        assert!(!empty.exists);
        assert_eq!(empty.read_only, None);

        let saved = f
            .state
            .review_op(
                &path,
                add_on_page(
                    "conversion dipped in week 3",
                    "Results Sign-ups rose steadily, but ",
                    "weekly, not daily",
                ),
                Some(report_page()),
            )
            .unwrap();
        let sidecar = page.with_file_name("report.html.review.md");
        assert_eq!(saved.sidecar_path, path_string(&sidecar));
        let content = fs::read_to_string(&sidecar).unwrap();
        assert!(content.contains("\nnote: report.html\n"), "{content}");
        assert!(
            content.contains("\n## C1 · open · L12 · Weekly report › Results\n<!-- anchor prefix="),
            "{content}"
        );
        assert!(
            content.contains("\n> conversion dipped in week 3\n\n**You:** weekly, not daily\n"),
            "{content}"
        );
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            REPORT,
            "the page changed"
        );

        let loaded = f.state.load_review(&path, Some(report_page())).unwrap();
        let [c1] = loaded.comments.as_slice() else {
            panic!("{:?}", loaded.comments);
        };
        assert_eq!(c1.state, AnchorState::Anchored);
        assert_eq!((c1.start_line, c1.end_line), (12, 12));
        assert_eq!(c1.heading_path, ["Weekly report", "Results"]);
        assert_eq!(
            utf16_slice(&report_page(), c1.text_start, c1.text_end),
            "conversion dipped in week 3"
        );
    }

    /// Text only a script makes is anchored by its quote all the same; its line is that of the
    /// block before it found in the source.
    #[test]
    fn a_comment_on_text_a_script_made_takes_the_line_before_it() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        let saved = f
            .state
            .review_op(
                &path,
                add_on_page("by a script", "other page does. Added ", "Which script?"),
                Some(report_page()),
            )
            .unwrap();
        let [c1] = saved.comments.as_slice() else {
            panic!("{:?}", saved.comments);
        };
        assert_eq!(c1.state, AnchorState::Anchored);
        assert_eq!((c1.start_line, c1.end_line), (18, 18));
        assert_eq!(c1.heading_path, ["Weekly report", "Next steps"]);
        assert_eq!(
            utf16_slice(&report_page(), c1.text_start, c1.text_end),
            "by a script"
        );
    }

    /// Edited so the sentence moves down, the page's comment follows it; reworded, it is found as
    /// moved. Never lost.
    #[test]
    fn a_page_comment_follows_its_text_through_edits() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        f.state
            .review_op(
                &path,
                add_on_page(
                    "conversion dipped in week 3 before recovering",
                    "Sign-ups rose steadily, but ",
                    "weekly, not daily",
                ),
                Some(report_page()),
            )
            .unwrap();

        let added = "<p>A new paragraph, added above the results.</p>\n";
        fs::write(
            &page,
            REPORT.replace("<h2>Results</h2>\n", &format!("{added}<h2>Results</h2>\n")),
        )
        .unwrap();
        let mut moved_down = report_page();
        moved_down.insert(
            2,
            PageBlock {
                text: "A new paragraph, added above the results.".to_owned(),
                heading_level: None,
            },
        );
        let loaded = f
            .state
            .load_review(&path, Some(moved_down.clone()))
            .unwrap();
        let c1 = &loaded.comments[0];
        assert_eq!(c1.state, AnchorState::Anchored);
        assert_eq!((c1.start_line, c1.end_line), (13, 13));
        assert_eq!(
            utf16_slice(&moved_down, c1.text_start, c1.text_end),
            "conversion dipped in week 3 before recovering"
        );

        let reworded = |text: &str| text.replace("dipped in week 3", "dipped sharply in week 3");
        fs::write(&page, reworded(REPORT)).unwrap();
        let page_now: Vec<PageBlock> = report_page()
            .into_iter()
            .map(|b| PageBlock {
                text: reworded(&b.text),
                ..b
            })
            .collect();
        let loaded = f.state.load_review(&path, Some(page_now)).unwrap();
        let c1 = &loaded.comments[0];
        assert_eq!(c1.state, AnchorState::Moved);
        assert_eq!((c1.start_line, c1.end_line), (12, 12));
    }

    /// A page's comments are fingerprinted by the text it showed, not by its file: the same HTML
    /// shows other text when a script writes something else or scripts are turned off, and a quote
    /// gone from what it shows is looked for, not kept where it was.
    #[test]
    fn a_page_showing_other_text_from_the_same_file_looks_for_its_comments_again() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        let saved = f
            .state
            .review_op(
                &path,
                add_on_page("Added by a script", "", "Which script?"),
                Some(report_page()),
            )
            .unwrap();
        assert_eq!(saved.comments[0].state, AnchorState::Anchored);
        // The same file, with scripts off: the script's paragraph isn't shown.
        let mut scriptless = report_page();
        scriptless.pop();
        let loaded = f.state.load_review(&path, Some(scriptless)).unwrap();
        assert_eq!(loaded.comments[0].state, AnchorState::Detached);
        assert_eq!(loaded.comments[0].text_start, None);
        // Scripts back on: found again.
        let loaded = f.state.load_review(&path, Some(report_page())).unwrap();
        assert_eq!(loaded.comments[0].state, AnchorState::Anchored);
    }

    /// A page's comments need the text its bridge reported, no more of it than the bridge sends;
    /// without it nothing is read or written.
    #[test]
    fn a_page_without_its_text_or_with_too_much_is_refused() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        let op = || add_on_page("conversion dipped in week 3", "", "weekly, not daily");
        assert_eq!(f.state.load_review(&path, None).unwrap_err(), NO_PAGE_TEXT);
        assert_eq!(
            f.state.review_op(&path, op(), None).unwrap_err(),
            NO_PAGE_TEXT
        );
        let half = "x".repeat(MAX_PAGE_TEXT / 2);
        let big = vec![
            PageBlock {
                text: half.clone(),
                heading_level: None,
            },
            PageBlock {
                text: half,
                heading_level: None,
            },
        ];
        assert_eq!(
            f.state.load_review(&path, Some(big.clone())).unwrap_err(),
            PAGE_TOO_LONG
        );
        assert_eq!(
            f.state.review_op(&path, op(), Some(big)).unwrap_err(),
            PAGE_TOO_LONG
        );
        assert!(!sidecar_path(&page).exists());
        // A note's text is its file's: blocks sent with it are dropped.
        let note = f.dir.file("notes/tide.md", NOTE);
        let note_path = path_string(&note);
        opened_path(&f.state.open_document(&note_path));
        let payload = f
            .state
            .review_op(&note_path, add("Why 50?"), Some(report_page()))
            .unwrap();
        assert_eq!(payload.comments[0].heading_path, ["Tide sync", "Batching"]);
    }

    /// A file beside a page named as its sidecar, without a sidecar's frontmatter, isn't one
    /// Lectern made: shown read-only, never written.
    #[test]
    fn a_pages_sidecar_without_frontmatter_is_read_only() {
        let f = fixture(profile(&[]), FakeHost::default());
        let page = f.dir.file("artifacts/report.html", REPORT);
        let own = "# My notes on the report\n\n## C1 · open · L12\n> conversion dipped\n";
        let sidecar = f.dir.file("artifacts/report.html.review.md", own);
        let path = path_string(&page);
        opened_path(&f.state.open_document(&path));
        let loaded = f.state.load_review(&path, Some(report_page())).unwrap();
        assert!(loaded.exists);
        assert!(loaded.read_only.is_some());
        assert!(f
            .state
            .review_op(
                &path,
                add_on_page("conversion dipped in week 3", "", "weekly, not daily"),
                Some(report_page()),
            )
            .is_err());
        assert_eq!(fs::read_to_string(&sidecar).unwrap(), own);
    }

    /// A page saved in UTF-16 (by its byte order mark, either way round) is read for its lines like
    /// any other: its comments load, anchored, and take new ones; a UTF-16 text file opens.
    #[test]
    fn a_utf16_page_with_a_sidecar_loads_its_comments() {
        let f = fixture(profile(&[]), FakeHost::default());
        for (name, little) in [("le.html", true), ("be.html", false)] {
            let units = REPORT.encode_utf16().flat_map(|u| {
                if little {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                }
            });
            let bom: [u8; 2] = if little { [0xFF, 0xFE] } else { [0xFE, 0xFF] };
            let page = f.dir.0.join("artifacts").join(name);
            fs::create_dir_all(page.parent().unwrap()).unwrap();
            fs::write(&page, bom.into_iter().chain(units).collect::<Vec<u8>>()).unwrap();
            fs::write(
                sidecar_path(&page),
                format!("---\nlectern-review: 1\nnote: {name}\n---\n\n## C1 · open · L12 · Results\n> conversion dipped in week 3\n\n**Claude (question):** Weekly?\n"),
            )
            .unwrap();
            let path = path_string(&page);
            opened_path(&f.state.open_document(&path));

            let loaded = f.state.load_review(&path, Some(report_page())).unwrap();
            assert_eq!(loaded.read_only, None, "{name}");
            let [c1] = loaded.comments.as_slice() else {
                panic!("{name}: {:?}", loaded.comments);
            };
            assert_eq!(c1.state, AnchorState::Anchored, "{name}");
            assert_eq!((c1.start_line, c1.end_line), (12, 12), "{name}");
            assert_eq!(
                utf16_slice(&report_page(), c1.text_start, c1.text_end),
                "conversion dipped in week 3"
            );
            let saved = f
                .state
                .review_op(
                    &path,
                    add_on_page("Added by a script", "", "Which script?"),
                    Some(report_page()),
                )
                .unwrap();
            assert_eq!(saved.comments[1].start_line, 18, "{name}");
        }
        let text = f.dir.0.join("notes").join("utf16.txt");
        fs::create_dir_all(text.parent().unwrap()).unwrap();
        let units = "Readme in UTF-16.\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes);
        fs::write(
            &text,
            [0xFF, 0xFE].into_iter().chain(units).collect::<Vec<u8>>(),
        )
        .unwrap();
        opened_path(&f.state.open_document(&path_string(&text)));
    }

    const TEXT: &str = "Readme for the vault.\nEvery word is invented.\n\nThe lanternfish survey is plain text,\nso search reads it.\n";

    /// A text file's sidecar, `<file name>.review.md`, loads and takes new comments, anchored to its
    /// paragraphs, and they follow the text when it's edited.
    #[test]
    fn a_text_file_takes_comments() {
        let f = fixture(profile(&[]), FakeHost::default());
        let text = f.dir.file("notes/readme.txt", TEXT);
        let sidecar = text.with_file_name("readme.txt.review.md");
        fs::write(
            &sidecar,
            "---\nlectern-review: 1\nnote: readme.txt\n---\n\n## C1 · open · L1\n> Readme for the vault.\n\n**Claude (question):** Which vault?\n",
        )
        .unwrap();
        let path = path_string(&text);
        opened_path(&f.state.open_document(&path));

        let loaded = f.state.load_review(&path, None).unwrap();
        assert!(loaded.exists);
        assert_eq!(loaded.read_only, None);
        assert_eq!(loaded.sidecar_path, path_string(&sidecar));
        let [agents] = loaded.comments.as_slice() else {
            panic!("{:?}", loaded.comments);
        };
        assert_eq!(agents.state, AnchorState::Anchored);
        assert_eq!((agents.start_line, agents.end_line), (1, 2));

        let add = ReviewOp::Add {
            anchor: NewAnchor {
                start_line: 4,
                end_line: 5,
                quote: "lanternfish survey is plain text".to_owned(),
                prefix: "Every word is invented. The ".to_owned(),
            },
            text: "Which survey?".to_owned(),
        };
        let payload = f.state.review_op(&path, add, None).unwrap();
        let ours = &payload.comments[1];
        assert_eq!(ours.state, AnchorState::Anchored);
        assert_eq!((ours.start_line, ours.end_line), (4, 5));
        assert!(ours.heading_path.is_empty());
        let saved = fs::read_to_string(&sidecar).unwrap();
        assert!(saved.contains("\n## C2 · open · L4–L5\n"), "{saved}");
        assert!(
            saved.contains("\n> lanternfish survey is plain text\n"),
            "{saved}"
        );
        assert_eq!(
            fs::read_to_string(&text).unwrap(),
            TEXT,
            "the text file changed"
        );

        // Edited elsewhere so the paragraph moves down: the comment follows it.
        fs::write(&text, format!("A new first line.\n\n{TEXT}")).unwrap();
        let loaded = f.state.load_review(&path, None).unwrap();
        let moved = &loaded.comments[1];
        assert_eq!(moved.state, AnchorState::Anchored);
        assert_eq!((moved.start_line, moved.end_line), (6, 7));
        // Reworded: found again by its words, as moved.
        fs::write(&text, TEXT.replace("is plain text", "is now plain text")).unwrap();
        let loaded = f.state.load_review(&path, None).unwrap();
        let reworded = &loaded.comments[1];
        assert_eq!(reworded.state, AnchorState::Moved);
        assert_eq!((reworded.start_line, reworded.end_line), (4, 5));
    }

    #[test]
    fn refuses_when_the_feature_is_off() {
        let (f, note) = with_open_note();
        let path = path_string(&note);
        let sidecar = note.with_file_name("tide.review.md");
        f.state.review_op(&path, add("Why 50?"), None).unwrap();
        let saved = fs::read(&sidecar).unwrap();
        set_reviews(&f, false);
        assert_eq!(f.state.load_review(&path, None).unwrap_err(), FEATURE_OFF);
        assert_eq!(
            f.state
                .review_op(&path, add("And why not 100?"), None)
                .unwrap_err(),
            FEATURE_OFF
        );
        assert_eq!(fs::read(&sidecar).unwrap(), saved);
        // Checked first: before trust and before the open note.
        let unc = r"\\lectern-untrusted.invalid\share\a.md";
        assert_eq!(f.state.load_review(unc, None).unwrap_err(), FEATURE_OFF);
        set_reviews(&f, true);
        assert!(f.state.load_review(&path, None).unwrap().exists);
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
        assert_eq!(f.state.load_review(doc, None).unwrap_err(), refusal);
        assert_eq!(
            f.state.review_op(doc, add("Why 50?"), None).unwrap_err(),
            refusal
        );
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
        f.state.review_op(&path, add("Why 50?"), None).unwrap();
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
