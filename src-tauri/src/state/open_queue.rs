//! Second launches that arrive before the app can route them, or before a window's UI can receive
//! events.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Instant;

use lectern_core::agent::status::{AgentCall, AskAnswer, AskOutcome, ANSWERS_KEPT};
use lectern_core::agent::target::ShowAt;
use lectern_core::ipc::{OpenRequest, OpenResult};

use super::sync::lock;

/// What became of an agent's request to show a file: the reader's answer, or the window's (shown,
/// failed or replaced), with why when it `failed`.
pub(super) type Settled = (AskAnswer, Option<String>);

/// Files that second launches, or agents, ask to open. Until the UI has asked for its startup
/// payload it can't receive events, so the latest request is held for `startup` instead; an
/// agent's never takes the place of the user's launch. What becomes of each agent's request held
/// here (shown, failed, or replaced by a newer one) reaches its `show` and is kept for `status`.
/// While the user's launch waits for the UI (a second launch held here, or the file the command
/// line named: `launched`), an agent's request isn't held to open in its place: it goes to the UI
/// as to one that's up, held until the UI is (`to_ui`), which shows the user's file first. The
/// newest agent's show held so wins, as at the ask bar: an older one is replaced.
#[derive(Default)]
pub struct OpenQueue {
    inner: Mutex<Queue>,
}

#[derive(Default)]
struct Queue {
    /// The UI has asked for its startup payload: requests go to it now.
    ready: bool,
    /// The window starts with what the command line named, which its startup opens.
    launched: bool,
    held: Option<HeldOpen>,
    /// An agent's show for the UI, held until the UI is up: the newest (`to_ui`).
    show: Option<HeldShow>,
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

/// An agent's show held for the UI: its file, and when its `show` stops waiting for the UI's
/// answer, which tells how long the UI has once it gets it.
struct HeldShow {
    path: String,
    call: AgentCall,
    by: Instant,
}

/// What became of an agent's request offered to a window's queue (`offer_from_agent`).
pub(super) enum AgentOffer {
    /// Held for the UI's startup: what becomes of it arrives here.
    Held(Receiver<Settled>),
    /// Not held: it goes to the UI as to one that's up (`to_ui`). The UI is up, or the window
    /// starts with the user's launch, which an agent's never takes the place of.
    ToUi,
}

/// What became of an agent's show offered to the UI (`OpenQueue::to_ui`).
pub(super) enum ForUi {
    /// The UI is up: it goes to it now.
    Now(AgentCall),
    /// Held until the UI is up, in the place of the show held so before it, if any: that one's id,
    /// which is replaced.
    Held { replaced: Option<u64> },
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

