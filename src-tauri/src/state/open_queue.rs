//! Second launches that arrive before the app can route them, or before a window's UI can receive
//! events.

use std::sync::Mutex;

use lectern_core::agent::target::ShowAt;
use lectern_core::ipc::OpenRequest;

use super::sync::lock;

/// Files that second launches, or agents, ask to open. Until the UI has asked for its startup
/// payload it can't receive events, so the latest request is held for `startup` instead; an
/// agent's never takes the place of a launch's.
#[derive(Default)]
pub struct OpenQueue {
    inner: Mutex<(bool, Option<HeldOpen>)>,
}

/// A request held for the UI's startup, and whether an agent made it: an agent's file opens as it
/// is, trusting nothing, where a launch's makes its host trusted. An agent's may have a target,
/// which the UI goes to once the file has opened.
pub(super) struct HeldOpen {
    pub(super) request: OpenRequest,
    pub(super) by_agent: bool,
    pub(super) at: Option<ShowAt>,
}

/// What became of an agent's request offered to a window's queue (`offer_from_agent`).
pub(super) enum AgentOffer {
    /// The UI is up: the request goes to it now, not held.
    Now,
    /// Held for the UI's startup.
    Held,
    /// The UI's startup holds a file the user launched, which an agent's never takes the place of.
    Refused,
}

impl OpenQueue {
    /// Holds `request`, a launch's, while the UI isn't ready, or hands it back to be sent now.
    pub fn offer(&self, request: OpenRequest) -> Option<OpenRequest> {
        let mut inner = lock(&self.inner);
        if inner.0 {
            return Some(request);
        }
        inner.1 = Some(HeldOpen {
            request,
            by_agent: false,
            at: None,
        });
        None
    }

    /// As `offer`, for an agent's request, going to `at` once open, which is refused while a
    /// launch's is held.
    pub(super) fn offer_from_agent(&self, request: OpenRequest, at: Option<ShowAt>) -> AgentOffer {
        let mut inner = lock(&self.inner);
        if inner.0 {
            return AgentOffer::Now;
        }
        if inner.1.as_ref().is_some_and(|held| !held.by_agent) {
            return AgentOffer::Refused;
        }
        inner.1 = Some(HeldOpen {
            request,
            by_agent: true,
            at,
        });
        AgentOffer::Held
    }

    /// Whether a request is held for the UI.
    pub(super) fn has_pending(&self) -> bool {
        lock(&self.inner).1.is_some()
    }

    /// Marks the UI ready and returns the request held for it, if any.
    pub(super) fn ready(&self) -> Option<HeldOpen> {
        let mut inner = lock(&self.inner);
        inner.0 = true;
        inner.1.take()
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
    /// latest of either is kept, except that an agent's never takes the place of a launch's.
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
        let queue = OpenQueue::default();
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\a.pdf"), at(2)),
            AgentOffer::Held
        ));
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\b.pdf"), at(3)),
            AgentOffer::Held
        ));
        assert_eq!(
            held(&queue),
            Some((r"C:\b.pdf".to_owned(), true, Some(Target::Page { page: 3 })))
        );
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\c.md"), None),
            AgentOffer::Now
        ));
        // A launch takes an agent's place, target and all, and keeps it.
        let queue = OpenQueue::default();
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\a.pdf"), at(2)),
            AgentOffer::Held
        ));
        assert!(queue.offer(request(r"C:\b.md")).is_none());
        assert!(matches!(
            queue.offer_from_agent(request(r"C:\c.md"), None),
            AgentOffer::Refused
        ));
        assert_eq!(held(&queue), Some((r"C:\b.md".to_owned(), false, None)));
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
