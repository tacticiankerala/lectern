//! The review tools: `comment`, `comments`, `reply` and `resolve`, and `show`'s comment target.
//!
//! - **Notes and text files:** Rust writes the agent's entries to the sidecar itself, through the
//!   store's save loop, with no window needed: core finds where a comment goes in the file as it is
//!   on disk, and the entry is the agent's (`**Claude (question):**`), never yours. A window showing
//!   the file reloads its comments, as for any change from outside.
//! - **HTML pages:** a comment needs the page's live text, so only a page a window shows takes one:
//!   that window adds it, and replies and resolves, with the page's blocks and the agent as the
//!   author. A page's threads are listed against the text its window reports, or, no window
//!   showing it, at their stored places in an unknown state.
//! - **PDFs** take no comments.
//! - **Every tool** needs review comments turned on, and a sidecar Lectern may change: one it made,
//!   under the 2 MB cap. A thread that isn't there is the tool's error, naming those that are.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use lectern_core::agent::request::{CommentArgs, CommentsArgs, ReadArgs, ReplyArgs, ResolveArgs};
use lectern_core::agent::review::{
    comment_anchor, comment_op, list_threads, no_such_thread, reply_op, resolve_op, thread_id,
    thread_number, threads_page, CommentAnswer, CommentsAnswer, ThreadAnswer,
};
use lectern_core::agent::status::AgentReply;
use lectern_core::agent::target::check_target;
use lectern_core::agent::{Agent, AgentRequest};
use lectern_core::ipc::PathForms;
use lectern_core::library::{doc_kind, DocKind};
use lectern_core::review::ops::{OpError, ReviewOp};
use lectern_core::review::store::{self, StoreError};
use lectern_core::review::text::{page_text_len, NoteText, MAX_PAGE_TEXT};
use lectern_core::review::{
    is_sidecar_name, iso_utc, sidecar_path, Comment, CommentStatus, Review,
};
use lectern_core::watch::WatchEvent;

use super::{file_name, read_source, Budget, UNEXPECTED};
use crate::state::paths::{path_string, same_path};
use crate::state::review::{read_page_source, FEATURE_OFF, NOT_A_NOTE, NOT_ON_A_PDF};
use crate::state::sync::read;
use crate::state::{App, WindowState};

/// The tools' errors for an HTML page no window shows, whose text only a window has.
const SHOW_PAGE_TO_COMMENT: &str = "Show the page first, then comment.";
const SHOW_PAGE_TO_REPLY: &str = "Show the page first, then reply.";
const SHOW_PAGE_TO_RESOLVE: &str = "Show the page first, then resolve.";

