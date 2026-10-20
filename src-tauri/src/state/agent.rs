//! What the app answers agents with (`agent.rs` serves the pipe):
//!
//! - **`status`:** every window's view, which each window's UI reports (`agent-request`,
//!   answered by the command `agent_reply`).
//! - **`show`:** the file goes to the window `route_show` picks. A window whose UI is up decides
//!   itself whether to ask the reader first (the bar) and answers `shown` or `asked`; a window
//!   still starting, or a new one, opens the file first thing. Such a show answers once the file
//!   has opened (`shown`, or the open's error), `replaced` when a newer agent's took its place
//!   first, or `waiting` when the window hasn't started in time; `status` has what became of it
//!   either way. A window starting with the user's launch (the command line's file, or a second
//!   launch's) opens that instead, which an agent's never takes the place of: the show goes to its
//!   UI as to one that's up, once it is, and the UI shows the user's file first; `waiting` when it
//!   hasn't started in time, and `status` has what its UI answers then. A newer show held so takes
//!   its place, as at the ask bar (`replaced`).
//!   The window then comes forward as `agentFocus` has it. With a `wait`, an `asked` waits that
//!   long for what became of the request, which the UI sends once it is settled
//!   (`AgentReply::Answered`). A window that turns to another workspace meanwhile (its page
//!   reloads) dismisses it; one that closes is the tool's error. With no target, a `#anchor` after
//!   the path is the heading it names, and a PDF's `#page=N` its page.
//! - **`workspace`:** switches to a workspace, bringing its window forward or reopening it in a
//!   new one, or creates one in a new window; an open window never changes workspace.
//! - **Targets:** a `show`'s target in a note or a text file is found by core, in the file as it
//!   is on disk, before anything opens: one that isn't there is the tool's error. A file the open
//!   wouldn't read (a text file over 20 MB) is refused unread, as the open refuses it. The window is
//!   sent it with where it is (`AgentCall::places`); a window still starting gets it with the file
//!   it opens first thing (`StartupPayload::agent_target`). An HTML page's target goes to the
//!   window as it is, to find in what the page reports, waiting for a page still loading as long
//!   as the pipe gives it (`AgentCall::within_ms`): one that isn't there is the tool's error, and
//!   one the page hadn't loaded in time for, or that a starting window will find, is `pending`. A
//!   PDF's target is its page.
//! - **`highlight`:** goes to the window showing the file (the most recently focused, and by
//!   default the file it shows), with where core found each target in a note or a text file. No
//!   window comes forward. A PDF can't be painted: the tool's error. Once the note changes, the
//!   window asks core where its highlights are now (`WindowState::agent_places`).
//! - **`read`:** a note's or a text file's text, as shown, read by Rust from the file; an HTML
//!   page's from what its window's page reported, read as incomplete when its bridge cut it short
//!   at its limit. A PDF has none.
//! - **The file shown** is the one the window's UI last opened, until it leaves it for none
//!   (`WindowState::leave_document`), or it goes missing while open (`document_missing`, still
//!   watched for its return): then no agent acts on it. An open the UI dropped on its way (an
//!   agent's, that a newer show took the place of) never becomes it (`keep_document`).
//!
//! Each answers within the pipe's time (`ANSWER_WITHIN`, and the reader's `wait`), a second to
//! spare: each step that waits waits only as long as is left of it (`Budget`).
//!
//! An agent's path on a network share is taken only when Lectern already trusts the share's host:
//! a library's, a path mapping's, WSL's, or one the user opened a file from (`agent_location`); a
//! new workspace's folders only on one the user configured, not one opened from this session. An
//! agent's open never adds one. That differs from the user's own opens (a launch, Explorer, the
//! file dialog), which make the host trusted for the session: an agent's path may come from text a
//! note fed it, and reaching a new host would hand that host the user's credentials.
//!
//! The review tools (`comment`, `comments`, `reply`, `resolve`) and `show`'s comment target are
//! in `review`.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::agent::paths::{agent_path, AgentPath};
use lectern_core::agent::read::{read_page, ReadAnswer, PAGE_TEXT_CUT};
use lectern_core::agent::request::{
    HighlightArgs, ReadArgs, ShowArgs, Target, WorkspaceAction, WorkspaceArgs,
};
use lectern_core::agent::show::{
    no_such_workspace, AgentRoute, FocusStep, ShowAnswer, ShowState, TargetState, WorkspaceAnswer,
};
use lectern_core::agent::status::{
    window_status, AgentCall, AgentReply, AgentSettings, AskAnswer, Status, WindowView,
    ANSWERS_KEPT,
};
use lectern_core::agent::target::{
    check_target, pdf_suffix_page, resolve_target, show_anchor, HighlightAnswer, ShowAt, TextPlace,
};
use lectern_core::agent::tools::TARGETS_MAX;
use lectern_core::agent::{Agent, AgentRequest, ANSWER_WITHIN};
use lectern_core::ipc::{AgentFocus, OpenRequest, PathForms};
use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::DocKind;
use lectern_core::review::text::{page_text_len, TextMap, MAX_PAGE_TEXT};
use serde::Serialize;
use serde_json::{json, Value};

use super::doc::{check_size, opened_kind, read_text as read_file, stat_doc};
use super::open_queue::{AgentOffer, ForUi, OpenQueue, Settled};
use super::paths::{normalize_root, path_string, same_path};
use super::sync::{lock, read};
use super::trust::Trust;
use super::windows::is_folder;
use super::{App, WindowState};
use crate::events::UiEvent;

mod review;

/// How long `status` waits for the windows' UIs to say what they show. A window that hasn't
/// answered by then is reported from what Rust knows of it: its workspace and its file.
const VIEW_WAIT: Duration = Duration::from_secs(3);

/// How long `show` waits for the window's UI to open the file or ask the reader, and
/// `highlight` and `read` for it to answer.
const UI_WAIT: Duration = Duration::from_secs(5);

/// What `show` and `workspace` keep of the pipe's time (`ANSWER_WITHIN`) to answer in.
const ANSWER_SPARE: Duration = Duration::from_secs(1);

/// The tool error when the window a request went to closes before it is settled.
const CLOSED: &str = "That window has closed.";

/// The tool error when a window's UI answers with something else than what was asked.
const UNEXPECTED: &str = "The window gave an answer Lectern didn't expect.";

/// The tool error for `read` of a PDF.
const NO_PDF_TEXT: &str = "PDF text isn't available.";

/// The tool error for `highlight` in a PDF, which can't be painted.
const NO_PDF_HIGHLIGHT: &str = "PDFs can't be highlighted; use show with a page target.";

/// The requests sent to windows' UIs that wait for an answer, by id, with the window each went
/// to; the `show`s whose agents wait for what becomes of them, by the same id; and the held `show`s
/// that answered `waiting`, whose UI's answer `status` keeps once it comes (`later`).
#[derive(Default)]
pub struct AgentCalls {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, (String, Sender<AgentReply>)>>,
    asks: Mutex<HashMap<u64, (String, Sender<Settled>)>>,
    /// The latest `ANSWERS_KEPT`, the oldest first. Changed only under `waiting`'s lock, so an
    /// answer finds either its waiter or its entry here.
    late: Mutex<VecDeque<Late>>,
    /// Runs as `later` begins, before it takes a held show's waiters: a test's way into the moment
    /// between a wait's end and its clean-up.
    #[cfg(test)]
    before_later: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// A held `show` that answered `waiting` (`AgentCalls::later`): its request's id, the window it
/// went to, its file, and the outcomes that window keeps for `status`, where its UI's answer goes.
struct Late {
    id: u64,
    label: String,
    path: String,
    opens: Arc<OpenQueue>,
}

impl Late {
    /// Keeps its UI's answer, `reply`, for `status`: shown, failed with why, or replaced. What
    /// becomes of an ask is the window's own to keep, once the reader has answered.
    fn settle(self, reply: AgentReply) {
        let settled = match reply {
            AgentReply::Shown { .. } => (AskAnswer::Shown, None),
            AgentReply::Failed { message } => (AskAnswer::Failed, Some(message)),
            AgentReply::Replaced => (AskAnswer::Replaced, None),
            _ => return,
        };
        self.opens.settled_late(&self.path, settled);
    }
}

impl AgentCalls {
    /// A new request to the window `label`: its id, and where its answer arrives.
    fn open(&self, label: &str) -> (u64, Receiver<AgentReply>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        lock(&self.waiting).insert(id, (label.to_owned(), sender));
        (id, receiver)
    }

    /// Where what becomes of the `show` `id`, sent to the window `label`, arrives once the window
    /// has asked the reader about it.
    fn await_answer(&self, label: &str, id: u64) -> Receiver<Settled> {
        let (sender, receiver) = mpsc::channel();
        lock(&self.asks).insert(id, (label.to_owned(), sender));
        receiver
    }

    /// Hands `reply` to whoever waits on request `id`, if the window `label` was asked it; false
    /// when no one waits (it took too long, or the id isn't one sent to that window). What became
    /// of a `show` goes to whoever waits for that. A held `show` that answered `waiting` has its
    /// UI's answer kept for `status` instead (`later`).
    fn answer(&self, label: &str, id: u64, reply: AgentReply) -> bool {
        if let AgentReply::Answered { answer, message } = reply {
            return take_for(&mut lock(&self.asks), label, id)
                .is_some_and(|s| s.send((answer, message)).is_ok());
        }
        let mut waiting = lock(&self.waiting);
        if let Some(sender) = take_for(&mut waiting, label, id) {
            return sender.send(reply).is_ok();
        }
        let late = {
            let mut late = lock(&self.late);
            let at = late.iter().position(|l| l.id == id && l.label == label);
            at.and_then(|at| late.remove(at))
        };
        drop(waiting);
        match late {
            Some(late) => {
                late.settle(reply);
                true
            }
            None => false,
        }
    }

    /// The held `show` `late.id`, whose answer on `reply` stopped being waited for (it answers
    /// `waiting`): what its UI answers is kept for `status` once it comes (`answer`), the latest
    /// `ANSWERS_KEPT`. An answer that came as the wait ended is handed back instead, to answer
    /// with, and the show's wait for the reader's choice, if it has one, stays.
    fn later(&self, late: Late, reply: &Receiver<AgentReply>) -> Option<AgentReply> {
        #[cfg(test)]
        if let Some(hook) = lock(&self.before_later).take() {
            hook();
        }
        let mut waiting = lock(&self.waiting);
        waiting.remove(&late.id);
        // Sent under the same lock, before the waiter went. The show answers with it, and, asked,
        // goes on to wait for the reader: what they choose still reaches it.
        if let Ok(came) = reply.try_recv() {
            return Some(came);
        }
        lock(&self.asks).remove(&late.id);
        let mut kept = lock(&self.late);
        kept.push_back(late);
        if kept.len() > ANSWERS_KEPT {
            kept.pop_front();
        }
        None
    }

    /// The held `show` `id`, for the window `label`, is replaced by a newer one before its UI had
    /// it, which the window's outcomes keep already: its waiter hears so, if it still waits.
    fn replaced(&self, label: &str, id: u64) {
        let mut waiting = lock(&self.waiting);
        if let Some(sender) = take_for(&mut waiting, label, id) {
            // Gone when it stopped waiting.
            let _ = sender.send(AgentReply::Replaced);
            return;
        }
        lock(&self.late).retain(|late| late.id != id);
    }

    /// Stops waiting on request `id`, and on what becomes of it.
    fn forget(&self, id: u64) {
        lock(&self.waiting).remove(&id);
        lock(&self.asks).remove(&id);
    }

    /// Stops waiting on every request sent to the window `label`, and on what becomes of them, as
    /// its state is let go of (`WindowState::retire`): whoever waits on one wakes at once, finding
    /// nothing will answer, and the held shows that answered `waiting` there are let go of too.
    pub(super) fn forget_window(&self, label: &str) {
        let mut waiting = lock(&self.waiting);
        waiting.retain(|_, (asked, _)| asked != label);
        lock(&self.asks).retain(|_, (asked, _)| asked != label);
        lock(&self.late).retain(|late| late.label != label);
    }
}

/// The time a `show` or `workspace` has to act in: the pipe's (`ANSWER_WITHIN` from when the
/// request came, the reader's `wait` aside), less a second to answer in. Each step that waits
/// (the file check, the window's UI, a new window showing, the focus) waits only as long as is
/// left, so together they never outlast the pipe.
#[derive(Clone, Copy, Debug)]
struct Budget(Instant);

impl Budget {
    /// The pipe's, for a request that has just come.
    fn new() -> Self {
        Self(Instant::now() + ANSWER_WITHIN - ANSWER_SPARE)
    }

    /// `most`, or what is left when that's less.
    fn at_most(self, most: Duration) -> Duration {
        most.min(self.left())
    }

