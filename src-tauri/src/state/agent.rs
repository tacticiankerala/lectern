//! What the app answers agents with (`agent.rs` serves the pipe): `status` from every window's
//! view, which each window's UI reports (`agent-request`, answered by the command
//! `agent_reply`). The other tools are not available yet.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lectern_core::agent::status::{
    window_status, AgentCall, AgentReply, AgentSettings, Status, WindowView,
};
use lectern_core::agent::AgentRequest;
use serde_json::{json, Value};

use super::paths::path_string;
use super::sync::{lock, read};
use super::{App, WindowState};
use crate::events::UiEvent;

/// How long `status` waits for the windows' UIs to say what they show. A window that hasn't
/// answered by then is reported from what Rust knows of it: its workspace and its file.
const VIEW_WAIT: Duration = Duration::from_secs(3);

/// The answer to a tool Lectern doesn't offer yet.
pub const NOT_YET: &str = "Not yet available.";

/// The requests sent to windows' UIs that wait for an answer, by id, with the window each went
/// to.
#[derive(Default)]
pub struct AgentCalls {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, (String, Sender<AgentReply>)>>,
}

impl AgentCalls {
    /// A new request to the window `label`: its id, and where its answer arrives.
    fn open(&self, label: &str) -> (u64, Receiver<AgentReply>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        lock(&self.waiting).insert(id, (label.to_owned(), sender));
        (id, receiver)
    }

    /// Hands `reply` to whoever waits on request `id`, if the window `label` was asked it; false
    /// when no one waits (it took too long, or the id isn't one sent to that window).
    fn answer(&self, label: &str, id: u64, reply: AgentReply) -> bool {
        let mut waiting = lock(&self.waiting);
        if !waiting.get(&id).is_some_and(|(asked, _)| asked == label) {
            return false;
        }
        let Some((_, sender)) = waiting.remove(&id) else {
            return false;
        };
        sender.send(reply).is_ok()
    }

    /// Stops waiting on request `id`.
    fn forget(&self, id: u64) {
        lock(&self.waiting).remove(&id);
    }
}

impl App {
    /// The answer to an agent's request: `status`, or a tool error for the tools not offered
    /// yet.
    pub fn agent_answer(&self, request: AgentRequest) -> Result<Value, String> {
        match request {
            AgentRequest::Status => {
                serde_json::to_value(self.agent_status()).map_err(|e| e.to_string())
            }
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
    pub fn agent_status(&self) -> Status {
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
                        Err(_) => {
                            self.agent_calls.forget(id);
                            window.known_view()
                        }
                    };
                let workspace = window
                    .workspace_id()
                    .and_then(|id| self.read_workspace(&id, |ws| ws.name.clone()));
                window_status(window.label(), workspace, view, &mapper)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Target;
    use crate::state::test_support::*;
    use lectern_core::agent::status::ViewSelection;
    use lectern_core::ipc::AgentOpens;
    use std::thread;

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
            };
            assert!(app.agent_reply("main", call.id, AgentReply::View(view)));
        });
        let status = f.app.agent_status();
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
        let status = f.app.agent_status();
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
        assert_eq!(f.app.agent_answer(read), Err(NOT_YET.to_owned()));
    }
}