impl App {
    /// `comment`: a thread on `args.target` in `args.path`, its first entry the agent's, within
    /// `budget`. A note's or a text file's is written by Rust; an HTML page's by the window showing
    /// it.
    pub(super) fn agent_comment(
        &self,
        args: CommentArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<CommentAnswer, String> {
        let (file, kind) = self.review_file(agent, &args.path)?;
        let name = file_name(&file);
        check_target(kind, &args.target, &name)?;
        let id = if kind == DocKind::Html {
            let window = self.window_showing(&file).ok_or(SHOW_PAGE_TO_COMMENT)?;
            let request = AgentRequest::Comment(CommentArgs {
                path: path_string(&file),
                ..args
            });
            self.in_page(&window, request, &agent.name, budget)?.0
        } else {
            let (text, source) = self.note_text(&file, &name, budget)?;
            let anchor = comment_anchor(&text, &source, &name, &args.target)?;
            let op = comment_op(anchor, args.text, &agent.name, args.kind);
            self.save_thread(&file, &name, &source, &op)?.1
        };
        Ok(CommentAnswer {
            id: thread_id(id),
            sidecar: self.sidecar_forms(agent, &file),
        })
    }

    /// `comments`: the threads of `args.path` with `args.status`, as the Comments tab shows them,
    /// within `budget`, a part at a time from `args.from` (`threads_page`). A note's are found in
    /// the file as it is on disk; an HTML page's in the text the window showing it reports, else
    /// they are at their stored places, their state unknown.
    pub(super) fn agent_comments(
        &self,
        args: CommentsArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<CommentsAnswer, String> {
        let (file, kind) = self.review_file(agent, &args.path)?;
        let name = file_name(&file);
        let loaded = store::load(&sidecar_path(&file), &name)
            .map_err(|e| format!("Couldn't read the comments: {e}"))?;
        let Some(review) = loaded.review else {
            return Ok(threads_page(Vec::new(), args.from, loaded.read_only));
        };
        let note = if kind == DocKind::Html {
            self.page_text(&file, &agent.name, budget)
        } else {
            let source = read_source(&file, budget.at_most(self.timings.exists))?;
            NoteText::of_note(&name, &source)
        };
        let threads = list_threads(&review, note.as_ref(), args.status);
        Ok(threads_page(threads, args.from, loaded.read_only))
    }

    /// `reply`: the agent's entry added to thread `args.id`.
    pub(super) fn agent_reply_to(
        &self,
        args: ReplyArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<ThreadAnswer, String> {
        let (path, id, text, kind) = (
            args.path.clone(),
            args.id.clone(),
            args.text.clone(),
            args.kind,
        );
        self.change_thread(
            &path,
            &id,
            (agent, budget),
            SHOW_PAGE_TO_REPLY,
            |n| reply_op(n, text, &agent.name, kind),
            |path| AgentRequest::Reply(ReplyArgs { path, ..args }),
        )
    }

    /// `resolve`: the agent's closing entry added to thread `args.id`, which resolves it.
    pub(super) fn agent_resolve(
        &self,
        args: ResolveArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<ThreadAnswer, String> {
        let (path, id, text) = (args.path.clone(), args.id.clone(), args.text.clone());
        self.change_thread(
            &path,
            &id,
            (agent, budget),
            SHOW_PAGE_TO_RESOLVE,
            |n| resolve_op(n, text, &agent.name),
            |path| AgentRequest::Resolve(ResolveArgs { path, ..args }),
        )
    }

    /// Adds the agent's entry to thread `id` of the file `raw` names: in a note or a text file,
    /// `op`'s, saved by Rust; in an HTML page, by the window showing it, which is sent `relayed`
    /// with the page's path (else the error `page_first`), once Rust has found the thread.
    fn change_thread(
        &self,
        raw: &str,
        id: &str,
        (agent, budget): (&Agent, Budget),
        page_first: &str,
        op: impl FnOnce(u32) -> ReviewOp,
        relayed: impl FnOnce(String) -> AgentRequest,
    ) -> Result<ThreadAnswer, String> {
        let (file, kind) = self.review_file(agent, raw)?;
        let name = file_name(&file);
        let n = thread_number(id).ok_or("`id` must be a thread's id, like C3.")?;
        let status = if kind == DocKind::Html {
            let window = self.window_showing(&file).ok_or(page_first)?;
            self.check_thread(&file, &name, n)?;
            self.in_page(&window, relayed(path_string(&file)), &agent.name, budget)?
                .1
        } else {
            let source = read_source(&file, budget.at_most(self.timings.exists))?;
            let (review, id) = self.save_thread(&file, &name, &source, &op(n))?;
            let status = review
                .comments()
                .find(|c| c.id == id)
                .map_or(CommentStatus::Open, Comment::effective_status);
            status
        };
        Ok(ThreadAnswer {
            id: thread_id(n),
            status,
            sidecar: self.sidecar_forms(agent, &file),
        })
    }

    /// Whether `show` may go to thread `id` of `file`: review comments are on, the file takes
    /// comments, and the thread is there.
    pub(super) fn check_comment_target(&self, file: &Path, id: &str) -> Result<(), String> {
        self.reviews_on()?;
        review_kind(file)?;
        let n = thread_number(id).ok_or("`comment` must be a thread's id, like C3.")?;
        self.check_thread(file, &file_name(file), n)
    }

    /// The file an agent's review tool names (`raw`, a Windows or WSL path, from `agent`), and its
    /// kind: a note, a text file or an HTML page, while review comments are on.
    fn review_file(&self, agent: &Agent, raw: &str) -> Result<(PathBuf, DocKind), String> {
        self.reviews_on()?;
        let file = PathBuf::from(self.agent_location(agent, raw)?.windows);
        let kind = review_kind(&file)?;
        Ok((file, kind))
    }

    /// Whether review comments are on, which every review tool needs.
    fn reviews_on(&self) -> Result<(), String> {
        if read(&self.settings).review_comments {
            Ok(())
        } else {
            Err(FEATURE_OFF.to_owned())
        }
    }

    /// Saves `op`, the agent's, to the sidecar of `file`, named `name`, read as `source`, and has
    /// each window showing the file reload its comments: the review as saved, and the thread it
    /// changed. A thread that isn't there is an error naming those that are.
    fn save_thread(
        &self,
        file: &Path,
        name: &str,
        source: &str,
        op: &ReviewOp,
    ) -> Result<(Review, u32), String> {
        let sidecar = sidecar_path(file);
        let now = iso_utc(SystemTime::now());
        match store::apply_op(&sidecar, name, source, op, &now) {
            Ok(saved) => {
                self.review_written(file);
                Ok(saved)
            }
            Err(StoreError::Op(OpError::NoSuchComment(n))) => {
                Err(no_such_thread(n, name, &thread_ids(&sidecar, name)))
            }
            Err(e) => {
                log::warn!(
                    "couldn't save an agent's comment to {}: {e}",
                    sidecar.display()
                );
                Err(e.to_string())
            }
        }
    }

    /// Whether `file`, named `name`, has thread `n`: an error naming those it has when it hasn't.
    fn check_thread(&self, file: &Path, name: &str, n: u32) -> Result<(), String> {
        let ids = thread_ids(&sidecar_path(file), name);
        if ids.contains(&n) {
            Ok(())
        } else {
            Err(no_such_thread(n, name, &ids))
        }
    }

    /// Sends `request` to the UI of `window`, showing an HTML page, which saves the agent's entry
    /// with the page's text: the thread, and its status now.
    fn in_page(
        &self,
        window: &WindowState,
        request: AgentRequest,
        agent: &str,
        budget: Budget,
    ) -> Result<(u32, CommentStatus), String> {
        match self.ask_window(window, request, Vec::new(), agent, budget)? {
            AgentReply::Commented { id, status } => Ok((id, status)),
            AgentReply::Failed { message } => Err(message),
            _ => Err(UNEXPECTED.to_owned()),
        }
    }

    /// The text of the HTML page `file`, as the window showing it last reported it, and its
    /// fingerprint; none when no window shows it, or it hasn't reported text Lectern takes.
    fn page_text(&self, file: &Path, agent: &str, budget: Budget) -> Option<NoteText> {
        let window = self.window_showing(file)?;
        let request = AgentRequest::Read(ReadArgs {
            path: Some(path_string(file)),
            ..ReadArgs::default()
        });
        let blocks = match self.ask_window(&window, request, Vec::new(), agent, budget) {
            Ok(AgentReply::PageText { blocks, .. }) if page_text_len(&blocks) <= MAX_PAGE_TEXT => {
                blocks
            }
            _ => return None,
        };
        let source = read_page_source(file).ok()?;
        Some(NoteText::of_page(&blocks, &source))
    }

    /// The window showing `file`: of those that do, the most recently focused.
    fn window_showing(&self, file: &Path) -> Option<Arc<WindowState>> {
        self.windows_by_focus().into_iter().find(|window| {
            window
                .shown_file()
                .is_some_and(|shown| same_path(&shown, file))
        })
    }

    /// The sidecar of `file` was written: each window showing the file reloads its comments, as
    /// for a change from outside.
    fn review_written(&self, file: &Path) {
        for window in self.windows_by_focus() {
            if let Some(shown) = window.shown_file().filter(|shown| same_path(shown, file)) {
                window.on_watch_event(WatchEvent::ReviewChanged(shown));
            }
        }
    }

    /// The sidecar of `file`, in both forms, in `agent`'s terms.
    fn sidecar_forms(&self, agent: &Agent, file: &Path) -> PathForms {
        self.agent_mapper(agent)
            .literal_forms(&path_string(&sidecar_path(file)))
    }
}

/// The kind of `file`, which an agent's review tool works on: a note, a text file or an HTML page,
/// not a sidecar itself. A PDF never takes comments.
fn review_kind(file: &Path) -> Result<DocKind, String> {
    let name = file_name(file);
    match doc_kind(&name) {
        Some(DocKind::Pdf) => Err(NOT_ON_A_PDF.to_owned()),
        Some(kind) if kind.takes_comments() && !is_sidecar_name(&name) => Ok(kind),
        _ => Err(NOT_A_NOTE.to_owned()),
    }
}

/// The ids of the threads in `sidecar`, the sidecar of the note named `name`; none when it can't
/// be read.
fn thread_ids(sidecar: &Path, name: &str) -> Vec<u32> {
    store::load(sidecar, name)
        .ok()
        .and_then(|loaded| loaded.review)
        .map(|review| review.comments().map(|c| c.id).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use lectern_core::agent::request::{
        CommentArgs, CommentsArgs, EntryKind, ReplyArgs, ResolveArgs, Target, ThreadFilter,
    };
    use lectern_core::agent::status::{AgentCall, AgentReply};
    use lectern_core::agent::{Agent, AgentRequest};
    use lectern_core::ipc::SettingsPatch;
    use lectern_core::review::ops::{NewAnchor, ReviewOp};
    use lectern_core::review::text::PageBlock;
    use lectern_core::review::CommentStatus;
    use serde_json::{json, Value};

    use super::super::tests::{agent_requests, quote, shown_plan, ui_answers};
    use crate::events::Target as EventTarget;
    use crate::state::paths::path_string;
    use crate::state::test_support::*;

    /// A text file: two paragraphs, on lines 1–2 and 4–5.
    const README: &str = "Readme for the vault.\nEvery word is invented.\n\nThe lanternfish survey is plain text,\nso search reads it.\n";

    fn comment(path: &Path, target: Target, text: &str) -> AgentRequest {
        AgentRequest::Comment(CommentArgs {
            path: path_string(path),
            target,
            text: text.to_owned(),
            kind: None,
        })
    }

    fn comments(path: &Path, status: Option<ThreadFilter>) -> AgentRequest {
        comments_from(path, status, None)
    }

    fn comments_from(path: &Path, status: Option<ThreadFilter>, from: Option<u32>) -> AgentRequest {
        AgentRequest::Comments(CommentsArgs {
            path: path_string(path),
            status,
            from,
        })
    }

    fn reply(path: &Path, id: &str, text: &str, kind: Option<EntryKind>) -> AgentRequest {
        AgentRequest::Reply(ReplyArgs {
            path: path_string(path),
            id: id.to_owned(),
            text: text.to_owned(),
            kind,
        })
    }

    fn resolve(path: &Path, id: &str) -> AgentRequest {
        AgentRequest::Resolve(ResolveArgs {
            path: path_string(path),
            id: id.to_owned(),
            text: None,
        })
    }

    fn sidecar_of(note: &Path) -> PathBuf {
        lectern_core::review::sidecar_path(note)
    }

    fn sidecar_text(note: &Path) -> String {
        fs::read_to_string(sidecar_of(note)).unwrap()
    }

    /// The ids of the threads `comments` answered with.
    fn ids(answer: &Value) -> Vec<String> {
        answer["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// An agent comment on a text file no window shows: Rust writes the sidecar itself, the
    /// thread headed `open` with the quote and Claude's question, never yours; a second is C2. No
    /// window is asked.
    #[test]
    fn an_agent_comment_on_a_text_file_with_no_window_open_is_saved_as_the_agents() {
        let f = fixture(profile(&[]), FakeHost::default());
        let readme = f.dir.file("notes/readme.txt", README);
        let first = f
            .app
            .agent_answer(
                comment(&readme, quote("lanternfish survey"), "Which survey?"),
                &claude(),
            )
            .unwrap();
        assert_eq!(first["id"], "C1");
        let sidecar = sidecar_of(&readme);
        assert_eq!(
            first["sidecar"]["windows"].as_str(),
            Some(path_string(&sidecar).as_str())
        );
        assert!(
            first["sidecar"]["wsl"]
                .as_str()
                .is_some_and(|wsl| wsl.ends_with("/notes/readme.txt.review.md")),
            "{first}"
        );
        let saved = sidecar_text(&readme);
        assert!(saved.contains("\n## C1 · open · L4–L5\n"), "{saved}");
        assert!(
            saved.contains("\n> lanternfish survey\n\n**Claude (question):** Which survey?\n"),
            "{saved}"
        );
        assert!(!saved.contains("**You:**"), "{saved}");
        let second = f
            .app
            .agent_answer(
                comment(&readme, Target::Line { line: 1 }, "Which vault?"),
                &Agent::named("Codex"),
            )
            .unwrap();
        assert_eq!(second["id"], "C2");
        assert!(sidecar_text(&readme).contains("\n**Codex (question):** Which vault?\n"));
        assert_eq!(fs::read_to_string(&readme).unwrap(), README);
        assert!(agent_requests(&f.host).is_empty());
        assert!(f.host.review_changes().is_empty(), "no window shows it");
    }

    /// A note a window shows: its comments reload, as for any change from outside; and the
    /// reader's Reply afterwards continues the agent's thread as theirs.
    #[test]
    fn a_comment_on_the_note_on_screen_reloads_its_comments_and_your_reply_is_yours() {
        let (f, plan) = shown_plan();
        let answer = f
            .app
            .agent_answer(comment(&plan, quote("Roll back"), "Why first?"), &claude())
            .unwrap();
        assert_eq!(answer["id"], "C1");
        assert_eq!(f.host.review_changes(), std::slice::from_ref(&plan));
        let yours = ReviewOp::Reply {
            id: 1,
            text: "It's safer.".to_owned(),
            author: None,
        };
        let payload = f.state.review_op(&path_string(&plan), yours, None).unwrap();
        // The thread the reply changed, as the window learns which an agent's operation did.
        assert_eq!(payload.changed, Some(1));
        let c1 = &payload.comments[0];
        assert_eq!(c1.status, CommentStatus::Open);
        assert_eq!(c1.entries[0].name, "Claude");
        assert_eq!(c1.entries[1].name, "You");
        assert!(
            sidecar_text(&plan)
                .contains("**Claude (question):** Why first?\n\n**You:** It's safer.\n"),
            "{}",
            sidecar_text(&plan)
        );
    }

    /// A file beside the note named as its sidecar that Lectern didn't make, and a passage that
    /// isn't there: refused, with nothing written.
    #[test]
    fn a_foreign_sidecar_or_a_missing_passage_writes_nothing() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.file("notes/x.md", "# X\n\nSome text.\n");
        let own = "# My own notes\n\n## C1 · open · L1\n> X\n";
        let sidecar = f.dir.file("notes/x.review.md", own);
        assert_eq!(
            f.app
                .agent_answer(comment(&note, quote("Some text"), "Why?"), &claude()),
            Err("This file isn't a Lectern comments file, so Lectern won't change it.".to_owned())
        );
        assert_eq!(fs::read_to_string(&sidecar).unwrap(), own);
        let other = f.dir.file("notes/y.md", "# Y\n");
        assert_eq!(
            f.app
                .agent_answer(comment(&other, quote("Some text"), "Why?"), &claude()),
            Err("That passage isn't in y.md.".to_owned())
        );
        assert!(!sidecar_of(&other).exists());
    }

    /// `comments` lists a note's threads as the Comments tab shows them, by status; a note
    /// without a sidecar has none.
    #[test]
    fn comments_lists_a_notes_threads_by_status() {
        let (f, plan) = shown_plan();
        assert_eq!(
            f.app.agent_answer(comments(&plan, None), &claude()),
            Ok(json!({"threads": [], "total": 0}))
        );
        fs::write(
            sidecar_of(&plan),
            "---\nlectern-review: 1\nnote: plan.md\n---\n\n\
             ## C1 · open · L5 · Plan › Rollout\n> Roll back\n\n**You:** Why?\n\n\
             ## C2 · open · L5\n> migration\n\n**Claude (question):** Which one?\n\n\
             ## C3 · resolved · L3\n> Rollout\n\n**You:** Rename?\n",
        )
        .unwrap();
        let open = f
            .app
            .agent_answer(comments(&plan, None), &claude())
            .unwrap();
        assert_eq!(ids(&open), ["C1", "C2"]);
        assert_eq!(open["threads"][1]["state"], "anchored");
        assert_eq!(open["threads"][1]["status"], "question");
        assert_eq!(
            open["threads"][1]["entries"][0],
            json!({"author": "agent", "name": "Claude", "kind": "question", "text": "Which one?"})
        );
        let question = f
            .app
            .agent_answer(comments(&plan, Some(ThreadFilter::Question)), &claude())
            .unwrap();
        assert_eq!(ids(&question), ["C2"]);
        let all = f
            .app
            .agent_answer(comments(&plan, Some(ThreadFilter::All)), &claude())
            .unwrap();
        assert_eq!(ids(&all), ["C1", "C2", "C3"]);
        assert!(agent_requests(&f.host).is_empty());
    }

    /// A note with more threads than one answer holds: `comments` gives them in parts, whole
    /// threads each, `next` saying where the next starts, and `from` asking for it.
    #[test]
    fn comments_gives_a_long_list_in_parts() {
        let (f, plan) = shown_plan();
        let mut sidecar = "---\nlectern-review: 1\nnote: plan.md\n---\n".to_owned();
        for id in 1..=40 {
            sidecar.push_str(&format!(
                "\n## C{id} · open · L5\n> Roll back\n\n**You:** {}\n",
                "w".repeat(2_000)
            ));
        }
        fs::write(sidecar_of(&plan), sidecar).unwrap();
        let first = f
            .app
            .agent_answer(comments(&plan, None), &claude())
            .unwrap();
        assert_eq!(first["total"], 40);
        let next = first["next"].as_u64().expect("more to come");
        assert_eq!(ids(&first).len() as u64, next);
        assert!(serde_json::to_vec(&first).unwrap().len() <= 48 * 1024);
        let next = u32::try_from(next).unwrap();
        let second = f
            .app
            .agent_answer(comments_from(&plan, None, Some(next)), &claude())
            .unwrap();
        assert_eq!(second.get("next"), None);
        let all: Vec<String> = ids(&first).into_iter().chain(ids(&second)).collect();
        let every: Vec<String> = (1..=40).map(|id| format!("C{id}")).collect();
        assert_eq!(all, every);
    }

    /// `reply` and `resolve` append the agent's entries through the store, and the status follows
    /// them; your later reply opens it again. An unknown thread is refused, naming those there are.
    #[test]
    fn reply_and_resolve_append_the_agents_entries() {
        let (f, plan) = shown_plan();
        f.app
            .agent_answer(comment(&plan, quote("Roll back"), "Why first?"), &claude())
            .unwrap();
        let replied = f
            .app
            .agent_answer(reply(&plan, "C1", "Because.", None), &claude())
            .unwrap();
        assert_eq!(replied["id"], "C1");
        assert_eq!(replied["status"], "replied");
        assert_eq!(
            replied["sidecar"]["windows"].as_str(),
            Some(path_string(&sidecar_of(&plan)).as_str())
        );
        let pushed = f
            .app
            .agent_answer(
                reply(&plan, "C1", "Not quite.", Some(EntryKind::Pushback)),
                &Agent::named("Codex"),
            )
            .unwrap();
        assert_eq!(pushed["status"], "pushback");
        let resolved = f.app.agent_answer(resolve(&plan, "C1"), &claude()).unwrap();
        assert_eq!(resolved["status"], "resolved");
        let saved = sidecar_text(&plan);
        assert!(
            saved.contains(
                "**Claude (reply):** Because.\n\n**Codex (pushback):** Not quite.\n\n\
                 **Claude (resolved):** Resolved.\n"
            ),
            "{saved}"
        );
        let yours = ReviewOp::Reply {
            id: 1,
            text: "One more thing.".to_owned(),
            author: None,
        };
        let payload = f.state.review_op(&path_string(&plan), yours, None).unwrap();
        assert_eq!(payload.comments[0].status, CommentStatus::Open);
        let before = sidecar_text(&plan);
        assert_eq!(
            f.app
                .agent_answer(reply(&plan, "C9", "Hello?", None), &claude()),
            Err("There's no thread C9 in plan.md. Its only thread is C1.".to_owned())
        );
        assert_eq!(
            f.app.agent_answer(resolve(&plan, "C9"), &claude()),
            Err("There's no thread C9 in plan.md. Its only thread is C1.".to_owned())
        );
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        assert_eq!(
            f.app.agent_answer(resolve(&notes, "C1"), &claude()),
            Err("There's no thread C1 in notes.md: it has no comments yet.".to_owned())
        );
        assert_eq!(sidecar_text(&plan), before);
        assert!(!sidecar_of(&notes).exists());
    }

    /// An invented page whose text its bridge reports as `report_page` has it.
    const REPORT: &str = "<!doctype html>\n<h1>Report</h1>\n<p>Sign-ups rose steadily.</p>\n";

    fn report_page() -> Vec<PageBlock> {
        vec![
            PageBlock {
                text: "Report".to_owned(),
                heading_level: Some(1),
            },
            PageBlock {
                text: "Sign-ups rose steadily.".to_owned(),
                heading_level: None,
            },
        ]
    }

    /// An HTML page's comment is added by the window showing it, with the page's live text, the
    /// agent its author; a page no window shows can't be commented on, nor by a line.
    #[test]
    fn a_pages_comment_goes_to_the_window_showing_it() {
        let (f, _) = shown_plan();
        let report = f.dir.file("vault/report.html", REPORT);
        let not_shown = comment(&report, quote("rose steadily"), "Weekly?");
        assert_eq!(
            f.app.agent_answer(not_shown.clone(), &claude()),
            Err("Show the page first, then comment.".to_owned())
        );
        let _ = f.state.open_document(&path_string(&report));
        assert_eq!(
            f.app.agent_answer(
                comment(&report, Target::Line { line: 3 }, "Weekly?"),
                &claude()
            ),
            Err("report.html is an HTML page: give a heading or a quote.".to_owned())
        );
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Commented {
                id: 1,
                status: CommentStatus::Question,
            }],
        );
        let answer = f.app.agent_answer(not_shown, &claude()).unwrap();
        let (target, call): (EventTarget, AgentCall) = answering.join().unwrap();
        assert_eq!(target, EventTarget::Window("main".to_owned()));
        assert_eq!(call.agent, "Claude");
        let AgentRequest::Comment(args) = call.request else {
            panic!("not a comment: {:?}", call.request);
        };
        assert_eq!(args.path, path_string(&report));
        assert_eq!(args.target, quote("rose steadily"));
        assert_eq!(answer["id"], "C1");
        assert_eq!(
            answer["sidecar"]["windows"].as_str(),
            Some(path_string(&sidecar_of(&report)).as_str())
        );
    }

    /// A page's reply and resolve go to its window too, once Rust has checked the thread is
    /// there; its threads are found in the text the window reports, or, no window showing it, are
    /// at their stored places in an unknown state.
    #[test]
    fn a_pages_threads_are_worked_through_its_window() {
        let (f, _) = shown_plan();
        let report = f.dir.file("vault/report.html", REPORT);
        fs::write(
            sidecar_of(&report),
            "---\nlectern-review: 1\nnote: report.html\n---\n\n\
             ## C1 · open · L3 · Report\n> rose steadily\n\n**You:** Weekly?\n",
        )
        .unwrap();
        let stored = f
            .app
            .agent_answer(comments(&report, None), &claude())
            .unwrap();
        assert_eq!(stored["threads"][0]["state"], "unknown");
        assert_eq!(stored["threads"][0]["startLine"], 3);
        assert!(agent_requests(&f.host).is_empty());

        let _ = f.state.open_document(&path_string(&report));
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::PageText {
                blocks: report_page(),
                truncated: false,
            }],
        );
        let live = f
            .app
            .agent_answer(comments(&report, None), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        assert!(matches!(call.request, AgentRequest::Read(_)), "{call:?}");
        assert_eq!(live["threads"][0]["state"], "anchored");

        let sent = agent_requests(&f.host).len();
        assert_eq!(
            f.app
                .agent_answer(reply(&report, "C4", "Hello?", None), &claude()),
            Err("There's no thread C4 in report.html. Its only thread is C1.".to_owned())
        );
        assert_eq!(
            agent_requests(&f.host).len(),
            sent,
            "the window isn't asked"
        );
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Commented {
                id: 1,
                status: CommentStatus::Replied,
            }],
        );
        let replied = f
            .app
            .agent_answer(reply(&report, "C1", "Weekly.", None), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        let AgentRequest::Reply(args) = call.request else {
            panic!("not a reply: {:?}", call.request);
        };
        assert_eq!((args.id.as_str(), args.text.as_str()), ("C1", "Weekly."));
        assert_eq!(replied["id"], "C1");
        assert_eq!(replied["status"], "replied");
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Failed {
                message: "The page hasn't loaded yet; try again in a moment.".to_owned(),
            }],
        );
        assert_eq!(
            f.app.agent_answer(resolve(&report, "C1"), &claude()),
            Err("The page hasn't loaded yet; try again in a moment.".to_owned())
        );
        let (_, call) = answering.join().unwrap();
        assert!(matches!(call.request, AgentRequest::Resolve(_)), "{call:?}");
    }

