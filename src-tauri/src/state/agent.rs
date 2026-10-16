//! What the app answers agents with (`agent.rs` serves the pipe):
//!
//! - **`status`:** every window's view, which each window's UI reports (`agent-request`,
//!   answered by the command `agent_reply`).
//! - **`show`:** the file goes to the window `route_show` picks. A window whose UI is up decides
//!   itself whether to ask the reader first (the bar) and answers `shown` or `asked`; a window
//!   still starting, or a new one, opens the file first thing, unless it holds a file the user
//!   launched, which an agent's never takes the place of. The window then comes forward as
//!   `agentFocus` has it. With a `wait`, an `asked` waits that long for what became of the
//!   request, which the UI sends once it is settled (`AgentReply::Answered`). A window that turns
//!   to another workspace meanwhile (its page reloads) dismisses it; one that closes is the tool's
//!   error.
//! - **`workspace`:** switches to a workspace, bringing its window forward or reopening it in a
//!   new one, or creates one in a new window; an open window never changes workspace.
//!
//! Both answer within the pipe's time (`ANSWER_WITHIN`, and the reader's `wait`), a second to
//! spare: each step that waits waits only as long as is left of it (`Budget`).
//!
//! An agent's path on a network share is taken only when Lectern already trusts the share's host:
//! a library's, a path mapping's, WSL's, or one the user opened a file from (`agent_location`).
//! An agent's open never adds one. That differs from the user's own opens (a launch, Explorer, the
//! file dialog), which make the host trusted for the session: an agent's path may come from text a
//! note fed it, and reaching a new host would hand that host the user's credentials. The other
//! tools are not available yet.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::agent::paths::{agent_path, AgentPath};
use lectern_core::agent::request::{ShowArgs, Target, WorkspaceAction, WorkspaceArgs};
use lectern_core::agent::show::{
    no_such_workspace, AgentRoute, FocusStep, ShowAnswer, ShowState, WorkspaceAnswer,
};
use lectern_core::agent::status::{
    window_status, AgentCall, AgentReply, AgentSettings, AskAnswer, Status, WindowView,
};
use lectern_core::agent::{AgentRequest, ANSWER_WITHIN};
use lectern_core::ipc::{AgentFocus, OpenRequest};
use lectern_core::library::pathmap::PathMapper;
use serde::Serialize;
use serde_json::{json, Value};

use super::open_queue::AgentOffer;
use super::paths::{normalize_root, path_string, same_path};
use super::sync::{lock, read};
use super::trust::Trust;
use super::windows::is_folder;
use super::{App, WindowState};
use crate::events::UiEvent;

/// How long `status` waits for the windows' UIs to say what they show. A window that hasn't
/// answered by then is reported from what Rust knows of it: its workspace and its file.
const VIEW_WAIT: Duration = Duration::from_secs(3);

/// How long `show` waits for the window's UI to open the file or ask the reader.
const SHOW_WAIT: Duration = Duration::from_secs(5);

/// What `show` and `workspace` keep of the pipe's time (`ANSWER_WITHIN`) to answer in.
const ANSWER_SPARE: Duration = Duration::from_secs(1);

/// The tool error when the window a request went to closes before it is settled.
const CLOSED: &str = "That window has closed.";

/// The tool error for an agent's `show` while the window it goes to starts with a file the user
/// launched.
const OPENING_FOR_USER: &str = "Lectern is opening a file for the user; try again in a moment.";

/// The answer to a tool Lectern doesn't offer yet.
pub const NOT_YET: &str = "Not yet available.";

/// The requests sent to windows' UIs that wait for an answer, by id, with the window each went
/// to; and the `show`s whose agents wait for what becomes of them, by the same id.
#[derive(Default)]
pub struct AgentCalls {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, (String, Sender<AgentReply>)>>,
    asks: Mutex<HashMap<u64, (String, Sender<Settled>)>>,
}

