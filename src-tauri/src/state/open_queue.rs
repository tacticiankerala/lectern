//! Second launches that arrive before the app can route them, or before a window's UI can receive
//! events.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

use lectern_core::agent::status::{AskAnswer, AskOutcome, ANSWERS_KEPT};
use lectern_core::agent::target::ShowAt;
use lectern_core::ipc::{OpenRequest, OpenResult};

use super::sync::lock;

/// What became of an agent's request to show a file: the reader's answer, or the window's (shown,
/// failed or replaced), with why when it `failed`.
pub(super) type Settled = (AskAnswer, Option<String>);

/// Files that second launches, or agents, ask to open. Until the UI has asked for its startup
/// payload it can't receive events, so the latest request is held for `startup` instead; an
/// agent's never takes the place of a launch's. What becomes of each agent's request held here
/// (shown, failed, or replaced by a newer one) reaches its `show` and is kept for `status`.
#[derive(Default)]
pub struct OpenQueue {
    inner: Mutex<Queue>,
}

#[derive(Default)]
struct Queue {
    /// The UI has asked for its startup payload: requests go to it now.
    ready: bool,
    held: Option<HeldOpen>,
    /// What became of the agents' requests held here, the oldest first: the latest
    /// `ANSWERS_KEPT`.
    answers: Vec<AskOutcome>,
}

/// A request held for the UI's startup, and whether an agent made it: an agent's file opens as it
/// is, trusting nothing, where a launch's makes its host trusted. An agent's may have a target,
/// which the UI goes to once the file has opened, and has its `show` waiting to hear what became of
/// it (`settle`).
pub(super) struct HeldOpen {
    pub(super) request: OpenRequest,
    pub(super) by_agent: bool,
    pub(super) at: Option<ShowAt>,
    pub(super) settle: Option<Sender<Settled>>,
}

/// What became of an agent's request offered to a window's queue (`offer_from_agent`).
pub(super) enum AgentOffer {
    /// The UI is up: the request goes to it now, not held.
    Now,
    /// Held for the UI's startup: what becomes of it arrives here.
    Held(Receiver<Settled>),
    /// The UI's startup holds a file the user launched, which an agent's never takes the place of.
    Refused,
}

impl Queue {
    /// Settles an agent's request for `path` as `settled`: its `show`, if one still waits, hears
    /// it, and `status` keeps it.
    fn settle(&mut self, path: &str, settled: Settled, to: Option<Sender<Settled>>) {
        let (answer, message) = settled.clone();
        self.answers.push(AskOutcome {
            path: path.to_owned(),
            answer,
            message,
        });
        let over = self.answers.len().saturating_sub(ANSWERS_KEPT);
        self.answers.drain(..over);
        if let Some(to) = to {
            // Gone when the show stopped waiting: `status` has it.
            let _ = to.send(settled);
        }
    }

    /// Holds `held`, settling an agent's request it takes the place of as replaced.
    fn hold(&mut self, held: HeldOpen) {
        if let Some(old) = self.held.replace(held).filter(|old| old.by_agent) {
            self.settle(&old.request.path, (AskAnswer::Replaced, None), old.settle);
        }
    }
}

impl OpenQueue {
    /// Holds `request`, a launch's, while the UI isn't ready, or hands it back to be sent now. An
    /// agent's request it takes the place of is replaced.
    pub fn offer(&self, request: OpenRequest) -> Option<OpenRequest> {
        let mut inner = lock(&self.inner);
        if inner.ready {
            return Some(request);
        }
        inner.hold(HeldOpen {
            request,
            by_agent: false,
            at: None,
            settle: None,
        });
        None
    }

    /// As `offer`, for an agent's request, going to `at` once open, which is refused while a
    /// launch's is held. Held, it takes the place of an agent's held before it, which is replaced,
    /// and what becomes of it arrives on the receiver `AgentOffer::Held` gives.
    pub(super) fn offer_from_agent(&self, request: OpenRequest, at: Option<ShowAt>) -> AgentOffer {
        let mut inner = lock(&self.inner);
        if inner.ready {
            return AgentOffer::Now;
        }
        if inner.held.as_ref().is_some_and(|held| !held.by_agent) {
            return AgentOffer::Refused;
        }
        let (settle, settled) = mpsc::channel();
        inner.hold(HeldOpen {
            request,
            by_agent: true,
            at,
            settle: Some(settle),
        });
        AgentOffer::Held(settled)
    }

    /// Whether a request is held for the UI.
    pub(super) fn has_pending(&self) -> bool {
        lock(&self.inner).held.is_some()
    }

    /// Marks the UI ready and returns the request held for it, if any.
    pub(super) fn ready(&self) -> Option<HeldOpen> {
        let mut inner = lock(&self.inner);
        inner.ready = true;
        inner.held.take()
    }

    /// An agent's request for `path`, held for the UI's startup, has opened as `opened`: shown, or
    /// failed with the open's error. Its `show` (`to`), if one still waits, hears it, and `status`
    /// keeps it.
    pub(super) fn opened(&self, path: &str, opened: &OpenResult, to: Option<Sender<Settled>>) {
        let settled = match opened {
            OpenResult::Ok { .. } => (AskAnswer::Shown, None),
            OpenResult::Err { error } => (AskAnswer::Failed, Some(error.message.clone())),
        };
        lock(&self.inner).settle(path, settled, to);
    }

    /// What became of the agents' requests held here, the oldest first, for `status`.
    pub(super) fn answers(&self) -> Vec<AskOutcome> {
        lock(&self.inner).answers.clone()
    }
}

/// Second launches that arrive before setup has made the app, which then routes them as it routes
/// any other (`App::route_held`): a file goes to the window of the workspace holding it, not to the
/// first window just because the app wasn't there yet.
#[derive(Default)]
pub struct HeldLaunches {
    inner: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    released: bool,
    requests: Vec<OpenRequest>,
}