    fn left(self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

/// The sender waiting in `waiting` on `id`, taken out, if the window `label` was asked it.
fn take_for<T>(
    waiting: &mut HashMap<u64, (String, Sender<T>)>,
    label: &str,
    id: u64,
) -> Option<Sender<T>> {
    if !waiting.get(&id).is_some_and(|(asked, _)| asked == label) {
        return None;
    }
    waiting.remove(&id).map(|(_, sender)| sender)
}

/// Where a `show` went: a window whose UI is up (or will be, once it has opened the user's launch),
/// or one that opens the file as it starts (a new window when `new`).
enum Placed {
    Ready(Arc<WindowState>),
    Starting {
        label: String,
        new: bool,
        reopened: bool,
        settled: Receiver<Settled>,
    },
}

impl App {
    /// The answer to the request of `agent` (its name as the reader sees it, and the WSL
    /// distribution it runs in, whose terms its paths are read and answered in) for one of the nine
    /// tools; a tool error is a sentence for the agent.
    pub fn agent_answer(
        self: &Arc<Self>,
        request: AgentRequest,
        agent: &Agent,
    ) -> Result<Value, String> {
        match request {
            AgentRequest::Status => to_value(&self.agent_status(agent)),
            AgentRequest::Show(args) => to_value(&self.agent_show(args, agent, Budget::new())?),
            AgentRequest::Workspace(args) => {
                to_value(&self.agent_workspace(args, agent, Budget::new())?)
            }
            AgentRequest::Highlight(args) => {
                to_value(&self.agent_highlight(args, agent, Budget::new())?)
            }
            AgentRequest::Read(args) => to_value(&self.agent_read(args, agent, Budget::new())?),
            AgentRequest::Comment(args) => {
                to_value(&self.agent_comment(args, agent, Budget::new())?)
            }
            AgentRequest::Comments(args) => {
                to_value(&self.agent_comments(args, agent, Budget::new())?)
            }
            AgentRequest::Reply(args) => {
                to_value(&self.agent_reply_to(args, agent, Budget::new())?)
            }
            AgentRequest::Resolve(args) => {
                to_value(&self.agent_resolve(args, agent, Budget::new())?)
            }
            // The connection answers it; nothing reaches here.
            AgentRequest::Hello { .. } => Ok(json!({})),
        }
    }

    /// The window `label`'s UI answers the request `id` it was sent. False when nothing waits on
    /// it any more.
    pub fn agent_reply(&self, label: &str, id: u64, reply: AgentReply) -> bool {
        self.agent_calls.answer(label, id, reply)
    }

    /// Every window, the most recently focused first, as its UI reports it (else as Rust knows
    /// it), with the closed workspaces and the agent settings; its paths in `agent`'s terms.
    pub fn agent_status(&self, agent: &Agent) -> Status {
        let windows = self.windows_by_focus();
        // Every window is asked at once, then each answer awaited until the one deadline.
        let asked: Vec<(Arc<WindowState>, u64, Receiver<AgentReply>)> = windows
            .into_iter()
            .map(|window| {
                let (id, answer) = self.agent_calls.open(window.label());
                window.emit(UiEvent::AgentRequest(AgentCall {
                    id,
                    request: AgentRequest::Status,
                    agent: agent.name.clone(),
                    places: Vec::new(),
                    within_ms: None,
                }));
                (window, id, answer)
            })
            .collect();
        let deadline = Instant::now() + VIEW_WAIT;
        let mapper = self.agent_mapper(agent);
        let windows = asked
            .into_iter()
            .map(|(window, id, answer)| {
                let mut view =
                    match answer.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(AgentReply::View(view)) => view,
                        Ok(AgentReply::Failed { message }) => {
                            log::warn!("{} couldn't say what it shows: {message}", window.label());
                            window.known_view()
                        }
                        Ok(_) => {
                            log::warn!("{} answered status with something else", window.label());
                            window.known_view()
                        }
                        Err(_) => {
                            self.agent_calls.forget(id);
                            window.known_view()
                        }
                    };
                // What became of the requests the window opened as it started, before its UI was
                // up to keep any: older than those its UI keeps.
                view.answers.splice(0..0, window.opens.answers());
                window_status(window.label(), self.workspace_name(&window), view, &mapper)
            })
            .collect();
        let shown = self.shown();
        let closed_workspaces = read(&self.workspaces)
            .items
            .iter()
            .filter(|ws| !shown.contains_key(&ws.id))
            .map(|ws| ws.name.clone())
            .collect();
        let settings = read(&self.settings);
        Status {
            windows,
            closed_workspaces,
            settings: AgentSettings {
                agent_opens: settings.agent_opens,
                agent_focus: settings.agent_focus,
            },
        }
    }

    /// `show`: the file `args.path` (a Windows or WSL path) in the window `route_show` picks, at
    /// its target (when none is given, what follows the path: `show_at_path`), within `budget`.
    /// It must be a file, on a share only when Lectern already trusts its host, which is checked
    /// first, and its target must be there (`show_at`) before anything opens or asks. A comment
    /// thread shown in a window whose comments are hidden is answered as such
    /// (`ShowAnswer::comments_hidden`): the window went to its passage alone.
    fn agent_show(
        self: &Arc<Self>,
        args: ShowArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<ShowAnswer, String> {
        let path = self.agent_location(agent, &args.path)?;
        let file = PathBuf::from(&path.windows);
        check_file(&file, budget.at_most(self.timings.exists))?;
        let at = match args.target {
            Some(target) => Some(self.show_at(&file, target, budget)?),
            None => self.show_at_path(&file, &path, budget)?,
        };
        let thread = matches!(
            at,
            Some(ShowAt {
                target: Target::Comment { .. },
                ..
            })
        );
        // An HTML page's passage, which its window finds once the page has loaded.
        let found_by_page = at.as_ref().is_some_and(|at| {
            at.place.is_none()
                && matches!(
                    at.target,
                    Target::Heading { .. } | Target::Quote { .. } | Target::Anchor { .. }
                )
        });
        let front = read(&self.settings).agent_focus == AgentFocus::Front;
        let request = OpenRequest {
            path: path.windows.clone(),
            t0_ms: None,
            folder: false,
        };
        let placed = {
            let _life = lock(&self.lifecycle);
            let route = self.route_agent(&file, args.workspace.as_deref(), args.new_window)?;
            let (label, new, reopened) = match route {
                AgentRoute::Window(label) => (label, false, false),
                AgentRoute::Reopen(id) => (self.bind_window(Some(id), front), true, true),
                AgentRoute::NewWindow => (self.bind_window(None, front), true, false),
            };
            let window = self.window(&label).ok_or(CLOSED)?;
            // A window whose UI isn't up yet opens the file first thing, as it would a launch's,
            // but trusting nothing; unless it starts with the user's launch, which goes first: the
            // request then goes to its UI, as to one that's up.
            match window.opens.offer_from_agent(request, at.clone()) {
                AgentOffer::Held(settled) => Placed::Starting {
                    label,
                    new,
                    reopened,
                    settled,
                },
                AgentOffer::ToUi => Placed::Ready(window),
            }
        };
        let (label, state, target, focus) = match placed {
            Placed::Starting {
                label,
                new,
                reopened,
                settled,
            } => {
                if new {
                    self.build(&label, front)?;
                    if reopened {
                        self.workspaces_changed();
                    }
                }
                let focus = self.agent_focus(&label, budget);
                let state = self.opened_as_it_starts(&label, &settled, budget)?;
                // Opened as the window starts: a page's passage is found once it has loaded.
                let target =
                    (state == ShowState::Shown && found_by_page).then_some(TargetState::Pending);
                (label, state, target, focus)
            }
            Placed::Ready(window) => {
                let (target, places) = match at {
                    Some(ShowAt { target, place }) => (Some(target), place.into_iter().collect()),
                    None => (None, Vec::new()),
                };
                let shown = ShowArgs {
                    path: path.windows.clone(),
                    workspace: None,
                    target,
                    new_window: false,
                    wait: args.wait,
                };
                let label = window.label().to_owned();
                let (state, target, focus) =
                    self.show_in(&window, shown, places, &agent.name, budget)?;
                (label, state, target, focus)
            }
        };
        let window = self.window(&label);
        let workspace = window.as_ref().and_then(|w| self.workspace_name(w));
        // A thread's passage is shown in a window whose comments are hidden: the answer says so.
        let hidden = thread
            && state == ShowState::Shown
            && window.is_some_and(|w| !w.settings().comments_visible);
        let answer = ShowAnswer {
            state,
            file: path.forms(),
            window: label,
            workspace,
            focus,
            target,
            comments: None,
            note: None,
        };
        Ok(if hidden {
            answer.comments_hidden()
        } else {
            answer
        })
    }

    /// Asks the UI of `window`, which is up, to show `args.path`, at its target, which is at
    /// `places` in a note or a text file. It opens it, or asks the reader first; either way the
    /// window then comes forward. The UI is told how long it has to answer (`within_ms`), which an
    /// HTML page's target may take to load: one that isn't there is the tool's error, and one the
    /// page hadn't loaded in time for is `pending`. Asked, with a `wait`, what became of it is
    /// waited for that long. A window let go of before it settles the request (`let_go_of`) ends
    /// the wait at once. A window still starting with the user's launch gets the request once its
    /// UI is up (`OpenQueue::to_ui`), with as long to answer as is left, and is waited for that
    /// long: `waiting` when it hasn't answered by then, as for any window that hasn't started in
    /// time, and `status` has what it answers later (`AgentCalls::later`). The newest held so
    /// wins, as at the ask bar: the one it takes the place of hears it was replaced.
    fn show_in(
        &self,
        window: &WindowState,
        args: ShowArgs,
        places: Vec<TextPlace>,
        agent: &str,
        budget: Budget,
    ) -> Result<(ShowState, Option<TargetState>, Option<FocusStep>), String> {
        let label = window.label();
        let wait = Duration::from_secs(args.wait.into());
        let (id, reply) = self.agent_calls.open(label);
        // Before the UI hears of it, so an answer however quick finds its waiter.
        let answered = (!wait.is_zero()).then(|| self.agent_calls.await_answer(label, id));
        // Let go of already, its requests went before these came: nothing would answer them.
        if window.is_retired() {
            self.agent_calls.forget(id);
            return Ok((self.let_go_of(label)?, None, None));
        }
        let within = budget.at_most(UI_WAIT);
        let path = args.path.clone();
        let call = AgentCall {
            id,
            request: AgentRequest::Show(args),
            agent: agent.to_owned(),
            places,
            within_ms: Some(u32::try_from(within.as_millis()).unwrap_or(u32::MAX)),
        };
        let held = match window.opens.to_ui(&path, call, budget.0) {
            ForUi::Now(call) => {
                window.emit(UiEvent::AgentRequest(call));
                false
            }
            ForUi::Held { replaced } => {
                if let Some(older) = replaced {
                    self.agent_calls.replaced(label, older);
                }
                true
            }
        };
        let received = match reply.recv_timeout(if held { budget.left() } else { within }) {
            // Its UI gets it once up all the same, and what it answers then is kept for `status`;
            // unless that came as the wait ended.
            Err(RecvTimeoutError::Timeout) if held => {
                let late = Late {
                    id,
                    label: label.to_owned(),
                    path,
                    opens: Arc::clone(&window.opens),
                };
                match self.agent_calls.later(late, &reply) {
                    Some(came) => Ok(came),
                    None => return Ok((ShowState::Waiting, None, None)),
                }
            }
            received => received,
        };
        let mut target = None;
        let state = match received {
            Ok(AgentReply::Shown { target: pending }) => {
                target = pending;
                ShowState::Shown
            }
            Ok(AgentReply::Asked) => ShowState::Asked,
            Ok(AgentReply::Replaced) => ShowState::Replaced,
            Ok(AgentReply::Failed { message }) => {
                self.agent_calls.forget(id);
                return Err(message);
            }
            Ok(_) => {
                self.agent_calls.forget(id);
                return Err(UNEXPECTED.to_owned());
            }
            Err(RecvTimeoutError::Timeout) => {
                self.agent_calls.forget(id);
                return Err("The window didn't answer in time.".to_owned());
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.agent_calls.forget(id);
                return Ok((self.let_go_of(label)?, None, None));
            }
        };
        let focus = self.agent_focus(label, budget);
        let state = match (state, answered) {
            (ShowState::Asked, Some(answered)) => {
                let settled = match answered.recv_timeout(wait) {
                    Ok((answer, message)) => ShowState::settled(answer, message),
                    Err(RecvTimeoutError::Timeout) => Ok(ShowState::Waiting),
                    Err(RecvTimeoutError::Disconnected) => self.let_go_of(label),
                };
                self.agent_calls.forget(id);
                settled?
            }
            (state, _) => {
                self.agent_calls.forget(id);
                state
            }
        };
        Ok((state, target, focus))
    }

    /// What a `show` answers when the window `label` let go of it before settling it (its state
    /// retired): the window turned to another workspace, its page reloading, which dismisses it;
    /// or it closed, which is the tool's error.
    fn let_go_of(&self, label: &str) -> Result<ShowState, String> {
        match self.window(label) {
            Some(_) => Ok(ShowState::Dismissed),
            None => Err(CLOSED.to_owned()),
        }
    }

    /// What a `show` held for the window `label` to open as it starts answers, once it has
    /// (`settled`), within `budget`: `shown`; the open's error, for a file it couldn't open;
    /// `replaced`, when a newer request took its place first. A window that hasn't started in time
    /// answers `waiting`, and `status` has what became of it once it has, as for a request the
    /// reader hasn't answered.
    fn opened_as_it_starts(
        &self,
        label: &str,
        settled: &Receiver<Settled>,
        budget: Budget,
    ) -> Result<ShowState, String> {
        match settled.recv_timeout(budget.left()) {
            Ok((answer, message)) => ShowState::settled(answer, message),
            Err(RecvTimeoutError::Timeout) => Ok(ShowState::Waiting),
            Err(RecvTimeoutError::Disconnected) => self.let_go_of(label),
        }
    }

    /// Where a `show` of `file` goes in it, `target`, which the file must take (`check_target`).
    /// In a note or a text file, core finds it in the file as it is on disk, read within `budget`:
    /// one that isn't there is an error, a sentence for the agent. An HTML page's target is the
    /// window's to find, and a PDF's is its page. A comment thread must be there (it is the
    /// window's to show).
    fn show_at(&self, file: &Path, target: Target, budget: Budget) -> Result<ShowAt, String> {
        let kind = opened_kind(file);
        let name = file_name(file);
        check_target(kind, &target, &name)?;
        if let Target::Comment { comment } = &target {
            self.check_comment_target(file, comment)?;
            return Ok(ShowAt {
                target,
                place: None,
            });
        }
        let place = match kind {
            DocKind::Markdown | DocKind::Text => {
                let (text, source) = self.note_text(file, &name, budget)?;
                Some(resolve_target(&text, &source, &name, &target)?)
            }
            DocKind::Html | DocKind::Pdf => None,
        };
        Ok(ShowAt { target, place })
    }

    /// Where a `show` of `file`, given no target, goes for what follows its path (`path`): its
    /// `:12` line, else the heading its `#anchor` names (`show_anchored`); in a PDF, the page its
    /// `#page=N` names, as a page target would (`pdf_suffix_page`), and anything else after it is
    /// the tool's error. Nowhere in particular when nothing follows the path.
    fn show_at_path(
        &self,
        file: &Path,
        path: &AgentPath,
        budget: Budget,
    ) -> Result<Option<ShowAt>, String> {
        if path.line.is_none() && path.anchor.is_none() {
            return Ok(None);
        }
        let target = if opened_kind(file) == DocKind::Pdf {
            pdf_suffix_page(path.anchor.as_deref(), &file_name(file))?
        } else if let Some(line) = path.line {
            Target::Line { line }
        } else {
            let anchor = path.anchor.as_deref().unwrap_or_default();
            return self.show_anchored(file, anchor, budget).map(Some);
        };
        self.show_at(file, target, budget).map(Some)
    }

    /// Where a `show` of `file`, given no target, goes for the `#anchor` after its path: in a note,
    /// the heading the anchor names, found as the reader's own links find one (`show_anchor`), else
    /// the tool's error naming it; in an HTML page, the anchor itself (`Target::Anchor`), which its
    /// window finds as Lectern's links find one, by a heading's id or the slug of its text. (A
    /// PDF's is its page: `show_at_path`.)
    fn show_anchored(&self, file: &Path, anchor: &str, budget: Budget) -> Result<ShowAt, String> {
        match opened_kind(file) {
            DocKind::Markdown | DocKind::Text => {
                let name = file_name(file);
                let (text, source) = self.note_text(file, &name, budget)?;
                show_anchor(&text, &source, &name, anchor)
            }
            DocKind::Html | DocKind::Pdf => {
                let anchor = anchor.to_owned();
                self.show_at(file, Target::Anchor { anchor }, budget)
            }
        }
    }

    /// The note or text file `file`, named `name`, read within `budget`: its text as shown, and
    /// its source.
    fn note_text(
        &self,
        file: &Path,
        name: &str,
        budget: Budget,
    ) -> Result<(TextMap, String), String> {
        let source = read_source(file, budget.at_most(self.timings.exists))?;
        let text = TextMap::of_note(name, &source).unwrap_or_default();
        Ok((text, source))
    }

    /// `highlight`: the agent's highlights in the file `args.path` (by default the file shown),
    /// painted by the window showing it, at where core found each target in a note or a text
    /// file, within `budget`. Every target must be there, or nothing is painted. A PDF can't be
    /// painted, which is the tool's error.
    fn agent_highlight(
        &self,
        args: HighlightArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<HighlightAnswer, String> {
        let (window, file) = self.shown_window(agent, args.path.as_deref())?;
        let kind = opened_kind(&file);
        if kind == DocKind::Pdf {
            return Err(NO_PDF_HIGHLIGHT.to_owned());
        }
        let name = file_name(&file);
        for target in &args.targets {
            check_target(kind, target, &name)?;
        }
        let places = match kind {
            DocKind::Markdown | DocKind::Text if !args.targets.is_empty() => {
                let (text, source) = self.note_text(&file, &name, budget)?;
                args.targets
                    .iter()
                    .map(|target| resolve_target(&text, &source, &name, target))
                    .collect::<Result<Vec<_>, _>>()?
            }
            _ => Vec::new(),
        };
        let request = AgentRequest::Highlight(HighlightArgs {
            path: Some(path_string(&file)),
            ..args
        });
        match self.ask_window(&window, request, places, &agent.name, budget)? {
            AgentReply::Highlighted { count } => Ok(HighlightAnswer {
                file: self.agent_mapper(agent).literal_forms(&path_string(&file)),
                window: window.label().to_owned(),
                highlights: count,
            }),
            AgentReply::Failed { message } => Err(message),
            _ => Err(UNEXPECTED.to_owned()),
        }
    }

    /// `read`: a page of the text of the file `args.path` (by default the file shown), as shown,
    /// within `budget` (`read_page`). A note's or a text file's is read from the file; an HTML
    /// page's is what the window's page last reported. A PDF has none.
    fn agent_read(
        &self,
        args: ReadArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<ReadAnswer, String> {
        let (window, file) = self.shown_window(agent, args.path.as_deref())?;
        let (from, max) = (args.from, args.max);
        // The text, whether its blocks have lines, and why it is less than the file's.
        let (text, lines, incomplete) = match opened_kind(&file) {
            DocKind::Pdf => return Err(NO_PDF_TEXT.to_owned()),
            DocKind::Html => {
                let request = AgentRequest::Read(ReadArgs {
                    path: Some(path_string(&file)),
                    ..args
                });
                match self.ask_window(&window, request, Vec::new(), &agent.name, budget)? {
                    AgentReply::PageText { blocks, .. }
                        if page_text_len(&blocks) > MAX_PAGE_TEXT =>
                    {
                        return Err("The page's text is over 2 MB, too much to read.".to_owned())
                    }
                    AgentReply::PageText { blocks, truncated } => (
                        TextMap::from_blocks(&blocks, ""),
                        false,
                        truncated.then_some(PAGE_TEXT_CUT),
                    ),
                    AgentReply::Failed { message } => return Err(message),
                    _ => return Err(UNEXPECTED.to_owned()),
                }
            }
            DocKind::Markdown | DocKind::Text => {
                let (text, _) = self.note_text(&file, &file_name(&file), budget)?;
                (text, true, None)
            }
        };
        let file = self.agent_mapper(agent).literal_forms(&path_string(&file));
        Ok(read_page(file, &text, lines, incomplete, from, max))
    }

    /// The window showing the file `raw` names (a Windows or WSL path, from `agent`), else the
    /// file shown, and that file: of the windows showing it, or any, the most recently focused. An
    /// error, a sentence for the agent, when no window shows it, or none shows a file.
    fn shown_window(
        &self,
        agent: &Agent,
        raw: Option<&str>,
    ) -> Result<(Arc<WindowState>, PathBuf), String> {
        let wanted = raw
            .map(|raw| {
                self.agent_location(agent, raw)
                    .map(|p| PathBuf::from(p.windows))
            })
            .transpose()?;
        self.windows_by_focus()
            .into_iter()
            .find_map(|window| {
                let shown = window.shown_file()?;
                let wanted = wanted.as_ref().is_none_or(|w| same_path(w, &shown));
                wanted.then_some((window, shown))
            })
            .ok_or_else(|| match &wanted {
                Some(file) => format!("{} isn't shown in Lectern. Show it first.", file_name(file)),
                None => "Lectern isn't showing a file. Show one first.".to_owned(),
            })
    }

    /// Every window, the most recently focused first.
    fn windows_by_focus(&self) -> Vec<Arc<WindowState>> {
        let focus = lock(&self.focus).clone();
        let mut windows = self.windows();
        windows.sort_by_key(|w| {
            focus
                .iter()
                .position(|label| label == w.label())
                .unwrap_or(usize::MAX)
        });
        windows
    }

    /// Sends `request`, with `places`, to the UI of `window` and waits for its answer, within
    /// `budget`: a sentence for the agent when it doesn't come in time, or the window was let go of
    /// (it closed, or turned to another workspace) before answering.
    fn ask_window(
        &self,
        window: &WindowState,
        request: AgentRequest,
        places: Vec<TextPlace>,
        agent: &str,
        budget: Budget,
    ) -> Result<AgentReply, String> {
        let label = window.label();
        let (id, reply) = self.agent_calls.open(label);
        let answer = if window.is_retired() {
            Err(RecvTimeoutError::Disconnected)
        } else {
            window.emit(UiEvent::AgentRequest(AgentCall {
                id,
                request,
                agent: agent.to_owned(),
                places,
                within_ms: None,
            }));
            reply.recv_timeout(budget.at_most(UI_WAIT))
        };
        self.agent_calls.forget(id);
        answer.map_err(|e| match e {
            RecvTimeoutError::Timeout => "The window didn't answer in time.".to_owned(),
            RecvTimeoutError::Disconnected => match self.window(label) {
                Some(_) => "That window turned to another workspace.".to_owned(),
                None => CLOSED.to_owned(),
            },
        })
    }

    /// `workspace`: switches to the workspace named `args.name`, or creates it with `args.folders`
    /// (`agent`'s paths), within `budget`.
    fn agent_workspace(
        self: &Arc<Self>,
        args: WorkspaceArgs,
        agent: &Agent,
        budget: Budget,
    ) -> Result<WorkspaceAnswer, String> {
        match args.action {
            WorkspaceAction::Switch => self.agent_switch(&args.name, budget),
            WorkspaceAction::Create => self.agent_create(&args.name, &args.folders, agent, budget),
        }
    }

    /// Brings the window of the workspace named `name` forward, or reopens it, closed, in a new
    /// window.
    fn agent_switch(
        self: &Arc<Self>,
        name: &str,
        budget: Budget,
    ) -> Result<WorkspaceAnswer, String> {
        let front = read(&self.settings).agent_focus == AgentFocus::Front;
        let (workspace, label, opened) = {
            let _life = lock(&self.lifecycle);
            let (id, workspace) = {
                let workspaces = read(&self.workspaces);
                let ws = workspaces
                    .find_by_name(name)
                    .ok_or_else(|| no_such_workspace(name, &workspaces))?;
                (ws.id.clone(), ws.name.clone())
            };
            match self.shown().remove(&id) {
                Some(label) => (workspace, label, false),
                None => (workspace, self.bind_window(Some(id), front), true),
            }
        };
        if opened {
            self.build(&label, front)?;
            self.workspaces_changed();
        }
        Ok(WorkspaceAnswer {
            workspace,
            focus: self.agent_focus(&label, budget),
            window: label,
            opened,
        })
    }

    /// A new workspace named `name`, which no workspace is called, holding `folders` (Windows or
    /// WSL paths of folders, from `agent`), opened in a new window. A folder on a share must be on
    /// a host the user configured (`Trust::configured_hosts`): one trusted only for this session,
    /// as the user opened a file there, would be trusted for good as a root.
    fn agent_create(
        self: &Arc<Self>,
        name: &str,
        folders: &[String],
        agent: &Agent,
        budget: Budget,
    ) -> Result<WorkspaceAnswer, String> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let configured = read(&self.trust).configured_hosts();
        for folder in folders {
            let located = agent_path(&self.agent_mapper(agent), folder, &configured)?;
            let root = normalize_root(&located.windows)?;
            if !is_folder(&root, self.timings.exists) {
                return Err(format!(
                    "{} isn't a folder Lectern can reach.",
                    root.display()
                ));
            }
            if !roots.iter().any(|r| same_path(r, &root)) {
                roots.push(root);
            }
        }
        let front = read(&self.settings).agent_focus == AgentFocus::Front;
        let (label, workspace) = {
            // From making the workspace to binding its window, so nothing opens it meanwhile: a
            // launch or a show of a file in its folders would reopen it in a window of its own.
            let _life = lock(&self.lifecycle);
            let (id, workspace) = self.change_workspaces(|workspaces| {
                if let Some(ws) = workspaces.find_by_name(name) {
                    return Err(format!(
                        "There's a workspace named “{}” already. Switch to it, or choose another \
                         name.",
                        ws.name
                    ));
                }
                let id = workspaces.create(name);
                let ws = workspaces
                    .get_mut(&id)
                    .ok_or("The workspace wasn't made.")?;
                ws.roots = roots.iter().map(|root| path_string(root)).collect();
                Ok((id, ws.name.clone()))
            })?;
            if !roots.is_empty() && self.reconfigure_trust() {
                self.forget_renders();
            }
            (self.bind_window(Some(id), front), workspace)
        };
        let built = self.build(&label, front);
        self.workspaces_changed();
        built?;
        Ok(WorkspaceAnswer {
            workspace,
            focus: self.agent_focus(&label, budget),
            window: label,
            opened: true,
        })
    }

    /// Brings the window `label` forward for an agent as the `agentFocus` setting has it, once it
    /// has shown (waiting a moment for a new one), within `budget`: how far it came, or none when
    /// it isn't showing or didn't say in time.
    fn agent_focus(&self, label: &str, budget: Budget) -> Option<FocusStep> {
        let window = self.window(label)?;
        let mode = read(&self.settings).agent_focus;
        if mode == AgentFocus::Front {
            // Restored at launch and not shown yet, it comes forward as it shows, not behind.
            window.quiet.store(false, Ordering::SeqCst);
        }
        if !window.ui_shown.wait(budget.at_most(self.timings.shown)) {
            log::info!("{label} hasn't shown yet; it comes forward as it does");
        }
        self.host.agent_focus(label, mode, budget.left())
    }

    /// The path `agent` wrote, `raw`, as `agent_location` takes it: a Linux path goes into the
    /// agent's own WSL distribution, when it runs in one (`agent_mapper`).
    fn agent_location(&self, agent: &Agent, raw: &str) -> Result<AgentPath, String> {
        agent_location(&self.agent_mapper(agent), &read(&self.trust), raw)
    }

    /// What maps `agent`'s paths, both ways: Lectern's mapper, in the agent's own WSL distribution
    /// when it runs in one (`Agent::mapper`). Every path an agent gives is read with it, and every
    /// path in an answer to it is given with it, so a file in another distribution has no WSL form.
    fn agent_mapper(&self, agent: &Agent) -> PathMapper {
        agent.mapper(&self.mapper())
    }

    /// `exe`, this Lectern's, in both forms, for the commands that connect an agent to it
    /// (Preferences › Agents): the WSL form, `PathMapper::to_wsl`'s, is none when WSL can't reach
    /// it, as on a share no mapping reaches.
    pub fn exe_forms(&self, exe: &Path) -> PathForms {
        PathForms {
            windows: Some(path_string(exe)),
            wsl: self.mapper().to_wsl(exe),
        }
    }

    /// The name of the workspace `window` shows; none for a blank window.
    fn workspace_name(&self, window: &WindowState) -> Option<String> {
        window
            .workspace_id()
            .and_then(|id| self.read_workspace(&id, |ws| ws.name.clone()))
    }
}

impl WindowState {
    /// Where each of `targets` is now in `path`, the note or text file the window shows, as core
    /// finds them in the file as it is on disk (`resolve_target`); none for one that isn't there.
    /// The window asks again for its agent's highlights once the file's text has changed.
    pub fn agent_places(
        &self,
        path: &str,
        targets: &[Target],
    ) -> Result<Vec<Option<TextPlace>>, String> {
        let file = PathBuf::from(path);
        if !self
            .shown_file()
            .is_some_and(|shown| same_path(&shown, &file))
        {
            return Err("That file isn't shown in this window.".to_owned());
        }
        if !matches!(opened_kind(&file), DocKind::Markdown | DocKind::Text) {
            return Err("Only a note's or a text file's targets are found again.".to_owned());
        }
        if targets.len() > TARGETS_MAX {
            return Err(format!("At most {TARGETS_MAX} targets are found at once."));
        }
        let source = note_source(&file)?;
        let name = file_name(&file);
        let text = TextMap::of_note(&name, &source).unwrap_or_default();
        Ok(targets
            .iter()
            .map(|target| resolve_target(&text, &source, &name, target).ok())
            .collect())
    }

    /// The file the window shows, which opened; none for a blank window, the welcome page or a
    /// file that didn't open.
    fn shown_file(&self) -> Option<PathBuf> {
        lock(&self.current)
            .as_ref()
            .filter(|c| c.doc.is_some())
            .map(|c| c.path.clone())
    }

    /// What Rust knows the window shows: the file it last opened.
    fn known_view(&self) -> WindowView {
        WindowView {
            path: lock(&self.current).as_ref().map(|c| path_string(&c.path)),
            ..WindowView::default()
        }
    }
}

/// The path an agent wrote, `raw` (`agent_path`): on a share only when Lectern trusts its host
/// already (`Trust::hosts`: the user's libraries, mappings and WSL, and the hosts of files the user
/// opened, which they have connected to). Checked on the text alone, before anything touches the
/// path; an agent's open never adds a host (`OpenQueue::offer_from_agent`).
fn agent_location(mapper: &PathMapper, trust: &Trust, raw: &str) -> Result<AgentPath, String> {
    agent_path(mapper, raw, &trust.hosts())
}

/// `path`'s file name, as an agent is told it; the whole path when it has none.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path_string(path), |n| n.to_string_lossy().into_owned())
}

/// The text of the note or text file at `path`, as its open reads it: an error, a sentence for the
/// agent, when it can't be read, or doesn't answer within `timeout` (a stalled share).
fn read_source(path: &Path, timeout: Duration) -> Result<String, String> {
    let (done, read) = mpsc::channel();
    let probed = path.to_path_buf();
    thread::Builder::new()
        .name("lectern-agent-read".to_owned())
        .spawn(move || {
            // Gone when the read already timed out.
            let _ = done.send(note_source(&probed));
        })
        .map_err(|e| e.to_string())?;
    match read.recv_timeout(timeout) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(message)) => Err(message),
        Err(RecvTimeoutError::Timeout) => Err(format!("{} didn't answer in time.", path.display())),
        Err(RecvTimeoutError::Disconnected) => {
            Err(format!("Lectern couldn't read {}.", path.display()))
        }
    }
}

/// The text of the note or text file at `path`, as its open reads it, and only what it would open:
/// a text file over the size an open shows is refused, as the open refuses it, before it is read.
fn note_source(path: &Path) -> Result<String, String> {
    let stamp = stat_doc(path).map_err(|e| e.message)?;
    check_size(path, opened_kind(path), stamp.size).map_err(|e| e.message)?;
    read_file(path)
        .map(|decoded| decoded.text)
        .map_err(|e| e.message)
}

fn to_value(answer: &impl Serialize) -> Result<Value, String> {
    serde_json::to_value(answer).map_err(|e| e.to_string())
}

/// Whether `path` is a file, as an agent's `show` needs: an error, a sentence for the agent, when
/// it is a folder, isn't there, or doesn't answer within `timeout` (a stalled share).
fn check_file(path: &Path, timeout: Duration) -> Result<(), String> {
    let (done, stat) = mpsc::channel();
    let probed = path.to_path_buf();
    thread::Builder::new()
        .name("lectern-probe".to_owned())
        .spawn(move || {
            // Gone when the check already timed out.
            let _ = done.send(fs::metadata(&probed).map(|meta| meta.is_dir()));
        })
        .map_err(|e| e.to_string())?;
    match stat.recv_timeout(timeout) {
        Ok(Ok(false)) => Ok(()),
        Ok(Ok(true)) => Err(format!(
            "{} is a folder, and show opens a file.",
            path.display()
        )),
        Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => {
            Err(format!("There's no file at {}.", path.display()))
        }
        Ok(Err(e)) => Err(format!("Lectern can't open {}: {e}", path.display())),
        Err(RecvTimeoutError::Timeout) => Err(format!("{} didn't answer in time.", path.display())),
        Err(RecvTimeoutError::Disconnected) => {
            Err(format!("Lectern couldn't check {}.", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Target;
    use crate::state::profile::Profile;
    use crate::state::sync::write;
    use crate::state::test_support::*;
    use lectern_core::agent::request::{HighlightArgs, ReadArgs, Target as AgentTarget};
    use lectern_core::agent::status::{AskOutcome, ViewSelection};
    use lectern_core::agent::target::{ShowAt, TextPlace};
    use lectern_core::ipc::{
        AgentOpens, OpenResult, OpenWhere, PathMapping, StartupPayload, WorkspaceOutcome,
    };
    use lectern_core::review::text::PageBlock;

    /// The `agent-request`s sent, in order, with the window each went to.
    pub(super) fn agent_requests(host: &FakeHost) -> Vec<(Target, AgentCall)> {
        lock(&host.events)
            .iter()
            .filter_map(|(target, e)| match e {
                UiEvent::AgentRequest(call) => Some((target.clone(), call.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn status_reports_each_window_as_its_ui_does() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n\n## Rollout\n\nText.\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let _ = f.state.open_document(&path_string(&plan));
        let host = Arc::clone(&f.host);
        let app = Arc::clone(&f.app);
        let plan_text = path_string(&plan);
        let answering = thread::spawn(move || {
            wait_until("the UI is asked", || !agent_requests(&host).is_empty());
            let (target, call) = agent_requests(&host).remove(0);
            assert_eq!(target, Target::Window("main".to_owned()));
            assert_eq!(call.request, AgentRequest::Status);
            let view = WindowView {
                path: Some(plan_text),
                heading: vec!["Plan".to_owned(), "Rollout".to_owned()],
                line: Some(3),
                selection: Some(ViewSelection {
                    quote: "Text.".to_owned(),
                    start_line: Some(5),
                    end_line: Some(5),
                }),
                ..WindowView::default()
            };
            assert!(app.agent_reply("main", call.id, AgentReply::View(view)));
        });
        let status = f.app.agent_status(&claude());
        answering.join().unwrap();
        assert_eq!(status.windows.len(), 1);
        let main = &status.windows[0];
        assert_eq!(main.label, "main");
        assert_eq!(main.workspace.as_deref(), Some("Main"));
        assert_eq!(
            main.file.as_ref().and_then(|f| f.windows.clone()),
            Some(path_string(&plan))
        );
        assert_eq!(main.heading.as_deref(), Some("Plan › Rollout"));
        assert_eq!(main.line, Some(3));
        assert_eq!(main.selection.as_ref().unwrap().quote, "Text.");
        assert_eq!(main.ask, None);
        assert!(status.closed_workspaces.is_empty());
        assert_eq!(status.settings.agent_opens, AgentOpens::Ask);
    }

    #[test]
    fn a_window_that_does_not_answer_is_reported_from_what_rust_knows() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let _ = f.state.open_document(&path_string(&plan));
        let status = f.app.agent_status(&claude());
        let main = &status.windows[0];
        assert_eq!(
            main.file.as_ref().and_then(|f| f.windows.clone()),
            Some(path_string(&plan))
        );
        assert_eq!((main.heading.clone(), main.line), (None, None));
        // Its answer, too late, finds no one waiting.
        let (_, call) = agent_requests(&f.host).remove(0);
        assert!(!f
            .app
            .agent_reply("main", call.id, AgentReply::View(WindowView::default())));
    }

    #[test]
    fn only_the_window_asked_answers() {
        let f = fixture(profile(&[]), FakeHost::default());
        let (id, answer) = f.app.agent_calls.open("main");
        let view = AgentReply::View(WindowView::default());
        assert!(!f.app.agent_reply("win-1", id, view.clone()));
        assert!(f.app.agent_reply("main", id, view.clone()));
        assert_eq!(answer.recv().unwrap(), view);
        assert!(!f.app.agent_reply("main", id, view));
    }

    // show

    /// Answers the next `agent-request` the window `label` is sent with `replies`, in order, from
    /// a thread of its own, which gives back the request it answered. Called before the request
    /// is sent.
    pub(super) fn ui_answers(
        f: &Fixture,
        label: &str,
        replies: Vec<AgentReply>,
    ) -> thread::JoinHandle<(Target, AgentCall)> {
        let (host, app, label) = (Arc::clone(&f.host), Arc::clone(&f.app), label.to_owned());
        let before = agent_requests(&host).len();
        thread::spawn(move || {
            wait_until("the UI is asked", || agent_requests(&host).len() > before);
            let (target, call) = agent_requests(&host).remove(before);
            for reply in replies {
                assert!(app.agent_reply(&label, call.id, reply));
            }
            (target, call)
        })
    }

    fn show(path: &str, wait: u32) -> AgentRequest {
        AgentRequest::Show(ShowArgs {
            path: path.to_owned(),
            workspace: None,
            target: None,
            new_window: false,
            wait,
        })
    }

    /// The vault (Main's library) holding plan.md and notes.md, with the first window's UI up and
    /// showing: it has asked for its startup payload, and shown.
    fn vault(profile_of: impl FnOnce(&Path) -> Profile) -> (Fixture, PathBuf, PathBuf) {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n\n## Rollout\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile_of(&root), FakeHost::default());
        let _ = f.state.startup();
        f.state.ui_shown.open();
        (f, plan, notes)
    }

    #[test]
    fn a_show_goes_to_the_windows_ui_which_asks_and_the_window_comes_forward() {
        let (f, _, notes) = vault(|root| profile(&[root]));
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let answer = f
            .app
            .agent_answer(show(&path_string(&notes), 0), &claude())
            .unwrap();
        let (target, call) = answering.join().unwrap();
        assert_eq!(target, Target::Window("main".to_owned()));
        assert_eq!(call.agent, "Claude");
        let AgentRequest::Show(args) = call.request else {
            panic!("not a show: {:?}", call.request);
        };
        assert_eq!(args.path, path_string(&notes));
        assert_eq!(answer["state"], "asked");
        assert_eq!(answer["file"]["windows"], path_string(&notes).as_str());
        assert_eq!(answer["window"], "main");
        assert_eq!(answer["workspace"], "Main");
        assert_eq!(answer["focus"], "foreground");
        assert_eq!(
            *lock(&f.host.agent_focused),
            [("main".to_owned(), AgentFocus::Front)]
        );
        // Nothing waits on it: what becomes of it is for `status`.
        assert!(!f.app.agent_reply(
            "main",
            call.id,
            AgentReply::Answered {
                answer: AskAnswer::Shown,
                message: None,
            }
        ));
    }

    /// A WSL path goes to the UI as its Windows form, and the answer gives both.
    #[test]
    fn a_wsl_path_is_shown_as_its_windows_form() {
        let (f, plan, _) = vault(|root| profile(&[root]));
        let wsl = f.app.mapper().to_wsl(&plan).expect("a WSL form");
        assert!(wsl.starts_with("/mnt/"), "{wsl}");
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f.app.agent_answer(show(&wsl, 0), &claude()).unwrap();
        let (_, call) = answering.join().unwrap();
        let AgentRequest::Show(args) = call.request else {
            panic!("not a show");
        };
        assert!(same_path(Path::new(&args.path), &plan), "{}", args.path);
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["file"]["wsl"], wsl.as_str());
        assert!(same_path(
            Path::new(answer["file"]["windows"].as_str().unwrap()),
            &plan
        ));
    }

    /// With a wait, an asked show answers what the reader did, or that they haven't yet.
    #[test]
    fn a_show_that_waits_answers_what_became_of_it() {
        let (f, _, notes) = vault(|root| profile(&[root]));
        let notes = path_string(&notes);
        for (answer, state) in [
            (AskAnswer::Shown, "shown"),
            (AskAnswer::Dismissed, "dismissed"),
            (AskAnswer::Replaced, "replaced"),
        ] {
            let answering = ui_answers(
                &f,
                "main",
                vec![
                    AgentReply::Asked,
                    AgentReply::Answered {
                        answer,
                        message: None,
                    },
                ],
            );
            let shown = f.app.agent_answer(show(&notes, 30), &claude()).unwrap();
            answering.join().unwrap();
            assert_eq!(shown["state"], state);
        }
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let started = Instant::now();
        let shown = f.app.agent_answer(show(&notes, 1), &claude()).unwrap();
        answering.join().unwrap();
        assert_eq!(shown["state"], "waiting");
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[test]
    fn a_show_the_window_could_not_open_is_its_error() {
        let (f, plan, _) = vault(|root| profile(&[root]));
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Failed {
                message: "It's too big to show.".to_owned(),
            }],
        );
        assert_eq!(
            f.app.agent_answer(show(&path_string(&plan), 5), &claude()),
            Err("It's too big to show.".to_owned())
        );
        answering.join().unwrap();
    }

    /// A file the reader chose to show that didn't open: the agent that waited hears why.
    #[test]
    fn a_show_the_reader_chose_that_did_not_open_is_its_error() {
        let (f, _, notes) = vault(|root| profile(&[root]));
        let why = "notes.md didn't open: It's too big to show.";
        let answering = ui_answers(
            &f,
            "main",
            vec![
                AgentReply::Asked,
                AgentReply::Answered {
                    answer: AskAnswer::Failed,
                    message: Some(why.to_owned()),
                },
            ],
        );
        assert_eq!(
            f.app
                .agent_answer(show(&path_string(&notes), 30), &claude()),
            Err(why.to_owned())
        );
        answering.join().unwrap();
    }

    /// The `show` `request` on a thread of its own, from Claude.
    fn showing(f: &Fixture, request: AgentRequest) -> thread::JoinHandle<Result<Value, String>> {
        let app = Arc::clone(&f.app);
        thread::spawn(move || app.agent_answer(request, &claude()))
    }

    /// Two `show`s that wait, to one window: the newer takes the older's place at the bar, which
    /// the older's agent hears at once, and the newer waits on for the reader.
    #[test]
    fn a_newer_show_to_the_same_window_replaces_one_that_waits() {
        let (f, plan, notes) = vault(|root| profile(&[root]));
        let first_asked = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let first = showing(&f, show(&path_string(&notes), 30));
        let (_, first_call) = first_asked.join().unwrap();
        // As the UI does with a newer request: the older one settles as replaced, and the newer
        // is asked about, here to hear Not now.
        let (host, app) = (Arc::clone(&f.host), Arc::clone(&f.app));
        let before = agent_requests(&host).len();
        let ui = thread::spawn(move || {
            wait_until("the UI is asked again", || {
                agent_requests(&host).len() > before
            });
            let (_, second_call) = agent_requests(&host).remove(before);
            let answered = |answer| AgentReply::Answered {
                answer,
                message: None,
            };
            assert!(app.agent_reply("main", first_call.id, answered(AskAnswer::Replaced)));
            assert!(app.agent_reply("main", second_call.id, AgentReply::Asked));
            assert!(app.agent_reply("main", second_call.id, answered(AskAnswer::Dismissed)));
        });
        let second = f
            .app
            .agent_answer(show(&path_string(&plan), 30), &claude())
            .unwrap();
        ui.join().unwrap();
        assert_eq!(first.join().unwrap().unwrap()["state"], "replaced");
        assert_eq!(second["state"], "dismissed");
    }

    /// A window that closes while a `show` waits for the reader ends the wait at once: the agent
    /// hears the window has closed.
    #[test]
    fn closing_the_window_while_a_show_waits_answers_that_it_closed() {
        let (f, _, notes) = vault(|root| profile(&[root]));
        // Another window, so closing this one lets it go.
        f.app.new_window().unwrap();
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let started = Instant::now();
        let shown = showing(&f, show(&path_string(&notes), 30));
        answering.join().unwrap();
        f.app.window_closing("main");
        assert_eq!(shown.join().unwrap(), Err(CLOSED.to_owned()));
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// A window that turns to another workspace while a `show` waits (its page reloads, and the
    /// bar with it) ends the wait at once: the request is dismissed.
    #[test]
    fn turning_the_window_to_another_workspace_while_a_show_waits_dismisses_it() {
        let (f, _, garden) = main_and_garden(AgentFocus::Front);
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let started = Instant::now();
        let shown = showing(&f, show(&path_string(&notes), 30));
        answering.join().unwrap();
        assert_eq!(
            f.app.open_workspace("main", &garden, OpenWhere::Here),
            Ok(WorkspaceOutcome::Reload)
        );
        assert_eq!(shown.join().unwrap().unwrap()["state"], "dismissed");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// A window that closes while a `highlight` waits for its UI ends the wait at once: the agent
    /// hears the window has closed.
    #[test]
    fn closing_the_window_during_a_highlight_answers_that_it_closed() {
        let (f, plan) = shown_plan();
        // Another window, so closing this one lets it go.
        f.app.new_window().unwrap();
        let before = agent_requests(&f.host).len();
        let started = Instant::now();
        let highlighting = {
            let app = Arc::clone(&f.app);
            let request = highlight(Some(&plan), vec![quote("Roll back")], false);
            thread::spawn(move || app.agent_answer(request, &claude()))
        };
        wait_until("the UI is asked", || agent_requests(&f.host).len() > before);
        f.app.window_closing("main");
        assert_eq!(highlighting.join().unwrap(), Err(CLOSED.to_owned()));
        assert!(started.elapsed() < UI_WAIT);
    }

    /// A `show` that reaches a window's state once it is let go of (the window closed, or turned
    /// to another workspace, as the show was routed) doesn't wait for an answer that won't come,
    /// and its UI hears nothing.
    #[test]
    fn a_show_that_reaches_a_window_let_go_of_answers_at_once() {
        let (f, _, garden) = main_and_garden(AgentFocus::Front);
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        let args = ShowArgs {
            path: path_string(&notes),
            workspace: None,
            target: None,
            new_window: false,
            wait: 30,
        };
        let started = Instant::now();
        let turned = f.app.window("main").unwrap();
        f.app
            .open_workspace("main", &garden, OpenWhere::Here)
            .unwrap();
        assert_eq!(
            f.app
                .show_in(&turned, args.clone(), Vec::new(), "Claude", Budget::new()),
            Ok((ShowState::Dismissed, None, None))
        );
        let closed = f.app.window("main").unwrap();
        f.app.window_destroyed("main");
        assert_eq!(
            f.app
                .show_in(&closed, args, Vec::new(), "Claude", Budget::new()),
            Err(CLOSED.to_owned())
        );
        assert!(started.elapsed() < UI_WAIT);
        assert!(agent_requests(&f.host).is_empty());
    }

    /// A `show` answers within the pipe's time, a second to spare: what is left of it bounds each
    /// wait, the window's UI's answer and the focus among them.
    #[test]
    fn a_show_waits_only_as_long_as_the_pipe_gives_it() {
        let (f, _, notes) = vault(|root| profile(&[root]));
        let args = || ShowArgs {
            path: path_string(&notes),
            workspace: None,
            target: None,
            new_window: false,
            wait: 0,
        };
        let within = |left: Duration| Budget(Instant::now() + left);
        // A UI that doesn't answer: given 300 ms, the show gives up then, not after UI_WAIT.
        let started = Instant::now();
        assert_eq!(
            f.app
                .agent_show(args(), &claude(), within(Duration::from_millis(300)))
                .unwrap_err(),
            "The window didn't answer in time."
        );
        assert!(started.elapsed() < UI_WAIT);
        // Answered at once, the focus is given what is left of a second, not the host's 2 s.
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        f.app
            .agent_show(args(), &claude(), within(Duration::from_secs(1)))
            .unwrap();
        answering.join().unwrap();
        let given = *lock(&f.host.focus_within).last().unwrap();
        assert!(given <= Duration::from_secs(1), "{given:?}");
        // The pipe's: ANSWER_WITHIN, less a second.
        let left = Budget::new().left();
        assert!(left <= ANSWER_WITHIN - ANSWER_SPARE, "{left:?}");
        assert!(left > ANSWER_WITHIN - ANSWER_SPARE * 2, "{left:?}");
    }

    /// Only a file is shown: a folder, a file that isn't there and a relative path are errors, and
    /// no window hears of them.
    #[test]
    fn a_show_of_anything_but_a_file_is_an_error() {
        let (f, plan, _) = vault(|root| profile(&[root]));
        let folder = plan.parent().unwrap().to_path_buf();
        let missing = folder.join("gone.md");
        let error = |path: &str| f.app.agent_answer(show(path, 0), &claude()).unwrap_err();
        assert!(error(&path_string(&folder)).contains("is a folder"));
        assert_eq!(
            error(&path_string(&missing)),
            format!("There's no file at {}.", missing.display())
        );
        assert!(error("notes/plan.md").contains("isn't a full path"));
        assert!(agent_requests(&f.host).is_empty());
        assert!(f.host.opened_windows().is_empty());
    }

    /// What `request`, a show to a window that opens the file as it starts, answers: once the file
    /// is held for that window, its UI asks for its startup payload (as a real window's does while
    /// the show waits), which opens it. With the payload, and the window.
    fn shown_as_it_starts(
        f: &Fixture,
        request: AgentRequest,
    ) -> (Result<Value, String>, StartupPayload, Arc<WindowState>) {
        let shown = showing(f, request);
        let held = || {
            f.app
                .windows()
                .into_iter()
                .find(|window| window.opens.has_pending())
        };
        wait_until("the file is held for the window's start", || {
            held().is_some()
        });
        let window = held().unwrap();
        let payload = window.startup();
        (shown.join().unwrap(), payload, window)
    }

    /// `status`'s answers for the window `label`, its UI answering at once with no answers of its
    /// own.
    fn status_answers(f: &Fixture, label: &str) -> Vec<(String, AskAnswer, Option<String>)> {
        let answering = ui_answers(f, label, vec![AgentReply::View(WindowView::default())]);
        let status = f.app.agent_status(&claude());
        answering.join().unwrap();
        let window = status.windows.iter().find(|w| w.label == label).unwrap();
        window
            .answers
            .iter()
            .map(|a| {
                (
                    a.path.windows.clone().unwrap_or_default(),
                    a.answer,
                    a.message.clone(),
                )
            })
            .collect()
    }

    /// A window whose UI isn't up yet opens the file first thing, as a launch's: no bar to ask.
    /// The show answers once it has opened.
    #[test]
    fn a_window_still_starting_opens_the_file_as_it_starts() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let (answer, payload, _) = shown_as_it_starts(&f, show(&path_string(&plan), 0));
        let answer = answer.unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["window"], "main");
        assert!(agent_requests(&f.host).is_empty());
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
    }

    /// A file a starting window can't open (here a binary `.md`, which passes the show's checks)
    /// is the show's error, not `shown`; `status` keeps it as failed.
    #[test]
    fn a_file_that_fails_to_open_as_its_window_starts_is_the_shows_error() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let binary = dir.file("vault/binary.md", "\u{0}\u{1}PNG");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let (answer, payload, _) = shown_as_it_starts(&f, show(&path_string(&binary), 0));
        let failed = format!("{} isn't a text file", binary.display());
        assert_eq!(answer, Err(failed.clone()));
        assert!(matches!(payload.initial, Some(OpenResult::Err { .. })));
        assert_eq!(
            status_answers(&f, "main"),
            [(path_string(&binary), AskAnswer::Failed, Some(failed))]
        );
    }

    /// A window that hasn't started by the time the show must answer: the show answers `waiting`,
    /// and `status` has what became of it once the window has opened it.
    #[test]
    fn a_show_whose_window_starts_too_late_answers_waiting_and_status_has_the_outcome() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let args = ShowArgs {
            path: path_string(&plan),
            workspace: None,
            target: None,
            new_window: false,
            wait: 0,
        };
        let started = Instant::now();
        let soon = Budget(Instant::now() + Duration::from_millis(300));
        let answer = f.app.agent_show(args, &claude(), soon).unwrap();
        assert_eq!(answer.state, ShowState::Waiting);
        assert!(started.elapsed() < UI_WAIT);
        assert!(status_answers(&f, "main").is_empty());
        let _ = f.state.startup();
        assert_eq!(
            status_answers(&f, "main"),
            [(path_string(&plan), AskAnswer::Shown, None)]
        );
    }

    /// A newer agent's show held for a starting window takes the place of an older one, which
    /// hears it was replaced, as at the ask bar; `status` has both.
    #[test]
    fn a_held_show_a_newer_one_takes_the_place_of_is_replaced() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let first = showing(&f, show(&path_string(&plan), 0));
        wait_until("the first is held", || f.state.opens.has_pending());
        let second = showing(&f, show(&path_string(&notes), 0));
        assert_eq!(first.join().unwrap().unwrap()["state"], "replaced");
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), notes);
        assert_eq!(second.join().unwrap().unwrap()["state"], "shown");
        assert_eq!(
            status_answers(&f, "main"),
            [
                (path_string(&plan), AskAnswer::Replaced, None),
                (path_string(&notes), AskAnswer::Shown, None),
            ]
        );
    }

    /// The path of the `show` `call`.
    fn shown_path(call: &AgentCall) -> &str {
        match &call.request {
            AgentRequest::Show(args) => &args.path,
            other => panic!("not a show: {other:?}"),
        }
    }

    /// A window still starting with a file the user launched (a second launch, held for it) opens
    /// that one: an agent's `show` meanwhile never takes its place. It goes to the window's UI
    /// once that is up, as to any window's, and the UI shows the launched file first.
    #[test]
    fn an_agents_show_never_takes_the_place_of_a_launch_a_starting_window_holds() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let launch = OpenRequest {
            path: path_string(&plan),
            t0_ms: None,
            folder: false,
        };
        assert!(f.state.opens.offer(launch).is_none());
        let shown = showing(&f, show(&path_string(&notes), 0));
        wait_until("the show waits for the window's UI", || {
            f.state.opens.agent_waits()
        });
        assert!(agent_requests(&f.host).is_empty());
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
        assert!(payload.agent_target.is_none());
        let (_, call) = answering.join().unwrap();
        assert_eq!(shown_path(&call), path_string(&notes));
        assert_eq!(shown.join().unwrap().unwrap()["state"], "asked");
    }