    /// As `offer`, for an agent's request, going to `at` once open. Held, it takes the place of an
    /// agent's held before it, which is replaced, and what becomes of it arrives on the receiver
    /// `AgentOffer::Held` gives. It isn't held while the UI is up, nor while the user's launch
    /// waits for it (`launched`, or a launch's held): it goes to the UI instead.
    pub(super) fn offer_from_agent(&self, request: OpenRequest, at: Option<ShowAt>) -> AgentOffer {
        let mut inner = lock(&self.inner);
        if inner.ready || inner.launched || inner.held.as_ref().is_some_and(|held| !held.by_agent) {
            return AgentOffer::ToUi;
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

    /// Marks the window as starting with what the command line named, the user's launch, which
    /// its startup opens: an agent's request meanwhile isn't held to open in its place
    /// (`offer_from_agent`). Unmarked, the document boot restores is no request of the user's, and
    /// an agent's takes its place.
    pub fn launched(&self) {
        lock(&self.inner).launched = true;
    }

    /// An agent's show `call` of `path` for the UI, whose `show` waits for the UI's answer until
    /// `by`: handed back to send now when the UI is up, else held until the UI asks for its startup
    /// payload (`ready`), and answered by the UI once it has shown its first document. The newest
    /// wins, as at the ask bar: a show held before it is replaced, which `status` keeps, and its
    /// id comes back for its `show` to hear so.
    pub(super) fn to_ui(&self, path: &str, call: AgentCall, by: Instant) -> ForUi {
        let mut inner = lock(&self.inner);
        if inner.ready {
            return ForUi::Now(call);
        }
        let held = HeldShow {
            path: path.to_owned(),
            call,
            by,
        };
        ForUi::Held {
            replaced: inner.show.replace(held).map(|old| {
                inner.settle(&old.path, (AskAnswer::Replaced, None), None);
                old.call.id
            }),
        }
    }

    /// What became of an agent's show of `path` after its `show` stopped waiting (it answered
    /// `waiting`): `status` keeps it.
    pub(super) fn settled_late(&self, path: &str, settled: Settled) {
        lock(&self.inner).settle(path, settled, None);
    }

    /// How many agents' shows are held for the UI.
    #[cfg(test)]
    pub(super) fn held_for_ui(&self) -> usize {
        usize::from(lock(&self.inner).show.is_some())
    }

    /// Whether an agent's request waits here: held for the UI's startup, or for the UI.
    #[cfg(test)]
    pub(super) fn agent_waits(&self) -> bool {
        let inner = lock(&self.inner);
        inner.show.is_some() || inner.held.as_ref().is_some_and(|held| held.by_agent)
    }

    /// Whether a request is held for the UI.
    pub(super) fn has_pending(&self) -> bool {
        lock(&self.inner).held.is_some()
    }

    /// Marks the UI ready and returns the request held for it, if any. The agent's show held for
    /// the UI goes to `send` first, so none sent after overtakes it, with as long to answer as its
    /// `show` still waits: none, once that has stopped waiting, though the UI still acts on it.
    pub(super) fn ready(&self, send: impl FnOnce(AgentCall)) -> Option<HeldOpen> {
        let mut inner = lock(&self.inner);
        inner.ready = true;
        if let Some(HeldShow { mut call, by, .. }) = inner.show.take() {
            let left = by.saturating_duration_since(Instant::now());
            call.within_ms = Some(u32::try_from(left.as_millis()).unwrap_or(u32::MAX));
            send(call);
        }
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
    use lectern_core::agent::request::{ShowArgs, Target};
    use lectern_core::agent::AgentRequest;
    use std::time::Duration;

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
            queue
                .ready(drop)
                .map(|held| (held.request.path, held.by_agent)),
            Some((r"C:\b.md".to_owned(), false))
        );
        assert_eq!(
            queue.offer(request(r"C:\c.md")).map(|r| r.path),
            Some(r"C:\c.md".to_owned())
        );
        assert!(queue.ready(drop).is_none());
    }

    /// An agent's request is held as a launch's is, marked as the agent's, with its target: the
    /// latest of either is kept, except that an agent's never takes the place of a launch's, and
    /// goes to the UI instead. An agent's whose place is taken is replaced: its `show` hears so,
    /// and `status` keeps it.
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
                .ready(drop)
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
            AgentOffer::ToUi
        ));
        assert_eq!(queue.answers(), [replaced(r"C:\a.pdf")]);
        // A launch takes an agent's place, target and all, and keeps it.
        let queue = OpenQueue::default();
        let agents = offered(&queue, r"C:\a.pdf", Some(2));
        assert!(queue.offer(request(r"C:\b.md")).is_none());
        assert_eq!(agents.try_recv(), Ok((AskAnswer::Replaced, None)));
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\c.md"), None),
            AgentOffer::ToUi
        ));
        assert_eq!(held(&queue), Some((r"C:\b.md".to_owned(), false, None)));
        assert_eq!(queue.answers(), [replaced(r"C:\a.pdf")]);
    }

    /// A window starting with what the command line named: an agent's request isn't held to open
    /// in its place, and an agent's show for the UI waits for it, the newest alone, as at the ask
    /// bar: an older one is replaced, which `status` keeps. The UI up, shows go straight through.
    #[test]
    fn while_the_command_lines_file_waits_for_the_ui_the_newest_agents_show_waits_after_it() {
        let by = Instant::now() + Duration::from_secs(60);
        let queue = OpenQueue::default();
        queue.launched();
        let request = OpenRequest {
            path: r"C:\a.md".to_owned(),
            t0_ms: None,
            folder: false,
        };
        assert!(matches!(
            queue.offer_from_agent(request, None),
            AgentOffer::ToUi
        ));
        assert!(!queue.has_pending());
        let held = |id: u64, path: &str| match queue.to_ui(path, show_call(id, path), by) {
            ForUi::Held { replaced } => replaced,
            ForUi::Now(_) => panic!("sent at once: {id}"),
        };
        assert_eq!(held(1, r"C:\a.md"), None);
        assert_eq!(held(2, r"C:\b.md"), Some(1));
        assert_eq!(held(3, r"C:\c.md"), Some(2));
        assert_eq!(queue.held_for_ui(), 1);
        let replaced = |path: &str| AskOutcome {
            path: path.to_owned(),
            answer: AskAnswer::Replaced,
            message: None,
        };
        assert_eq!(
            queue.answers(),
            [replaced(r"C:\a.md"), replaced(r"C:\b.md")]
        );
        let mut sent = Vec::new();
        assert!(queue.ready(|call| sent.push(call.id)).is_none());
        assert_eq!(sent, [3]);
        assert_eq!(queue.held_for_ui(), 0);
        // Up now: straight through.
        assert!(matches!(
            queue.to_ui(r"C:\d.md", show_call(4, r"C:\d.md"), by),
            ForUi::Now(call) if call.id == 4
        ));
        assert!(queue.ready(|_| panic!("sent twice")).is_none());
    }

    /// An agent's show, held for the UI with the time its `show` gave it, goes to the UI with as
    /// long as that `show` still waits: none, once it has stopped waiting, though it still goes.
    #[test]
    fn a_held_show_goes_to_the_ui_with_the_time_its_show_still_waits() {
        let sent = |by: Instant| {
            let queue = OpenQueue::default();
            queue.launched();
            let call = AgentCall {
                within_ms: Some(5000),
                ..show_call(1, r"C:\a.md")
            };
            assert!(matches!(
                queue.to_ui(r"C:\a.md", call, by),
                ForUi::Held { replaced: None }
            ));
            let mut sent = None;
            assert!(queue.ready(|call| sent = Some(call)).is_none());
            sent.expect("sent").within_ms.expect("given time")
        };
        // Its show waits 2 s more: 2 s at most, not the 5 s it was held with.
        let within = sent(Instant::now() + Duration::from_secs(2));
        assert!(within <= 2000, "{within}");
        // Its show stopped waiting: no time.
        assert_eq!(sent(Instant::now()), 0);
    }

    /// An agent's show `id` of `path`, as `show_in` makes it.
    fn show_call(id: u64, path: &str) -> AgentCall {
        AgentCall {
            id,
            request: AgentRequest::Show(ShowArgs {
                path: path.to_owned(),
                workspace: None,
                target: None,
                new_window: false,
                wait: 0,
            }),
            agent: "Claude".to_owned(),
            places: Vec::new(),
            within_ms: None,
        }
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