impl HeldLaunches {
    /// Holds `request` until `release`, or, once released, hands it back to be routed now.
    pub fn hold(&self, request: OpenRequest) -> Option<OpenRequest> {
        let mut inner = lock(&self.inner);
        if inner.released {
            return Some(request);
        }
        inner.requests.push(request);
        None
    }

    /// The requests held, in the order they came; later ones aren't held.
    pub(super) fn release(&self) -> Vec<OpenRequest> {
        let mut inner = lock(&self.inner);
        inner.released = true;
        std::mem::take(&mut inner.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lectern_core::agent::request::Target;

    #[test]
    fn open_requests_wait_for_the_ui_then_go_straight_through() {
        let queue = OpenQueue::default();
        let request = |path: &str| OpenRequest {
            path: path.to_owned(),
            t0_ms: None,
            folder: false,
        };
        assert!(queue.offer(request(r"C:\a.md")).is_none());
        assert!(queue.offer(request(r"C:\b.md")).is_none());
        assert_eq!(
            queue.ready().map(|held| (held.request.path, held.by_agent)),
            Some((r"C:\b.md".to_owned(), false))
        );
        assert_eq!(
            queue.offer(request(r"C:\c.md")).map(|r| r.path),
            Some(r"C:\c.md".to_owned())
        );
        assert!(queue.ready().is_none());
    }

    /// An agent's request is held as a launch's is, marked as the agent's, with its target: the
    /// latest of either is kept, except that an agent's never takes the place of a launch's. An
    /// agent's whose place is taken is replaced: its `show` hears so, and `status` keeps it.
    #[test]
    fn an_agents_open_waits_for_the_ui_with_its_target_but_never_displaces_a_launch() {
        let request = |path: &str| OpenRequest {
            path: path.to_owned(),
            t0_ms: None,
            folder: false,
        };
        let at = |page: u32| {
            Some(ShowAt {
                target: Target::Page { page },
                place: None,
            })
        };
        let held = |queue: &OpenQueue| {
            queue
                .ready()
                .map(|h| (h.request.path, h.by_agent, h.at.map(|at| at.target)))
        };
        let offered = |queue: &OpenQueue, path: &str, page: Option<u32>| match queue
            .offer_from_agent(request(path), page.and_then(at))
        {
            AgentOffer::Held(settled) => settled,
            _ => panic!("not held: {path}"),
        };
        let replaced = |path: &str| AskOutcome {
            path: path.to_owned(),
            answer: AskAnswer::Replaced,
            message: None,
        };
        let queue = OpenQueue::default();
        let first = offered(&queue, r"C:\a.pdf", Some(2));
        let second = offered(&queue, r"C:\b.pdf", Some(3));
        assert_eq!(first.try_recv(), Ok((AskAnswer::Replaced, None)));
        assert!(second.try_recv().is_err());
        assert_eq!(
            held(&queue),
            Some((r"C:\b.pdf".to_owned(), true, Some(Target::Page { page: 3 })))
        );
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\c.md"), None),
            AgentOffer::Now
        ));
        assert_eq!(queue.answers(), [replaced(r"C:\a.pdf")]);
        // A launch takes an agent's place, target and all, and keeps it.
        let queue = OpenQueue::default();
        let agents = offered(&queue, r"C:\a.pdf", Some(2));
        assert!(queue.offer(request(r"C:\b.md")).is_none());
        assert_eq!(agents.try_recv(), Ok((AskAnswer::Replaced, None)));
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\c.md"), None),
            AgentOffer::Refused
        ));
        assert_eq!(held(&queue), Some((r"C:\b.md".to_owned(), false, None)));
        assert_eq!(queue.answers(), [replaced(r"C:\a.pdf")]);
    }

    /// What became of an agent's held request once it opened reaches its `show` and `status`,
    /// which keeps the latest few.
    #[test]
    fn an_agents_held_open_is_settled_as_it_opened() {
        let queue = OpenQueue::default();
        let (to, settled) = std::sync::mpsc::channel();
        let failed = OpenResult::Err {
            error: lectern_core::ipc::OpenError {
                kind: lectern_core::ipc::OpenErrorKind::Binary,
                message: "binary.md isn't a text file".to_owned(),
                path: r"C:\binary.md".to_owned(),
            },
        };
        queue.opened(r"C:\binary.md", &failed, Some(to));
        let why = Some("binary.md isn't a text file".to_owned());
        assert_eq!(settled.try_recv(), Ok((AskAnswer::Failed, why.clone())));
        for n in 0..ANSWERS_KEPT {
            queue.opened(&format!(r"C:\{n}.md"), &failed, None);
        }
        let answers = queue.answers();
        assert_eq!(answers.len(), ANSWERS_KEPT);
        assert_eq!(answers[0].path, r"C:\0.md");
        assert_eq!(answers[0].message, why);
    }

    #[test]
    fn launches_before_setup_are_held_in_order_then_go_straight_through() {
        let held = HeldLaunches::default();
        let request = |path: &str| OpenRequest {
            path: path.to_owned(),
            t0_ms: None,
            folder: false,
        };
        assert!(held.hold(request(r"C:\a.md")).is_none());
        assert!(held.hold(request(r"C:\b.md")).is_none());
        let released: Vec<String> = held.release().into_iter().map(|r| r.path).collect();
        assert_eq!(released, [r"C:\a.md", r"C:\b.md"]);
        assert_eq!(
            held.hold(request(r"C:\c.md")).map(|r| r.path),
            Some(r"C:\c.md".to_owned())
        );
        assert!(held.release().is_empty());
    }
}