    /// The first window starting with the file the user launched Lectern with (the command
    /// line's, which boot is still rendering): an agent's `show` meanwhile never takes its place.
    /// The window opens the user's file, and the show then goes to its UI as to any window's, which
    /// shows the user's file first: then asks the reader, or, with `agentOpens: switch`, opens the
    /// agent's file after it.
    #[test]
    fn an_agents_show_waits_for_the_file_the_user_launched_lectern_with() {
        for (opens, reply, state) in [
            (AgentOpens::Ask, AgentReply::Asked, "asked"),
            (
                AgentOpens::Switch,
                AgentReply::Shown { target: None },
                "shown",
            ),
        ] {
            let dir = TempDir::new();
            let root = dir.folder("vault");
            let plan = dir.file("vault/plan.md", "# Plan\n");
            let notes = dir.file("vault/notes.md", "# Notes\n");
            let mut profile = profile(&[&root]);
            profile.settings.agent_opens = opens;
            let f = fixture_in(dir, profile, FakeHost::default());
            // As setup marks it when the command line names a path.
            f.opens.launched();
            let shown = showing(&f, show(&path_string(&notes), 0));
            wait_until("the show waits for the window's UI", || {
                f.opens.agent_waits()
            });
            assert!(agent_requests(&f.host).is_empty());
            f.early.fill(early_doc(&plan));
            let answering = ui_answers(&f, "main", vec![reply]);
            let payload = f.state.startup();
            assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
            assert!(payload.agent_target.is_none());
            let (_, call) = answering.join().unwrap();
            assert_eq!(shown_path(&call), path_string(&notes));
            assert_eq!(shown.join().unwrap().unwrap()["state"], state);
        }
    }