    /// PDFs take no comments, and while review comments are off, no review tool works.
    #[test]
    fn pdfs_and_review_comments_turned_off_refuse_every_review_tool() {
        let (f, plan) = shown_plan();
        let brief = f.dir.file("vault/brief.pdf", "%PDF-1.4\n");
        let no_pdf = Err("PDFs don't take comments.".to_owned());
        for request in [
            comment(&brief, Target::Page { page: 1 }, "Why?"),
            comments(&brief, None),
            reply(&brief, "C1", "Why?", None),
            resolve(&brief, "C1"),
        ] {
            assert_eq!(f.app.agent_answer(request, &claude()), no_pdf);
        }
        f.state.set_settings(SettingsPatch {
            review_comments: Some(false),
            ..SettingsPatch::default()
        });
        let off = Err("Review comments are turned off in Preferences.".to_owned());
        for request in [
            comment(&plan, quote("Roll back"), "Why?"),
            comments(&plan, None),
            reply(&plan, "C1", "Why?", None),
            resolve(&plan, "C1"),
        ] {
            assert_eq!(f.app.agent_answer(request, &claude()), off);
        }
        assert!(!sidecar_of(&plan).exists());
        assert!(agent_requests(&f.host).is_empty());
    }

    /// `show` at a comment thread: the window is sent the thread to show, once Rust has found it;
    /// one that isn't there is refused before anything opens.
    #[test]
    fn a_comment_target_needs_its_thread() {
        let (f, plan) = shown_plan();
        let thread = |id: &str| Target::Comment {
            comment: id.to_owned(),
        };
        let show = |id: &str| super::super::tests::show_at(&plan, thread(id));
        assert_eq!(
            f.app.agent_answer(show("C1"), &claude()),
            Err("There's no thread C1 in plan.md: it has no comments yet.".to_owned())
        );
        assert!(agent_requests(&f.host).is_empty());
        f.state
            .review_op(
                &path_string(&plan),
                ReviewOp::Add {
                    anchor: NewAnchor {
                        start_line: 5,
                        end_line: 5,
                        quote: "Roll back".to_owned(),
                        prefix: String::new(),
                        text_start: None,
                    },
                    text: "Why?".to_owned(),
                    author: None,
                },
                None,
            )
            .unwrap();
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f.app.agent_answer(show("C1"), &claude()).unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!((answer.get("comments"), answer.get("note")), (None, None));
        let AgentRequest::Show(args) = call.request else {
            panic!("not a show: {:?}", call.request);
        };
        assert_eq!(args.target, Some(thread("C1")));
        assert!(call.places.is_empty());
        assert_eq!(
            f.app.agent_answer(show("C2"), &claude()),
            Err("There's no thread C2 in plan.md. Its only thread is C1.".to_owned())
        );
    }

    /// A comment thread shown in a window whose comments the reader has hidden: the window still
    /// goes to its passage, and the answer says the comments are hidden there.
    #[test]
    fn a_thread_shown_where_comments_are_hidden_says_so() {
        let (f, plan) = shown_plan();
        f.app
            .agent_answer(comment(&plan, quote("Roll back"), "Why first?"), &claude())
            .unwrap();
        f.state.set_settings(SettingsPatch {
            comments_visible: Some(false),
            ..SettingsPatch::default()
        });
        let show = super::super::tests::show_at(
            &plan,
            Target::Comment {
                comment: "C1".to_owned(),
            },
        );
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f.app.agent_answer(show, &claude()).unwrap();
        answering.join().unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["comments"], "hidden");
        assert_eq!(
            answer["note"],
            "Comments are hidden in that window; the passage is shown."
        );
        // Any other target says nothing of the comments.
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f
            .app
            .agent_answer(
                super::super::tests::show_at(&plan, quote("Roll back")),
                &claude(),
            )
            .unwrap();
        answering.join().unwrap();
        assert_eq!(answer.get("comments"), None);
    }
}