/// What became of a `show` the window asked the reader about, with why when it `failed`.
type Settled = (AskAnswer, Option<String>);

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
    /// of a `show` goes to whoever waits for that.
    fn answer(&self, label: &str, id: u64, reply: AgentReply) -> bool {
        if let AgentReply::Answered { answer, message } = reply {
            return take_for(&self.asks, label, id)
                .is_some_and(|s| s.send((answer, message)).is_ok());
        }
        take_for(&self.waiting, label, id).is_some_and(|s| s.send(reply).is_ok())
    }

    /// Stops waiting on request `id`, and on what becomes of it.
    fn forget(&self, id: u64) {
        lock(&self.waiting).remove(&id);
        lock(&self.asks).remove(&id);
    }

    /// Stops waiting on every request sent to the window `label`, and on what becomes of them, as
    /// its state is let go of (`WindowState::retire`): whoever waits on one wakes at once, finding
    /// nothing will answer.
    pub(super) fn forget_window(&self, label: &str) {
        lock(&self.waiting).retain(|_, (asked, _)| asked != label);
        lock(&self.asks).retain(|_, (asked, _)| asked != label);
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
    waiting: &Mutex<HashMap<u64, (String, Sender<T>)>>,
    label: &str,
    id: u64,
) -> Option<Sender<T>> {
    let mut waiting = lock(waiting);
    if !waiting.get(&id).is_some_and(|(asked, _)| asked == label) {
        return None;
    }
    waiting.remove(&id).map(|(_, sender)| sender)
}

/// Where a `show` went: a window whose UI is up, or one that opens the file as it starts (a new
/// window when `new`).
enum Placed {
    Ready(Arc<WindowState>),
    Starting {
        label: String,
        new: bool,
        reopened: bool,
    },
}

impl App {
    /// The answer to the request of the agent `agent` (its name as the reader sees it): `status`,
    /// `show` and `workspace`, or a tool error for the tools not offered yet.
    pub fn agent_answer(
        self: &Arc<Self>,
        request: AgentRequest,
        agent: &str,
    ) -> Result<Value, String> {
        match request {
            AgentRequest::Status => to_value(&self.agent_status(agent)),
            AgentRequest::Show(args) => to_value(&self.agent_show(args, agent, Budget::new())?),
            AgentRequest::Workspace(args) => to_value(&self.agent_workspace(args, Budget::new())?),
            // The connection answers it; nothing reaches here.
            AgentRequest::Hello { .. } => Ok(json!({})),
            _ => Err(NOT_YET.to_owned()),
        }
    }

    /// The window `label`'s UI answers the request `id` it was sent. False when nothing waits on
    /// it any more.
    pub fn agent_reply(&self, label: &str, id: u64, reply: AgentReply) -> bool {
        self.agent_calls.answer(label, id, reply)
    }