    /// Shows held for a window starting with the user's file: the newest wins, as at the ask bar.
    /// Each older one is replaced, which its agent hears at once and `status` keeps, so the window
    /// holds one at most, and only the newest goes to its UI once that is up.
    #[test]
    fn of_the_shows_waiting_for_a_launch_the_newest_alone_goes_to_the_ui() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let shows = ["a", "b", "c"].map(|name| dir.file(&format!("vault/{name}.md"), "# Show\n"));
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.opens.launched();
        f.early.fill(early_doc(&plan));
        let first = showing(&f, show(&path_string(&shows[0]), 0));
        wait_until("the first waits for the window's UI", || {
            f.opens.held_for_ui() == 1
        });
        let second = showing(&f, show(&path_string(&shows[1]), 0));
        assert_eq!(first.join().unwrap().unwrap()["state"], "replaced");
        assert_eq!(f.opens.held_for_ui(), 1);
        let third = showing(&f, show(&path_string(&shows[2]), 0));
        assert_eq!(second.join().unwrap().unwrap()["state"], "replaced");
        assert_eq!(f.opens.held_for_ui(), 1);
        assert!(agent_requests(&f.host).is_empty());
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
        let (_, call) = answering.join().unwrap();
        assert_eq!(shown_path(&call), path_string(&shows[2]));
        assert_eq!(third.join().unwrap().unwrap()["state"], "asked");
        assert_eq!(agent_requests(&f.host).len(), 1);
        assert_eq!(f.opens.held_for_ui(), 0);
        assert_eq!(
            status_answers(&f, "main"),
            [
                (path_string(&shows[0]), AskAnswer::Replaced, None),
                (path_string(&shows[1]), AskAnswer::Replaced, None),
            ]
        );
    }

    /// A held show that waits for the reader (`wait`), whose wait for its UI ends just as the UI
    /// answers `asked` (the answer comes between the wait's end and its clean-up): it answers with
    /// that, and goes on to wait for the reader, as any asked show does, answering what they chose.
    #[test]
    fn a_held_show_asked_as_its_wait_ends_still_waits_for_the_reader() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.opens.launched();
        f.early.fill(early_doc(&plan));
        // As the show's wait ends, the window starts and its UI asks the reader.
        let (state, app, host) = (
            Arc::clone(&f.state),
            Arc::clone(&f.app),
            Arc::clone(&f.host),
        );
        *lock(&f.app.agent_calls.before_later) = Some(Box::new(move || {
            let _ = state.startup();
            let (_, call) = agent_requests(&host).remove(0);
            assert!(app.agent_reply("main", call.id, AgentReply::Asked));
        }));
        let args = ShowArgs {
            path: path_string(&notes),
            workspace: None,
            target: None,
            new_window: false,
            wait: 30,
        };
        let soon = Budget(Instant::now() + Duration::from_millis(300));
        let app = Arc::clone(&f.app);
        let shown = thread::spawn(move || app.agent_show(args, &claude(), soon));
        // Past its clean-up, the show brings its window forward, then waits for the reader.
        wait_until("the window comes forward", || {
            !lock(&f.host.focus_within).is_empty()
        });
        let (_, call) = agent_requests(&f.host).remove(0);
        let chose = AgentReply::Answered {
            answer: AskAnswer::Shown,
            message: None,
        };
        let heard = f.app.agent_reply("main", call.id, chose);
        assert_eq!(shown.join().unwrap().unwrap().state, ShowState::Shown);
        assert!(heard);
    }

    /// The held shows that answered `waiting` are kept for their UI's answer, the latest
    /// `ANSWERS_KEPT`, each for the window it went to, until that window is let go of. An answer
    /// that came as the wait ended goes to the show itself.
    #[test]
    fn late_shows_are_kept_for_the_latest_few_and_go_with_their_window() {
        let calls = AgentCalls::default();
        let opens = Arc::new(OpenQueue::default());
        let path = |n: usize| format!(r"C:\{n}.md");
        let late = |label: &str, n: usize| {
            let (id, reply) = calls.open(label);
            let late = Late {
                id,
                label: label.to_owned(),
                path: path(n),
                opens: Arc::clone(&opens),
            };
            assert_eq!(calls.later(late, &reply), None);
            id
        };
        let ids: Vec<u64> = (0..=ANSWERS_KEPT).map(|n| late("main", n)).collect();
        // The oldest was let go of: its answer is no one's.
        assert!(!calls.answer("main", ids[0], AgentReply::Replaced));
        // Another window's answer isn't taken for it.
        assert!(!calls.answer("win-1", ids[1], AgentReply::Replaced));
        assert!(calls.answer("main", ids[1], AgentReply::Replaced));
        // Kept once, then done with.
        assert!(!calls.answer("main", ids[1], AgentReply::Replaced));
        // What becomes of an ask is its window's to keep.
        assert!(calls.answer("main", ids[2], AgentReply::Asked));
        let other = late("win-1", 99);
        calls.forget_window("win-1");
        assert!(!calls.answer("win-1", other, AgentReply::Replaced));
        let replaced = AskOutcome {
            path: path(1),
            answer: AskAnswer::Replaced,
            message: None,
        };
        assert_eq!(opens.answers(), [replaced]);
        // Answered as its wait ended: handed back, not kept.
        let (id, reply) = calls.open("main");
        assert!(calls.answer("main", id, AgentReply::Asked));
        let late = Late {
            id,
            label: "main".to_owned(),
            path: path(100),
            opens: Arc::clone(&opens),
        };
        assert_eq!(calls.later(late, &reply), Some(AgentReply::Asked));
        assert!(!calls.answer("main", id, AgentReply::Replaced));
        assert_eq!(opens.answers().len(), 1);
    }

    /// A boot that restores the workspace's last note (the command line names none): that isn't a
    /// file the user asked for, so an agent's `show` meanwhile opens in its place, as the window
    /// starts.
    #[test]
    fn an_agents_show_takes_the_place_of_a_restored_note() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.early.fill(restored_doc(&plan));
        let (answer, payload, _) = shown_as_it_starts(&f, show(&path_string(&notes), 0));
        assert_eq!(answer.unwrap()["state"], "shown");
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), notes);
        assert!(agent_requests(&f.host).is_empty());
    }

    /// A show waiting for a window that starts with the user's file, which hasn't started by the
    /// time the show must answer: it answers `waiting`, as a show held for any starting window
    /// does, and still goes to the window's UI once that is up, after the user's file.
    #[test]
    fn a_show_waiting_for_a_launch_that_starts_too_late_answers_waiting_and_still_goes_to_the_ui() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.opens.launched();
        f.early.fill(early_doc(&plan));
        let args = ShowArgs {
            path: path_string(&notes),
            workspace: None,
            target: None,
            new_window: false,
            wait: 0,
        };
        let started = Instant::now();
        let soon = Budget(Instant::now() + Duration::from_millis(300));
        let answer = f.app.agent_show(args, &claude(), soon).unwrap();
        assert_eq!(answer.state, ShowState::Waiting);
        assert!(started.elapsed() < UI_WAIT);
        assert!(agent_requests(&f.host).is_empty());
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
        let requests = agent_requests(&f.host);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, Target::Window("main".to_owned()));
        assert_eq!(shown_path(&requests[0].1), path_string(&notes));
        // Past its show's deadline already: the UI is given no time, not the time it had once.
        assert_eq!(requests[0].1.within_ms, Some(0));
    }

    /// A held show that answered `waiting`, whose file then fails to open once the window's UI
    /// has it: `status` has that outcome, with why, as it has an ask's.
    #[test]
    fn a_held_shows_outcome_after_it_answered_waiting_reaches_status() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let notes = dir.file("vault/notes.md", "# Notes\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.opens.launched();
        f.early.fill(early_doc(&plan));
        let args = ShowArgs {
            path: path_string(&notes),
            workspace: None,
            target: None,
            new_window: false,
            wait: 0,
        };
        let soon = Budget(Instant::now() + Duration::from_millis(300));
        let answer = f.app.agent_show(args, &claude(), soon).unwrap();
        assert_eq!(answer.state, ShowState::Waiting);
        let _ = f.state.startup();
        let (_, call) = agent_requests(&f.host).remove(0);
        let failed = format!("{} isn't a text file", notes.display());
        let reply = AgentReply::Failed {
            message: failed.clone(),
        };
        assert!(f.app.agent_reply("main", call.id, reply));
        assert_eq!(
            status_answers(&f, "main"),
            [(path_string(&notes), AskAnswer::Failed, Some(failed))]
        );
    }

    /// `newWindow`: a file no open window's libraries hold opens in a new blank window, as it
    /// starts; one a workspace holds goes to that workspace's window all the same.
    #[test]
    fn a_show_in_a_new_window_opens_a_blank_one_unless_a_workspace_holds_the_file() {
        let (f, plan, _) = vault(|root| profile(&[root]));
        let loose = f.dir.file("elsewhere/loose.md", "# Loose\n");
        let in_new = |path: &Path| {
            AgentRequest::Show(ShowArgs {
                path: path_string(path),
                workspace: None,
                target: None,
                new_window: true,
                wait: 0,
            })
        };
        let (answer, payload, window) = shown_as_it_starts(&f, in_new(&loose));
        let answer = answer.unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["workspace"], Value::Null);
        let label = answer["window"].as_str().unwrap().to_owned();
        assert_ne!(label, "main");
        assert_eq!(window.label(), label);
        assert_eq!(*lock(&f.host.opened), [(label.clone(), true)]);
        assert_eq!(window.workspace_id(), None);
        assert!(agent_requests(&f.host).is_empty());
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), loose);
        // Main's file goes to Main's window.
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f.app.agent_answer(in_new(&plan), &claude()).unwrap();
        answering.join().unwrap();
        assert_eq!(
            (answer["window"].as_str(), answer["workspace"].as_str()),
            (Some("main"), Some("Main"))
        );
        assert_eq!(f.host.opened_windows(), [label]);
    }

    /// Workspaces Main (open, over the vault) and Garden (closed, over the garden), each holding
    /// a note.
    fn main_and_garden(focus: AgentFocus) -> (Fixture, PathBuf, String) {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let garden = dir.folder("garden");
        let seeds = dir.file("garden/seeds.md", "# Seeds\n");
        let mut profile = profile(&[&root]);
        profile.settings.agent_focus = focus;
        let id = profile.workspaces.create("Garden");
        profile.workspaces.get_mut(&id).unwrap().roots = vec![path_string(&garden)];
        let f = fixture_in(dir, profile, FakeHost::default());
        let _ = f.state.startup();
        f.state.ui_shown.open();
        (f, seeds, id)
    }

    /// A file only a closed workspace holds reopens it in a new window, which opens the file as
    /// it starts, and comes forward as the setting says: with the focus, or without.
    #[test]
    fn a_show_of_a_file_in_a_closed_workspace_reopens_it_in_a_new_window() {
        for (focus, step, takes_focus) in [
            (AgentFocus::Front, "foreground", true),
            (AgentFocus::Raise, "raised", false),
            (AgentFocus::None, "flashed", false),
        ] {
            let (f, seeds, garden) = main_and_garden(focus);
            let (answer, payload, window) = shown_as_it_starts(&f, show(&path_string(&seeds), 0));
            let answer = answer.unwrap();
            assert_eq!(answer["state"], "shown");
            assert_eq!(answer["workspace"], "Garden");
            assert_eq!(answer["focus"], step);
            let label = answer["window"].as_str().unwrap().to_owned();
            assert_eq!(window.label(), label);
            assert_eq!(*lock(&f.host.opened), [(label.clone(), takes_focus)]);
            assert_eq!(*lock(&f.host.agent_focused), [(label.clone(), focus)]);
            assert_eq!(window.workspace_id(), Some(garden));
            assert!(agent_requests(&f.host).is_empty());
            assert_eq!(opened_path(payload.initial.as_ref().unwrap()), seeds);
        }
    }

    /// Named, a workspace takes the file whatever holds it; a name none has is an error naming
    /// those there are.
    #[test]
    fn a_show_in_a_named_workspace_goes_there() {
        let (f, seeds, _) = main_and_garden(AgentFocus::Front);
        let in_main = |name: &str| {
            AgentRequest::Show(ShowArgs {
                path: path_string(&seeds),
                workspace: Some(name.to_owned()),
                target: None,
                new_window: false,
                wait: 0,
            })
        };
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f.app.agent_answer(in_main("main"), &claude()).unwrap();
        answering.join().unwrap();
        assert_eq!(
            (answer["window"].as_str(), answer["workspace"].as_str()),
            (Some("main"), Some("Main"))
        );
        assert_eq!(
            f.app.agent_answer(in_main("Attic"), &claude()),
            Err(
                "There's no workspace named “Attic”. The workspaces are Main and Garden."
                    .to_owned()
            )
        );
        assert!(f.host.opened_windows().is_empty());
    }

    const UNTRUSTED: &str = r"Lectern doesn't open files on \\nas2 for an agent; add that share as a library folder first.";

    /// An agent's path on a share is taken only on a host Lectern already trusts: a library's,
    /// a mapping's or WSL's, a WSL path included, or one the user opened a file from, which they
    /// have connected to already.
    #[test]
    fn an_agents_path_on_a_share_needs_a_host_lectern_trusts() {
        let mut trust = Trust::new(&[r"\\nas\share\notes".to_owned()], &[]);
        let mapper = PathMapper {
            mappings: Vec::new(),
            wsl_distro: Some("Ubuntu".to_owned()),
        };
        let reach = |trust: &Trust, raw: &str| agent_location(&mapper, trust, raw);
        assert_eq!(reach(&trust, r"\\nas2\share\x.md").unwrap_err(), UNTRUSTED);
        assert_eq!(
            reach(&trust, r"\\nas\share\notes\x.md").unwrap().windows,
            r"\\nas\share\notes\x.md"
        );
        assert!(reach(&trust, r"\\wsl.localhost\Ubuntu\home\me\x.md").is_ok());
        assert_eq!(
            reach(&trust, "/home/me/x.md").unwrap().windows,
            r"\\wsl.localhost\Ubuntu\home\me\x.md"
        );
        assert!(reach(&trust, r"\\nas3\x\b.md").is_err());
        trust.opened_by_user(Path::new(r"\\nas3\x\a.md"));
        assert_eq!(
            reach(&trust, r"\\nas3\x\b.md").unwrap().windows,
            r"\\nas3\x\b.md"
        );
    }

    /// An agent's Linux paths go into the WSL distribution it runs in, when its `mcp` named one,
    /// whichever Lectern's settings name; an agent in none of its own goes by the settings.
    #[test]
    fn an_agents_linux_paths_go_into_the_distro_it_runs_in() {
        let mut profile = profile(&[]);
        profile.settings.wsl_distro = Some("Ubuntu".to_owned());
        let f = fixture(profile, FakeHost::default());
        let debian = Agent {
            distro: Some("Debian".to_owned()),
            ..claude()
        };
        let located = |agent: &Agent| {
            f.app
                .agent_location(agent, "/home/me/notes/plan.md:12")
                .unwrap()
        };
        let path = located(&debian);
        assert_eq!(
            path.windows,
            r"\\wsl.localhost\Debian\home\me\notes\plan.md"
        );
        assert_eq!(path.wsl.as_deref(), Some("/home/me/notes/plan.md"));
        assert_eq!(path.line, Some(12));
        assert_eq!(
            located(&claude()).windows,
            r"\\wsl.localhost\Ubuntu\home\me\notes\plan.md"
        );
        // A drive's path is the same from any distribution.
        assert_eq!(
            f.app
                .agent_location(&debian, "/mnt/s/Notes/My Vault/plan.md")
                .unwrap()
                .windows,
            r"S:\Notes\My Vault\plan.md"
        );
        // What `show` answers with: another distribution's file has no WSL form in the agent's.
        for (shown, wsl) in [(UBUNTUS, None), (DEBIANS, Some("/home/me/notes/plan.md"))] {
            let path = f.app.agent_location(&debian, shown).unwrap();
            assert_eq!(path.windows, shown);
            assert_eq!(path.wsl.as_deref(), wsl, "{shown}");
        }
    }

    /// A note in Lectern's own WSL distribution, and one in another.
    const UBUNTUS: &str = r"\\wsl.localhost\Ubuntu\home\me\notes\plan.md";
    const DEBIANS: &str = r"\\wsl.localhost\Debian\home\me\notes\plan.md";

    /// Every path in an answer is in the terms of the agent's own distribution: an agent in Debian
    /// gets `/home/me/…` for Debian's file and no WSL form for Ubuntu's, Lectern's own, in
    /// `status` as in `highlight`. An agent in none of its own reads them as Lectern does.
    #[test]
    fn an_agent_gets_wsl_forms_only_for_its_own_distros_files() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let mut profile = profile(&[&root]);
        profile.settings.wsl_distro = Some("Ubuntu".to_owned());
        let f = fixture_in(dir, profile, FakeHost::default());
        let _ = f.state.open_document(&path_string(&plan));
        let debian = Agent {
            distro: Some("Debian".to_owned()),
            ..claude()
        };
        let form = |wsl: Option<&str>, shown: &str| json!({"windows": shown, "wsl": wsl});
        // `status`: the window says what it shows.
        let status_file = |agent: &Agent, shown: &str| {
            let view = WindowView {
                path: Some(shown.to_owned()),
                ..WindowView::default()
            };
            let answering = ui_answers(&f, "main", vec![AgentReply::View(view)]);
            let status = f.app.agent_answer(AgentRequest::Status, agent).unwrap();
            answering.join().unwrap();
            status["windows"][0]["file"].clone()
        };
        let home = Some("/home/me/notes/plan.md");
        assert_eq!(status_file(&debian, UBUNTUS), form(None, UBUNTUS));
        assert_eq!(status_file(&debian, DEBIANS), form(home, DEBIANS));
        assert_eq!(status_file(&claude(), UBUNTUS), form(home, UBUNTUS));
        assert_eq!(status_file(&claude(), DEBIANS), form(None, DEBIANS));
        // `highlight`, of the file the window shows: here one in WSL, which the test can't open
        // without a distribution, so the window is made to show it.
        let highlighted_file = |agent: &Agent, shown: &str| {
            if let Some(current) = lock(&f.state.current).as_mut() {
                current.path = PathBuf::from(shown);
            }
            let answering = ui_answers(&f, "main", vec![AgentReply::Highlighted { count: 0 }]);
            let request = AgentRequest::Highlight(HighlightArgs {
                path: None,
                targets: Vec::new(),
                clear: true,
            });
            let answer = f.app.agent_answer(request, agent).unwrap();
            answering.join().unwrap();
            answer["file"].clone()
        };
        assert_eq!(highlighted_file(&debian, UBUNTUS), form(None, UBUNTUS));
        assert_eq!(highlighted_file(&debian, DEBIANS), form(home, DEBIANS));
        assert_eq!(highlighted_file(&claude(), UBUNTUS), form(home, UBUNTUS));
    }

    /// The exe the connect commands name, in both forms: WSL reaches it on a drive (through
    /// `/mnt`) or through a path mapping, and not on a share no mapping reaches.
    #[test]
    fn the_exe_comes_in_both_forms_when_wsl_reaches_it() {
        let mut profile = profile(&[]);
        profile.settings.path_mappings = vec![PathMapping {
            from: "/home/me/apps".to_owned(),
            to: r"\\nas\share\apps".to_owned(),
        }];
        let f = fixture(profile, FakeHost::default());
        let forms = |exe: &str| f.app.exe_forms(Path::new(exe));
        let installed = r"C:\Users\me\AppData\Local\Lectern\lectern.exe";
        assert_eq!(
            forms(installed),
            PathForms {
                windows: Some(installed.to_owned()),
                wsl: Some("/mnt/c/Users/me/AppData/Local/Lectern/lectern.exe".to_owned()),
            }
        );
        assert_eq!(
            forms(r"\\nas\share\apps\lectern.exe").wsl.as_deref(),
            Some("/home/me/apps/lectern.exe")
        );
        let unreached = r"\\nas2\tools\lectern.exe";
        assert_eq!(
            forms(unreached),
            PathForms {
                windows: Some(unreached.to_owned()),
                wsl: None,
            }
        );
    }

    /// A window starting with an agent's file opens it as it is: unlike the user's own open, it
    /// trusts nothing. So once the share is no longer the user's (here its mapping goes), an agent
    /// can't reach it; after the user's own open (a second launch) of the same file, it can. The
    /// share is `0.0.0.0`, which no packet goes to.
    #[test]
    fn an_agents_open_adds_no_host_as_the_users_does() {
        let share = PathMapping {
            from: "/home/me/shared".to_owned(),
            to: r"\\0.0.0.0\share".to_owned(),
        };
        let mut profile = profile(&[]);
        profile.settings.path_mappings = vec![share.clone()];
        let f = fixture(profile, FakeHost::default());
        let request = |name: &str| OpenRequest {
            path: format!(r"\\0.0.0.0\share\{name}"),
            t0_ms: None,
            folder: false,
        };
        let reach = |raw: &str| agent_location(&f.app.mapper(), &read(&f.app.trust), raw);
        assert!(matches!(
            f.state.opens.offer_from_agent(request("a.md"), None),
            AgentOffer::Held(_)
        ));
        let _ = f.state.startup();
        write(&f.app.trust).configure(&[], &[]);
        assert!(!f.app.trusts(r"\\0.0.0.0\share\a.md"));
        assert_eq!(
            reach(r"\\0.0.0.0\share\b.md").unwrap_err(),
            r"Lectern doesn't open files on \\0.0.0.0 for an agent; add that share as a library folder first."
        );
        write(&f.app.trust).configure(&[], &[share]);
        f.state.forward_now(request("a.md"));
        write(&f.app.trust).configure(&[], &[]);
        assert!(f.app.trusts(r"\\0.0.0.0\share\a.md"));
        assert!(reach(r"\\0.0.0.0\share\b.md").is_ok());
    }

    /// An agent's `show` of a share Lectern doesn't trust, or a workspace over one, is refused on
    /// the path's text alone: the host stays untrusted, nothing looks for the file (which would
    /// reach the host), no window hears of it and no workspace is made.
    #[test]
    fn an_agent_never_makes_a_share_trusted() {
        let f = fixture(profile(&[]), FakeHost::default());
        let shared = r"\\nas2\share\x.md";
        assert_eq!(
            f.app.agent_answer(show(shared, 0), &claude()),
            Err(UNTRUSTED.to_owned())
        );
        assert!(!f.app.trusts(shared));
        let over_share = workspace(
            WorkspaceAction::Create,
            "Shared",
            &[r"\\nas2\share".to_owned()],
        );
        assert_eq!(
            f.app.agent_answer(over_share, &claude()),
            Err(UNTRUSTED.to_owned())
        );
        assert!(read(&f.app.workspaces).find_by_name("Shared").is_none());
        assert!(agent_requests(&f.host).is_empty());
        assert!(f.host.opened_windows().is_empty());
    }

    /// `workspace create` takes a share only on a host the user configured (a library's, a
    /// mapping's or WSL's), never one trusted only because the user opened a file there this
    /// session: the workspace would make that trust last. A `show` there still goes. The share is
    /// `0.0.0.0`, which no packet goes to.
    #[test]
    fn creating_a_workspace_takes_only_hosts_the_user_configured() {
        let f = fixture(profile(&[]), FakeHost::default());
        write(&f.app.trust).opened_by_user(Path::new(r"\\0.0.0.0\share\plan.md"));
        assert!(f
            .app
            .agent_location(&claude(), r"\\0.0.0.0\share\notes.md")
            .is_ok());
        let over_share = workspace(
            WorkspaceAction::Create,
            "Shared",
            &[r"\\0.0.0.0\share".to_owned()],
        );
        assert_eq!(
            f.app.agent_answer(over_share, &claude()),
            Err(r"Lectern doesn't open files on \\0.0.0.0 for an agent; add that share as a library folder first.".to_owned())
        );
        assert!(read(&f.app.workspaces).find_by_name("Shared").is_none());
        assert!(f.host.opened_windows().is_empty());
    }

    // workspace

    fn workspace(action: WorkspaceAction, name: &str, folders: &[String]) -> AgentRequest {
        AgentRequest::Workspace(WorkspaceArgs {
            action,
            name: name.to_owned(),
            folders: folders.to_vec(),
        })
    }

    #[test]
    fn switching_to_an_open_workspace_brings_its_window_forward() {
        let (f, _, _) = main_and_garden(AgentFocus::Raise);
        let answer = f
            .app
            .agent_answer(workspace(WorkspaceAction::Switch, " MAIN ", &[]), &claude())
            .unwrap();
        assert_eq!(
            answer,
            json!({"workspace": "Main", "window": "main", "opened": false, "focus": "raised"})
        );
        assert!(f.host.opened_windows().is_empty());
        assert_eq!(
            *lock(&f.host.agent_focused),
            [("main".to_owned(), AgentFocus::Raise)]
        );
    }

    #[test]
    fn switching_to_a_closed_workspace_reopens_it_in_a_new_window() {
        let (f, _, garden) = main_and_garden(AgentFocus::Front);
        let answer = f
            .app
            .agent_answer(workspace(WorkspaceAction::Switch, "garden", &[]), &claude())
            .unwrap();
        let label = answer["window"].as_str().unwrap().to_owned();
        assert_ne!(label, "main");
        assert_eq!(answer["workspace"], "Garden");
        assert_eq!(answer["opened"], true);
        assert_eq!(*lock(&f.host.opened), [(label.clone(), true)]);
        assert_eq!(
            f.app.window(&label).unwrap().workspace_id(),
            Some(garden.clone())
        );
        // The main window still shows Main: no window changes workspace.
        assert_eq!(f.state.workspace_id().as_deref(), Some("w1"));
        // Switching again finds it open.
        let again = f
            .app
            .agent_answer(workspace(WorkspaceAction::Switch, "Garden", &[]), &claude())
            .unwrap();
        assert_eq!(
            (again["window"].as_str(), again["opened"].as_bool()),
            (Some(label.as_str()), Some(false))
        );
        assert_eq!(
            f.app
                .agent_answer(workspace(WorkspaceAction::Switch, "Attic", &[]), &claude()),
            Err(
                "There's no workspace named “Attic”. The workspaces are Main and Garden."
                    .to_owned()
            )
        );
    }

    /// A workspace created with a WSL folder holds its Windows form, in a new window.
    #[test]
    fn creating_a_workspace_with_a_wsl_folder_opens_a_new_window_over_its_windows_form() {
        let (f, _, _) = main_and_garden(AgentFocus::Front);
        let reading = f.dir.folder("reading");
        let wsl = f.app.mapper().to_wsl(&reading).expect("a WSL form");
        let answer = f
            .app
            .agent_answer(
                workspace(
                    WorkspaceAction::Create,
                    "Reading",
                    &[wsl.clone(), format!("{wsl}/")],
                ),
                &claude(),
            )
            .unwrap();
        assert_eq!(answer["workspace"], "Reading");
        assert_eq!(answer["opened"], true);
        let label = answer["window"].as_str().unwrap().to_owned();
        assert_eq!(*lock(&f.host.opened), [(label.clone(), true)]);
        let window = f.app.window(&label).unwrap();
        let roots = window.roots();
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert!(same_path(Path::new(&roots[0]), &reading), "{roots:?}");
        let saved = saved_workspaces(&f);
        let ws = saved.find_by_name("Reading").unwrap();
        assert!(ws.open);
        assert_eq!(ws.roots, roots);
    }

    #[test]
    fn creating_a_workspace_needs_a_new_name_and_real_folders() {
        let (f, _, _) = main_and_garden(AgentFocus::Front);
        assert_eq!(
            f.app
                .agent_answer(workspace(WorkspaceAction::Create, "garden", &[]), &claude()),
            Err(
                "There's a workspace named “Garden” already. Switch to it, or choose another name."
                    .to_owned()
            )
        );
        let missing = f.dir.0.join("missing");
        let error = f
            .app
            .agent_answer(
                workspace(WorkspaceAction::Create, "Attic", &[path_string(&missing)]),
                &claude(),
            )
            .unwrap_err();
        assert_eq!(
            error,
            format!("{} isn't a folder Lectern can reach.", missing.display())
        );
        assert!(f.host.opened_windows().is_empty());
        assert!(read(&f.app.workspaces).find_by_name("Attic").is_none());
    }

    /// A workspace an agent creates opens in its own window before anything else can open it. A
    /// `show` of a file in its folder that races the create, routed and bound under the lifecycle
    /// lock as any is, finds it not made yet, so the workspace never shows in two windows.
    #[test]
    fn a_workspace_being_created_is_never_opened_twice() {
        let (f, _, _) = main_and_garden(AgentFocus::Front);
        let reading = f.dir.folder("reading");
        let note = f.dir.file("reading/note.md", "# Note\n");
        let made = || read(&f.app.workspaces).find_by_name("Reading").is_some();
        let creating = {
            // Something else is binding a window: the create waits its turn.
            let _life = lock(&f.app.lifecycle);
            let app = Arc::clone(&f.app);
            let folder = path_string(&reading);
            let creating = thread::spawn(move || {
                app.agent_answer(
                    workspace(WorkspaceAction::Create, "Reading", &[folder]),
                    &claude(),
                )
            });
            let started = Instant::now();
            while !made() && started.elapsed() < Duration::from_millis(500) {
                thread::sleep(Duration::from_millis(10));
            }
            // The racing show, as `agent_show` routes and binds it.
            if let Ok(AgentRoute::Reopen(id)) = f.app.route_agent(&note, None, false) {
                f.app.bind_window(Some(id), false);
            }
            creating
        };
        creating.join().unwrap().unwrap();
        let id = read(&f.app.workspaces)
            .find_by_name("Reading")
            .unwrap()
            .id
            .clone();
        let showing: Vec<String> = f
            .app
            .windows()
            .iter()
            .filter(|w| w.workspace_id().as_deref() == Some(id.as_str()))
            .map(|w| w.label().to_owned())
            .collect();
        assert_eq!(showing.len(), 1, "{showing:?}");
    }

    // Targets, highlights and read.

    /// A note with a heading over a passage: its text is "Plan Rollout Roll back the migration
    /// first.", Rollout on line 3, the passage on line 5.
    const ROLLOUT: &str = "# Plan\n\n## Rollout\n\nRoll back the migration first.\n";

    /// The vault, with plan.md (`ROLLOUT`) on screen in Main's window, whose UI is up.
    pub(super) fn shown_plan() -> (Fixture, PathBuf) {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", ROLLOUT);
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let _ = f.state.startup();
        f.state.ui_shown.open();
        let _ = f.state.open_document(&path_string(&plan));
        (f, plan)
    }

    pub(super) fn quote(text: &str) -> AgentTarget {
        AgentTarget::Quote {
            quote: text.to_owned(),
            prefix: None,
            suffix: None,
        }
    }

    fn heading(text: &str) -> AgentTarget {
        AgentTarget::Heading {
            heading: text.to_owned(),
        }
    }

    /// A place core found: its lines, its text, and where it starts in its block's own text.
    fn place(lines: (u32, u32), text: (u32, u32), in_block: u32) -> TextPlace {
        TextPlace {
            start_line: lines.0,
            end_line: lines.1,
            text_start: Some(text.0),
            text_end: Some(text.1),
            block_offset: Some(in_block),
        }
    }

    pub(super) fn show_at(path: &Path, target: AgentTarget) -> AgentRequest {
        AgentRequest::Show(ShowArgs {
            path: path_string(path),
            workspace: None,
            target: Some(target),
            new_window: false,
            wait: 0,
        })
    }

    fn highlight(path: Option<&Path>, targets: Vec<AgentTarget>, clear: bool) -> AgentRequest {
        AgentRequest::Highlight(HighlightArgs {
            path: path.map(path_string),
            targets,
            clear,
        })
    }

    fn read_from(path: Option<&Path>, from: Option<u32>, max: Option<u32>) -> AgentRequest {
        AgentRequest::Read(ReadArgs {
            path: path.map(path_string),
            from,
            max,
        })
    }

    /// The show `call` asked for, with the places sent with it.
    fn shown_target(call: AgentCall) -> (Option<AgentTarget>, Vec<TextPlace>) {
        match call.request {
            AgentRequest::Show(args) => (args.target, call.places),
            other => panic!("not a show: {other:?}"),
        }
    }

    /// In a note, core finds the target: the window's UI is sent it with where it is.
    #[test]
    fn a_show_sends_the_window_where_core_found_its_target_in_a_note() {
        let (f, plan) = shown_plan();
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f
            .app
            .agent_answer(show_at(&plan, quote("Roll back")), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(
            shown_target(call),
            (Some(quote("Roll back")), vec![place((5, 5), (13, 22), 0)])
        );
        // The path's own line (`plan.md:3`) is a line target, found as any is.
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let at_line = format!("{}:3", path_string(&plan));
        f.app.agent_answer(show(&at_line, 0), &claude()).unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(
            shown_target(call),
            (
                Some(AgentTarget::Line { line: 3 }),
                vec![place((3, 3), (5, 12), 0)]
            )
        );
    }

    /// A line inside a paragraph over more lines is that line: the window is sent where its own
    /// text starts, to go to.
    #[test]
    fn a_show_of_a_line_inside_a_paragraph_sends_where_that_line_starts() {
        let (f, _) = shown_plan();
        let steps = f.dir.file(
            "vault/steps.md",
            "# Steps\n\nFreeze the writes.\nRoll back the migration.\nThaw the writes.\n",
        );
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        f.app
            .agent_answer(show_at(&steps, AgentTarget::Line { line: 4 }), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        // "Steps Freeze the writes. " is 25 units, then the line's 24; its paragraph starts at 6.
        assert_eq!(
            shown_target(call),
            (
                Some(AgentTarget::Line { line: 4 }),
                vec![place((4, 4), (25, 49), 19)]
            )
        );
    }

    /// A `#anchor` after the path, with no target, is the heading it names, as the reader's own
    /// links find it (by its id, else its slug); one no heading has is the tool's error, naming it.
    /// A target given wins.
    #[test]
    fn a_shows_anchor_goes_to_the_heading_it_names() {
        let (f, plan) = shown_plan();
        let anchored = |anchor: &str| format!("{}#{anchor}", path_string(&plan));
        for anchor in ["rollout", "Rollout"] {
            let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
            f.app
                .agent_answer(show(&anchored(anchor), 0), &claude())
                .unwrap();
            let (_, call) = answering.join().unwrap();
            assert_eq!(
                shown_target(call),
                (
                    Some(heading("Plan › Rollout")),
                    vec![place((3, 3), (5, 12), 0)]
                ),
                "{anchor}"
            );
        }
        let sent = agent_requests(&f.host).len();
        assert_eq!(
            f.app.agent_answer(show(&anchored("risks"), 0), &claude()),
            Err("There's no heading #risks in plan.md.".to_owned())
        );
        assert_eq!(agent_requests(&f.host).len(), sent);
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let with_target = AgentRequest::Show(ShowArgs {
            path: anchored("rollout"),
            workspace: None,
            target: Some(AgentTarget::Line { line: 1 }),
            new_window: false,
            wait: 0,
        });
        f.app.agent_answer(with_target, &claude()).unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(shown_target(call).0, Some(AgentTarget::Line { line: 1 }));
    }

    /// An HTML page's `#anchor`, with no target, goes to its window as an anchor, not as a
    /// heading's text: the window finds it as Lectern's links find one, by a heading's id or the
    /// slug of its text.
    #[test]
    fn a_pages_anchor_goes_to_its_window_as_an_anchor() {
        let (f, _) = shown_plan();
        let report = f.dir.file(
            "vault/report.html",
            "<h1>Report</h1>\n<h2 id=\"plan\">Rollout plan</h2>\n",
        );
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let anchored = format!("{}#rollout-plan", path_string(&report));
        f.app.agent_answer(show(&anchored, 0), &claude()).unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(
            shown_target(call),
            (
                Some(AgentTarget::Anchor {
                    anchor: "rollout-plan".to_owned()
                }),
                vec![]
            )
        );
    }

    /// A PDF's `#page=N` (in any letter case), with no target, goes to that page, as a page
    /// target does: checked as one, and past the end as one is (Lectern doesn't count a PDF's
    /// pages). Anything else after a PDF's path is the tool's error, saying what it takes. A target
    /// given wins.
    #[test]
    fn a_pdfs_page_fragment_goes_to_that_page() {
        let (f, _) = shown_plan();
        let brief = f.dir.file("vault/brief.pdf", "%PDF-1.4\n");
        let after = |suffix: &str| format!("{}{suffix}", path_string(&brief));
        let sent = |request: AgentRequest| {
            let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
            f.app.agent_answer(request, &claude()).unwrap();
            shown_target(answering.join().unwrap().1).0
        };
        for (suffix, page) in [("#page=3", 3), ("#PAGE=2", 2), ("#page=999", 999)] {
            assert_eq!(
                sent(show(&after(suffix), 0)),
                Some(AgentTarget::Page { page }),
                "{suffix}"
            );
        }
        // Past the end, as a page target is.
        assert_eq!(
            sent(show_at(&brief, AgentTarget::Page { page: 999 })),
            Some(AgentTarget::Page { page: 999 })
        );
        let requests = agent_requests(&f.host).len();
        let page_zero = show_at(&brief, AgentTarget::Page { page: 0 })
            .check()
            .unwrap_err();
        for suffix in ["#page=0", "#page=x"] {
            assert_eq!(
                f.app.agent_answer(show(&after(suffix), 0), &claude()),
                Err(page_zero.clone()),
                "{suffix}"
            );
        }
        for suffix in ["#intro", "#L7", ":7"] {
            assert_eq!(
                f.app.agent_answer(show(&after(suffix), 0), &claude()),
                Err(
                    "brief.pdf is a PDF: give #page=N after its path, or a page target.".to_owned()
                ),
                "{suffix}"
            );
        }
        assert_eq!(agent_requests(&f.host).len(), requests);
        let with_target = AgentRequest::Show(ShowArgs {
            path: after("#page=3"),
            workspace: None,
            target: Some(AgentTarget::Page { page: 5 }),
            new_window: false,
            wait: 0,
        });
        assert_eq!(sent(with_target), Some(AgentTarget::Page { page: 5 }));
    }

    /// A target that isn't there, or that the file doesn't take, is the tool's error: nothing
    /// opens, nothing asks and no window comes forward.
    #[test]
    fn a_show_whose_target_is_not_there_opens_nothing() {
        let (f, plan) = shown_plan();
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        let brief = f.dir.file("vault/brief.pdf", "%PDF-1.4\n");
        let error = |path: &Path, target| f.app.agent_answer(show_at(path, target), &claude());
        assert_eq!(
            error(&notes, quote("Roll back")),
            Err("That passage isn't in notes.md.".to_owned())
        );
        assert_eq!(
            error(&notes, heading("Rollout")),
            Err("There's no heading “Rollout” in notes.md.".to_owned())
        );
        assert_eq!(
            error(&plan, AgentTarget::Page { page: 2 }),
            Err("A page target is for PDFs; give a heading, a line or a quote.".to_owned())
        );
        assert_eq!(
            error(&brief, quote("Brief")),
            Err("brief.pdf is a PDF: give a page target.".to_owned())
        );
        // A comment thread that isn't there.
        let comment = AgentTarget::Comment {
            comment: "C1".to_owned(),
        };
        assert_eq!(
            error(&plan, comment),
            Err("There's no thread C1 in plan.md: it has no comments yet.".to_owned())
        );
        assert!(agent_requests(&f.host).is_empty());
        assert!(lock(&f.host.agent_focused).is_empty());
    }

    /// An HTML page's target goes to the window as it is, to find in what the page reports.
    #[test]
    fn a_pages_target_goes_to_the_window_to_find() {
        let (f, _) = shown_plan();
        let report = f
            .dir
            .file("vault/report.html", "<h1>Report</h1><p>Rose.</p>");
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        f.app
            .agent_answer(show_at(&report, quote("Rose")), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        assert_eq!(shown_target(call), (Some(quote("Rose")), vec![]));
    }

    /// A window still starting, or a new one, goes to the target once the file has opened: its
    /// startup payload carries it, with where core found it in a note.
    #[test]
    fn a_target_reaches_a_window_still_starting() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", ROLLOUT);
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let (answer, payload, _) = shown_as_it_starts(&f, show_at(&plan, heading("rollout")));
        let answer = answer.unwrap();
        assert_eq!(answer["state"], "shown");
        // Core found it: nothing pending.
        assert_eq!(answer.get("target"), None);
        assert!(agent_requests(&f.host).is_empty());
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
        assert_eq!(
            payload.agent_target,
            Some(ShowAt {
                target: heading("rollout"),
                place: Some(place((3, 3), (5, 12), 0)),
            })
        );
        // A PDF no workspace holds, in a new window, at its page.
        let brief = f.dir.file("elsewhere/brief.pdf", "%PDF-1.4\n");
        let (answer, payload, window) = shown_as_it_starts(
            &f,
            AgentRequest::Show(ShowArgs {
                path: path_string(&brief),
                workspace: None,
                target: Some(AgentTarget::Page { page: 3 }),
                new_window: true,
                wait: 0,
            }),
        );
        let answer = answer.unwrap();
        assert_eq!(answer["window"].as_str(), Some(window.label()));
        assert_ne!(window.label(), "main");
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), brief);
        assert_eq!(
            payload.agent_target,
            Some(ShowAt {
                target: AgentTarget::Page { page: 3 },
                place: None,
            })
        );
        assert_eq!(answer.get("target"), None);
        // An HTML page's passage, which the new window finds once the page has loaded: pending.
        let report = f.dir.file("elsewhere/report.html", "<p>Rose.</p>");
        let (answer, _, _) = shown_as_it_starts(
            &f,
            AgentRequest::Show(ShowArgs {
                path: path_string(&report),
                workspace: None,
                target: Some(quote("Rose")),
                new_window: true,
                wait: 0,
            }),
        );
        let answer = answer.unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["target"], "pending");
    }

    /// `highlight` goes to the window showing the file, the shown one by default, with where core
    /// found each target; it answers how many highlights the window shows now, and brings no
    /// window forward.
    #[test]
    fn highlight_goes_to_the_window_showing_the_file_with_where_each_target_is() {
        let (f, plan) = shown_plan();
        for path in [Some(plan.as_path()), None] {
            let answering = ui_answers(&f, "main", vec![AgentReply::Highlighted { count: 2 }]);
            let answer = f
                .app
                .agent_answer(
                    highlight(path, vec![heading("Rollout"), quote("migration")], false),
                    &claude(),
                )
                .unwrap();
            let (target, call) = answering.join().unwrap();
            assert_eq!(target, Target::Window("main".to_owned()));
            let AgentRequest::Highlight(args) = &call.request else {
                panic!("not a highlight: {:?}", call.request);
            };
            assert!(same_path(Path::new(args.path.as_deref().unwrap()), &plan));
            assert_eq!(
                call.places,
                [place((3, 3), (5, 12), 0), place((5, 5), (27, 36), 14)]
            );
            assert_eq!(answer["window"], "main");
            assert_eq!(answer["highlights"], 2);
            assert!(same_path(
                Path::new(answer["file"]["windows"].as_str().unwrap()),
                &plan
            ));
        }
        // Clearing alone reads nothing: the window is only told.
        let answering = ui_answers(&f, "main", vec![AgentReply::Highlighted { count: 0 }]);
        let answer = f
            .app
            .agent_answer(highlight(None, vec![], true), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        assert!(call.places.is_empty());
        assert_eq!(answer["highlights"], 0);
        assert!(lock(&f.host.agent_focused).is_empty());
    }

    /// A file no window shows can't be highlighted or read, nor a passage that isn't there:
    /// nothing is sent.
    #[test]
    fn highlight_and_read_need_the_file_shown_and_the_target_there() {
        let (f, plan) = shown_plan();
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        let not_shown = Err("notes.md isn't shown in Lectern. Show it first.".to_owned());
        assert_eq!(
            f.app.agent_answer(
                highlight(Some(&notes), vec![quote("Notes")], false),
                &claude()
            ),
            not_shown
        );
        assert_eq!(
            f.app
                .agent_answer(read_from(Some(&notes), None, None), &claude()),
            not_shown
        );
        assert_eq!(
            f.app.agent_answer(
                highlight(Some(&plan), vec![quote("roll forward")], false),
                &claude()
            ),
            Err("That passage isn't in plan.md.".to_owned())
        );
        assert!(agent_requests(&f.host).is_empty());
        // A window showing nothing has no file to go by.
        let f = fixture(profile(&[]), FakeHost::default());
        let nothing = Err("Lectern isn't showing a file. Show one first.".to_owned());
        assert_eq!(
            f.app
                .agent_answer(highlight(None, vec![quote("x")], false), &claude()),
            nothing
        );
        assert_eq!(
            f.app.agent_answer(read_from(None, None, None), &claude()),
            nothing
        );
    }

    /// A note's text is read by Rust, from the file, a page at a time: the window isn't asked.
    #[test]
    fn read_gives_a_notes_text_a_page_at_a_time() {
        let (f, plan) = shown_plan();
        let answer = f
            .app
            .agent_answer(read_from(None, Some(5), Some(7)), &claude())
            .unwrap();
        assert_eq!(answer["text"], "Rollout");
        assert_eq!(answer["truncated"], true);
        assert_eq!(answer["next"], 12);
        assert_eq!(
            answer["blocks"],
            json!([{"start": 0, "line": 3, "heading": 0}])
        );
        assert_eq!(answer["headings"], json!(["Plan › Rollout"]));
        assert_eq!(answer["truncated"], true);
        assert!(same_path(
            Path::new(answer["file"]["windows"].as_str().unwrap()),
            &plan
        ));
        assert!(agent_requests(&f.host).is_empty());
    }

    /// An HTML page's text is what its window's page reported, scripts and all, and its window
    /// finds a highlight's passage; a PDF has no text to read.
    #[test]
    fn a_page_is_read_and_highlighted_by_its_window_and_a_pdf_has_no_text() {
        let (f, _) = shown_plan();
        let report = f.dir.file("vault/report.html", "<h1>Report</h1>");
        let _ = f.state.open_document(&path_string(&report));
        let blocks = vec![
            PageBlock {
                text: "Report".to_owned(),
                heading_level: Some(1),
            },
            PageBlock {
                text: "Made by a script.".to_owned(),
                heading_level: None,
            },
        ];
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::PageText {
                blocks,
                truncated: false,
            }],
        );
        let answer = f
            .app
            .agent_answer(read_from(None, None, None), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        assert!(matches!(call.request, AgentRequest::Read(_)));
        assert_eq!(answer["text"], "Report Made by a script.");
        assert_eq!(answer["blocks"][1], json!({"start": 7, "heading": 0}));
        assert_eq!(answer["headings"], json!(["Report"]));
        assert_eq!(answer["blocks"][0], json!({"start": 0, "heading": 0}));
        assert_eq!(answer.get("incomplete"), None);
        // A page its bridge cut short at its limit reads as incomplete.
        let cut = vec![PageBlock {
            text: "Rows.".to_owned(),
            heading_level: None,
        }];
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::PageText {
                blocks: cut,
                truncated: true,
            }],
        );
        let answer = f
            .app
            .agent_answer(read_from(None, None, None), &claude())
            .unwrap();
        answering.join().unwrap();
        assert_eq!(answer["text"], "Rows.");
        assert_eq!(answer["truncated"], true);
        assert_eq!(answer["incomplete"], "page text over 2 MiB");
        assert_eq!(answer.get("next"), None);
        let missing = "That passage isn't in report.html.";
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Failed {
                message: missing.to_owned(),
            }],
        );
        assert_eq!(
            f.app
                .agent_answer(highlight(None, vec![quote("Rose")], false), &claude()),
            Err(missing.to_owned())
        );
        let (_, call) = answering.join().unwrap();
        assert!(call.places.is_empty());
        let brief = f.dir.file("vault/brief.pdf", "%PDF-1.4\n");
        let _ = f.state.open_document(&path_string(&brief));
        let sent = agent_requests(&f.host).len();
        assert_eq!(
            f.app.agent_answer(read_from(None, None, None), &claude()),
            Err("PDF text isn't available.".to_owned())
        );
        // A PDF can't be painted: highlighting one, or clearing it, is the tool's error.
        for (targets, clear) in [(vec![quote("Brief")], false), (vec![], true)] {
            assert_eq!(
                f.app
                    .agent_answer(highlight(None, targets, clear), &claude()),
                Err("PDFs can't be highlighted; use show with a page target.".to_owned())
            );
        }
        assert_eq!(agent_requests(&f.host).len(), sent);
    }

    /// An HTML page's target is found by its window once the page has loaded. The window has as
    /// long as the pipe gives it to say: one that isn't there is the tool's error, as in a note,
    /// and one the page hadn't loaded in time for is `pending`.
    #[test]
    fn a_pages_target_still_loading_is_pending_and_one_not_there_is_the_tools_error() {
        let (f, _) = shown_plan();
        let report = f
            .dir
            .file("vault/report.html", "<h1>Report</h1><p>Rose.</p>");
        let pending = AgentReply::Shown {
            target: Some(TargetState::Pending),
        };
        let answering = ui_answers(&f, "main", vec![pending]);
        let answer = f
            .app
            .agent_answer(show_at(&report, quote("Rose")), &claude())
            .unwrap();
        let (_, call) = answering.join().unwrap();
        let within = call.within_ms.expect("how long the window has");
        assert!(
            within > 0 && u128::from(within) <= UI_WAIT.as_millis(),
            "{within}"
        );
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["target"], "pending");
        let missing = "That passage isn't in report.html.";
        let answering = ui_answers(
            &f,
            "main",
            vec![AgentReply::Failed {
                message: missing.to_owned(),
            }],
        );
        assert_eq!(
            f.app
                .agent_answer(show_at(&report, quote("Tulip")), &claude()),
            Err(missing.to_owned())
        );
        answering.join().unwrap();
        // A newer request took its place before the page loaded.
        let answering = ui_answers(&f, "main", vec![AgentReply::Replaced]);
        let answer = f
            .app
            .agent_answer(show_at(&report, quote("Rose")), &claude())
            .unwrap();
        answering.join().unwrap();
        assert_eq!(answer["state"], "replaced");
        // Found, or in a note, the answer has no target to tell of.
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown { target: None }]);
        let answer = f
            .app
            .agent_answer(show_at(&report, quote("Rose")), &claude())
            .unwrap();
        answering.join().unwrap();
        assert_eq!(answer.get("target"), None);
    }

    /// A text file over the size an open shows is refused before it is read, as the open is.
    #[test]
    fn a_target_in_a_text_file_too_large_to_show_is_refused_unread() {
        let (f, _) = shown_plan();
        let huge = f.dir.file("vault/huge.txt", "");
        fs::File::options()
            .write(true)
            .open(&huge)
            .unwrap()
            .set_len(20 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            f.app
                .agent_answer(show_at(&huge, AgentTarget::Line { line: 1 }), &claude()),
            Err("This file is too large to show (over 20 MB).".to_owned())
        );
        assert!(agent_requests(&f.host).is_empty());
    }

    /// The window finds its agent's highlights again after the note changes: core looks for each
    /// target in the file as it is now, and one that's gone is none.
    #[test]
    fn the_window_finds_its_notes_targets_again_through_core() {
        let (f, plan) = shown_plan();
        let path = path_string(&plan);
        let targets = [
            quote("Roll back"),
            heading("Rollout"),
            quote("Roll forward"),
        ];
        assert_eq!(
            f.state.agent_places(&path, &targets),
            Ok(vec![
                Some(place((5, 5), (13, 22), 0)),
                Some(place((3, 3), (5, 12), 0)),
                None
            ])
        );
        // Changed on disk: "Plan Intro. Rollout Roll forward, then roll back." Each is looked
        // for again.
        fs::write(
            &plan,
            "# Plan\n\nIntro.\n\n## Rollout\n\nRoll forward, then roll back.\n",
        )
        .unwrap();
        assert_eq!(
            f.state.agent_places(&path, &targets),
            Ok(vec![
                None,
                Some(place((5, 5), (12, 19), 0)),
                Some(place((7, 7), (20, 32), 0))
            ])
        );
        // Only for the file the window shows.
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        assert_eq!(
            f.state.agent_places(&path_string(&notes), &targets),
            Err("That file isn't shown in this window.".to_owned())
        );
    }

    /// A window whose UI leaves its document for none (the welcome page, a file deleted while
    /// open) shows no file to Rust either: an agent can't read, highlight or comment on it, and an
    /// open begun before it left doesn't take its place afterwards.
    #[test]
    fn a_window_that_leaves_its_document_shows_none() {
        let (f, plan) = shown_plan();
        let path = path_string(&plan);
        // An open numbered 1 is on its way as the window leaves, numbered 2.
        f.state.leave_document(2);
        assert_eq!(f.state.known_view().path, None);
        assert_eq!(
            f.app.agent_answer(read_from(None, None, None), &claude()),
            Err("Lectern isn't showing a file. Show one first.".to_owned())
        );
        assert_eq!(
            f.app.agent_answer(
                highlight(Some(&plan), vec![quote("Roll back")], false),
                &claude()
            ),
            Err("plan.md isn't shown in Lectern. Show it first.".to_owned())
        );
        assert!(f.state.load_review(&path, None).is_err());
        assert!(f.state.agent_places(&path, &[quote("Roll back")]).is_err());
        // The open numbered before it left lands after: it isn't on screen.
        assert!(!f.state.make_current(1, &plan, None));
        assert_eq!(f.state.known_view().path, None);
        // A new open is.
        let _ = f.state.open_document(&path);
        assert!(f.state.shown_file().is_some());
    }

    /// A window whose file was deleted while open, and says so, shows no file to an agent either,
    /// and an open begun before doesn't take its place afterwards; but Rust still watches the
    /// file, so its return reloads the window, which then shows it again.
    #[test]
    fn a_window_whose_file_went_missing_shows_none_but_watches_for_its_return() {
        let (f, plan) = shown_plan();
        let path = path_string(&plan);
        // An open numbered 1 is on its way as the file goes missing, which the window says,
        // numbered 2.
        fs::remove_file(&plan).unwrap();
        lock(&f.watched).clear();
        f.state.document_missing(2);
        assert_eq!(
            f.app
                .agent_answer(read_from(Some(&plan), None, None), &claude()),
            Err("plan.md isn't shown in Lectern. Show it first.".to_owned())
        );
        assert_eq!(
            f.app.agent_answer(read_from(None, None, None), &claude()),
            Err("Lectern isn't showing a file. Show one first.".to_owned())
        );
        assert!(f.state.agent_places(&path, &[quote("Roll back")]).is_err());
        // Still watched: the watcher was never told to stop.
        assert_eq!(*lock(&f.watched), Vec::new());
        // The open numbered before it went missing lands after: it isn't on screen.
        assert!(!f.state.make_current(1, &plan, None));
        // It comes back, and the window opens it again: it's shown.
        fs::write(&plan, ROLLOUT).unwrap();
        let _ = f.state.open_document(&path);
        assert_eq!(f.state.shown_file(), Some(plan.clone()));
    }

    /// A window that drops an open on its way and keeps the file it shows (an agent's newer show
    /// of that file) keeps it as Rust's document too: an open begun before doesn't take its place.
    /// When one already had, it says so, for the window to open its file again.
    #[test]
    fn a_window_that_keeps_its_file_takes_no_open_begun_before() {
        let (f, plan) = shown_plan();
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        // An open numbered 1 is on its way as the window keeps its file, numbered 2.
        let rendered = f.state.load(&notes).ok();
        assert!(f.state.keep_document(&plan, 2));
        assert!(!f.state.make_current(1, &notes, rendered.as_ref()));
        assert_eq!(f.state.shown_file(), Some(plan.clone()));
        // One that became the document first: the file on screen isn't kept.
        let _ = f.state.open_document(&path_string(&notes));
        assert!(!f.state.keep_document(&plan, 3));
        // Opened again, it is.
        let _ = f.state.open_document(&path_string(&plan));
        assert!(f.state.keep_document(&plan, 4));
        assert_eq!(f.state.shown_file(), Some(plan));
    }

    /// The window's UI numbers its opens, and its leaves and keeps of its document, in the order
    /// it sends them; Rust may run them in another (an open's command runs once the pool gets to
    /// it, a keep at once). An open numbered below the window's last keep never becomes current,
    /// whichever ran first: one run after the keep is refused, and one run before it is current
    /// until then, which the keep says, so the window opens its file again.
    #[test]
    fn an_open_numbered_before_the_window_kept_its_file_never_becomes_current() {
        let (f, plan) = shown_plan();
        let notes = f.dir.file("vault/notes.md", "# Notes\n");
        let other = f.dir.file("vault/other.md", "# Other\n");
        // Sent before the keep (1 and 2), run after it (3): neither becomes current.
        assert!(f.state.keep_document(&plan, 3));
        let opened = f.state.open_numbered(1, &path_string(&notes));
        assert!(matches!(opened, OpenResult::Ok { .. }));
        assert_eq!(f.state.shown_file(), Some(plan.clone()));
        let opened = f.state.open_user_path_numbered(2, &path_string(&other));
        assert!(opened.doc.is_some());
        assert_eq!(f.state.shown_file(), Some(plan.clone()));
        assert!(f.state.keep_document(&plan, 4));
        // Run before the keep sent after it (5, then 6): current until the keep, which says the
        // plan isn't, so the window opens it again (7), and that is.
        let _ = f.state.open_numbered(5, &path_string(&notes));
        assert_eq!(f.state.shown_file(), Some(notes));
        assert!(!f.state.keep_document(&plan, 6));
        let _ = f.state.open_numbered(7, &path_string(&plan));
        assert!(f.state.keep_document(&plan, 8));
        assert_eq!(f.state.shown_file(), Some(plan));
    }
}
