//! Second launches that arrive before the app can route them, or before a window's UI can receive
//! events.

use std::sync::Mutex;

use lectern_core::ipc::OpenRequest;

use super::sync::lock;

/// Files that second launches ask to open. Until the UI has asked for its startup payload it
/// can't receive events, so the latest request is held for `startup` instead.
#[derive(Default)]
pub struct OpenQueue {
    inner: Mutex<(bool, Option<OpenRequest>)>,
}

impl OpenQueue {
    /// Holds `request` while the UI isn't ready, or hands it back to be sent now.
    pub fn offer(&self, request: OpenRequest) -> Option<OpenRequest> {
        let mut inner = lock(&self.inner);
        if inner.0 {
            return Some(request);
        }
        inner.1 = Some(request);
        None
    }

    /// Whether a request is held for the UI.
    pub(super) fn has_pending(&self) -> bool {
        lock(&self.inner).1.is_some()
    }

    /// Marks the UI ready and returns the request held for it, if any.
    pub(super) fn ready(&self) -> Option<OpenRequest> {
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
        assert_eq!(queue.ready().map(|r| r.path), Some(r"C:\b.md".to_owned()));
        assert_eq!(
            queue.offer(request(r"C:\c.md")).map(|r| r.path),
            Some(r"C:\c.md".to_owned())
        );
        assert!(queue.ready().is_none());
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