    /// Every window, the most recently focused first, as its UI reports it (else as Rust knows
    /// it), with the closed workspaces and the agent settings.
    pub fn agent_status(&self, agent: &str) -> Status {
        let focus = lock(&self.focus).clone();
        let mut windows = self.windows();
        windows.sort_by_key(|w| {
            focus
                .iter()
                .position(|label| label == w.label())
                .unwrap_or(usize::MAX)
        });
        // Every window is asked at once, then each answer awaited until the one deadline.
        let asked: Vec<(Arc<WindowState>, u64, Receiver<AgentReply>)> = windows
            .into_iter()
            .map(|window| {
                let (id, answer) = self.agent_calls.open(window.label());
                window.emit(UiEvent::AgentRequest(AgentCall {
                    id,
                    request: AgentRequest::Status,
                    agent: agent.to_owned(),
                }));
                (window, id, answer)
            })
            .collect();
        let deadline = Instant::now() + VIEW_WAIT;
        let mapper = self.mapper();
        let windows = asked
            .into_iter()
            .map(|(window, id, answer)| {
                let view =
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
    /// its target (a line, for now; the path's own `:12` when no target is given), within
    /// `budget`. It must be a file, on a share only when Lectern already trusts its host, which is
    /// checked first.
    fn agent_show(
        self: &Arc<Self>,
        args: ShowArgs,
        agent: &str,
        budget: Budget,
    ) -> Result<ShowAnswer, String> {
        let path = self.agent_location(&args.path)?;
        let file = PathBuf::from(&path.windows);
        check_file(&file, budget.at_most(self.timings.exists))?;
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
            // but trusting nothing; unless it holds a launch's, which goes first.
            match window.opens.offer_from_agent(request) {
                AgentOffer::Held => Placed::Starting {
                    label,
                    new,
                    reopened,
                },
                AgentOffer::Now => Placed::Ready(window),
                AgentOffer::Refused => return Err(OPENING_FOR_USER.to_owned()),
            }
        };
        let (label, state, focus) = match placed {
            Placed::Starting {
                label,
                new,
                reopened,
            } => {
                if new {
                    self.build(&label, front)?;
                    if reopened {
                        self.workspaces_changed();
                    }
                }
                let focus = self.agent_focus(&label, budget);
                (label, ShowState::Shown, focus)
            }
            Placed::Ready(window) => {
                let ShowArgs { target, wait, .. } = args;
                let shown = ShowArgs {
                    path: path.windows.clone(),
                    workspace: None,
                    target: target.or(path.line.map(|line| Target::Line { line })),
                    new_window: false,
                    wait,
                };
                let label = window.label().to_owned();
                let (state, focus) = self.show_in(&window, shown, agent, budget)?;
                (label, state, focus)
            }
        };
        let workspace = self.window(&label).and_then(|w| self.workspace_name(&w));
        Ok(ShowAnswer {
            state,
            file: path.forms(),
            window: label,
            workspace,
            focus,
        })
    }

    /// Asks the UI of `window`, which is up, to show `args.path`. It opens it, or asks the reader
    /// first; either way the window then comes forward. Asked, with a `wait`, what became of it is
    /// waited for that long. A window let go of before it settles the request (`let_go_of`) ends
    /// the wait at once.
    fn show_in(
        &self,
        window: &WindowState,
        args: ShowArgs,
        agent: &str,
        budget: Budget,
    ) -> Result<(ShowState, Option<FocusStep>), String> {
        let label = window.label();
        let wait = Duration::from_secs(args.wait.into());
        let (id, reply) = self.agent_calls.open(label);
        // Before the UI hears of it, so an answer however quick finds its waiter.
        let answered = (!wait.is_zero()).then(|| self.agent_calls.await_answer(label, id));
        // Let go of already, its requests went before these came: nothing would answer them.
        if window.is_retired() {
            self.agent_calls.forget(id);
            return Ok((self.let_go_of(label)?, None));
        }
        window.emit(UiEvent::AgentRequest(AgentCall {
            id,
            request: AgentRequest::Show(args),
            agent: agent.to_owned(),
        }));
        let state = match reply.recv_timeout(budget.at_most(SHOW_WAIT)) {
            Ok(AgentReply::Shown) => ShowState::Shown,
            Ok(AgentReply::Asked) => ShowState::Asked,
            Ok(AgentReply::Failed { message }) => {
                self.agent_calls.forget(id);
                return Err(message);
            }
            Ok(_) => {
                self.agent_calls.forget(id);
                return Err("The window gave an answer Lectern didn't expect.".to_owned());
            }
            Err(RecvTimeoutError::Timeout) => {
                self.agent_calls.forget(id);
                return Err("The window didn't answer in time.".to_owned());
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.agent_calls.forget(id);
                return Ok((self.let_go_of(label)?, None));
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
        Ok((state, focus))
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

    /// `workspace`: switches to the workspace named `args.name`, or creates it with `args.folders`,
    /// within `budget`.
    fn agent_workspace(
        self: &Arc<Self>,
        args: WorkspaceArgs,
        budget: Budget,
    ) -> Result<WorkspaceAnswer, String> {
        match args.action {
            WorkspaceAction::Switch => self.agent_switch(&args.name, budget),
            WorkspaceAction::Create => self.agent_create(&args.name, &args.folders, budget),
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
    /// WSL paths of folders), opened in a new window.
    fn agent_create(
        self: &Arc<Self>,
        name: &str,
        folders: &[String],
        budget: Budget,
    ) -> Result<WorkspaceAnswer, String> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for folder in folders {
            let root = normalize_root(&self.agent_location(folder)?.windows)?;
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

    /// The path an agent wrote, `raw`, as `agent_location` takes it.
    fn agent_location(&self, raw: &str) -> Result<AgentPath, String> {
        agent_location(&self.mapper(), &read(&self.trust), raw)
    }

    /// The name of the workspace `window` shows; none for a blank window.
    fn workspace_name(&self, window: &WindowState) -> Option<String> {
        window
            .workspace_id()
            .and_then(|id| self.read_workspace(&id, |ws| ws.name.clone()))
    }
}

impl WindowState {
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
    use lectern_core::agent::status::ViewSelection;
    use lectern_core::ipc::{AgentOpens, OpenWhere, PathMapping, WorkspaceOutcome};

    /// The `agent-request`s sent, in order, with the window each went to.
    fn agent_requests(host: &FakeHost) -> Vec<(Target, AgentCall)> {
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
        let status = f.app.agent_status("Claude");
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
        let status = f.app.agent_status("Claude");
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

    #[test]
    fn the_other_tools_are_not_available_yet() {
        let f = fixture(profile(&[]), FakeHost::default());
        let read = AgentRequest::Read(Default::default());
        assert_eq!(f.app.agent_answer(read, "Claude"), Err(NOT_YET.to_owned()));
    }

    // show

    /// Answers the next `agent-request` the window `label` is sent with `replies`, in order, from
    /// a thread of its own, which gives back the request it answered. Called before the request
    /// is sent.
    fn ui_answers(
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
            .agent_answer(show(&path_string(&notes), 0), "Claude")
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
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown]);
        let answer = f.app.agent_answer(show(&wsl, 0), "Claude").unwrap();
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
            let shown = f.app.agent_answer(show(&notes, 30), "Claude").unwrap();
            answering.join().unwrap();
            assert_eq!(shown["state"], state);
        }
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        let started = Instant::now();
        let shown = f.app.agent_answer(show(&notes, 1), "Claude").unwrap();
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
            f.app.agent_answer(show(&path_string(&plan), 5), "Claude"),
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
            f.app.agent_answer(show(&path_string(&notes), 30), "Claude"),
            Err(why.to_owned())
        );
        answering.join().unwrap();
    }

    /// The `show` `request` on a thread of its own, from Claude.
    fn showing(f: &Fixture, request: AgentRequest) -> thread::JoinHandle<Result<Value, String>> {
        let app = Arc::clone(&f.app);
        thread::spawn(move || app.agent_answer(request, "Claude"))
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
            .agent_answer(show(&path_string(&plan), 30), "Claude")
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
                .show_in(&turned, args.clone(), "Claude", Budget::new()),
            Ok((ShowState::Dismissed, None))
        );
        let closed = f.app.window("main").unwrap();
        f.app.window_destroyed("main");
        assert_eq!(
            f.app.show_in(&closed, args, "Claude", Budget::new()),
            Err(CLOSED.to_owned())
        );
        assert!(started.elapsed() < SHOW_WAIT);
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
        // A UI that doesn't answer: given 300 ms, the show gives up then, not after SHOW_WAIT.
        let started = Instant::now();
        assert_eq!(
            f.app
                .agent_show(args(), "Claude", within(Duration::from_millis(300)))
                .unwrap_err(),
            "The window didn't answer in time."
        );
        assert!(started.elapsed() < SHOW_WAIT);
        // Answered at once, the focus is given what is left of a second, not the host's 2 s.
        let answering = ui_answers(&f, "main", vec![AgentReply::Asked]);
        f.app
            .agent_show(args(), "Claude", within(Duration::from_secs(1)))
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
        let error = |path: &str| f.app.agent_answer(show(path, 0), "Claude").unwrap_err();
        assert!(error(&path_string(&folder)).contains("is a folder"));
        assert_eq!(
            error(&path_string(&missing)),
            format!("There's no file at {}.", missing.display())
        );
        assert!(error("notes/plan.md").contains("isn't a full path"));
        assert!(agent_requests(&f.host).is_empty());
        assert!(f.host.opened_windows().is_empty());
    }

    /// A window whose UI isn't up yet opens the file first thing, as a launch's: no bar to ask.
    #[test]
    fn a_window_still_starting_opens_the_file_as_it_starts() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let plan = dir.file("vault/plan.md", "# Plan\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let answer = f
            .app
            .agent_answer(show(&path_string(&plan), 0), "Claude")
            .unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["window"], "main");
        assert!(agent_requests(&f.host).is_empty());
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
    }

    /// A window still starting with a file the user launched opens that one: an agent's `show`
    /// meanwhile is refused, never taking its place.
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
        assert_eq!(
            f.app.agent_answer(show(&path_string(&notes), 0), "Claude"),
            Err(OPENING_FOR_USER.to_owned())
        );
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), plan);
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
        let answer = f.app.agent_answer(in_new(&loose), "Claude").unwrap();
        assert_eq!(answer["state"], "shown");
        assert_eq!(answer["workspace"], Value::Null);
        let label = answer["window"].as_str().unwrap().to_owned();
        assert_ne!(label, "main");
        assert_eq!(*lock(&f.host.opened), [(label.clone(), true)]);
        let window = f.app.window(&label).unwrap();
        assert_eq!(window.workspace_id(), None);
        assert!(agent_requests(&f.host).is_empty());
        let payload = window.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), loose);
        // Main's file goes to Main's window.
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown]);
        let answer = f.app.agent_answer(in_new(&plan), "Claude").unwrap();
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
            let answer = f
                .app
                .agent_answer(show(&path_string(&seeds), 0), "Claude")
                .unwrap();
            assert_eq!(answer["state"], "shown");
            assert_eq!(answer["workspace"], "Garden");
            assert_eq!(answer["focus"], step);
            let label = answer["window"].as_str().unwrap().to_owned();
            assert_eq!(*lock(&f.host.opened), [(label.clone(), takes_focus)]);
            assert_eq!(*lock(&f.host.agent_focused), [(label.clone(), focus)]);
            let window = f.app.window(&label).unwrap();
            assert_eq!(window.workspace_id(), Some(garden));
            assert!(agent_requests(&f.host).is_empty());
            let payload = window.startup();
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
        let answering = ui_answers(&f, "main", vec![AgentReply::Shown]);
        let answer = f.app.agent_answer(in_main("main"), "Claude").unwrap();
        answering.join().unwrap();
        assert_eq!(
            (answer["window"].as_str(), answer["workspace"].as_str()),
            (Some("main"), Some("Main"))
        );
        assert_eq!(
            f.app.agent_answer(in_main("Attic"), "Claude"),
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
            f.state.opens.offer_from_agent(request("a.md")),
            AgentOffer::Held
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
            f.app.agent_answer(show(shared, 0), "Claude"),
            Err(UNTRUSTED.to_owned())
        );
        assert!(!f.app.trusts(shared));
        let over_share = workspace(
            WorkspaceAction::Create,
            "Shared",
            &[r"\\nas2\share".to_owned()],
        );
        assert_eq!(
            f.app.agent_answer(over_share, "Claude"),
            Err(UNTRUSTED.to_owned())
        );
        assert!(read(&f.app.workspaces).find_by_name("Shared").is_none());
        assert!(agent_requests(&f.host).is_empty());
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
            .agent_answer(workspace(WorkspaceAction::Switch, " MAIN ", &[]), "Claude")
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
            .agent_answer(workspace(WorkspaceAction::Switch, "garden", &[]), "Claude")
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
            .agent_answer(workspace(WorkspaceAction::Switch, "Garden", &[]), "Claude")
            .unwrap();
        assert_eq!(
            (again["window"].as_str(), again["opened"].as_bool()),
            (Some(label.as_str()), Some(false))
        );
        assert_eq!(
            f.app
                .agent_answer(workspace(WorkspaceAction::Switch, "Attic", &[]), "Claude"),
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
                "Claude",
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
                .agent_answer(workspace(WorkspaceAction::Create, "garden", &[]), "Claude"),
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
                "Claude",
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
                    "Claude",
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
}
