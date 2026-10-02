//! Releasing the compiled grammars while the window is in the background (unfocused or
//! minimised), and warming them again when it comes back.
//!
//! The grammars go once the window has been in the background, and they have gone unused, for a
//! whole `after` (60 s in the app): a quick Alt+Tab away and back never releases anything.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// What `BackgroundRelease` works on: the highlighter's grammars in the app, a fake in tests.
pub trait Grammars: Send + Sync + 'static {
    /// When a highlight last used the compiled grammars; `None` while none are loaded.
    fn last_used(&self) -> Option<Instant>;
    /// Takes them out of use. Called under `BackgroundRelease`'s lock, so it hands back what
    /// to free, which is freed once that lock is let go.
    fn release(&self) -> Option<Box<dyn Send>>;
    /// Compiles the likely ones again, out of the renders' way.
    fn rewarm(&self);
}

/// What `ReleasePolicy` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAction {
    Release,
    Rewarm,
}

/// When to release and re-warm, with the time passed in.
#[derive(Debug, Clone)]
pub struct ReleasePolicy {
    after: Duration,
    /// Since when the window has been in the background, continuously.
    background_since: Option<Instant>,
    /// The grammars were released during this spell in the background.
    released: bool,
}

impl ReleasePolicy {
    pub fn new(after: Duration) -> Self {
        Self {
            after,
            background_since: None,
            released: false,
        }
    }

    /// The window went to the background or came back. Coming back after a release asks for a
    /// re-warm.
    pub fn set_background(&mut self, background: bool, now: Instant) -> Option<ReleaseAction> {
        if background {
            self.background_since.get_or_insert(now);
            None
        } else {
            self.background_since = None;
            std::mem::take(&mut self.released).then_some(ReleaseAction::Rewarm)
        }
    }

    /// When grammars last used at `last_used` fall due: `after` past the later of that and the
    /// window going to the background. `None` in the foreground or with nothing loaded.
    pub fn due_at(&self, last_used: Option<Instant>) -> Option<Instant> {
        let since = self.background_since?;
        Some(since.max(last_used?) + self.after)
    }

    /// Asks for the release once it is due.
    pub fn poll(&mut self, now: Instant, last_used: Option<Instant>) -> Option<ReleaseAction> {
        let due = self.due_at(last_used)?;
        (now >= due).then(|| {
            self.released = true;
            ReleaseAction::Release
        })
    }

    pub fn in_background(&self) -> bool {
        self.background_since.is_some()
    }
}

/// Runs a `ReleasePolicy` against `Grammars` on a thread of its own, which stops when this is
/// dropped.
pub struct BackgroundRelease {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    grammars: Box<dyn Grammars>,
}

struct State {
    policy: ReleasePolicy,
    stop: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl BackgroundRelease {
    /// Releases `grammars` once the window has been in the background, and they unused, for
    /// `after`.
    pub fn start(after: Duration, grammars: impl Grammars) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                policy: ReleasePolicy::new(after),
                stop: false,
            }),
            wake: Condvar::new(),
            grammars: Box::new(grammars),
        });
        let worker = Arc::clone(&shared);
        if let Err(e) = thread::Builder::new()
            .name("lectern-grammar-release".to_owned())
            .spawn(move || run(&worker, after))
        {
            log::warn!("couldn't start releasing grammars in the background: {e}");
        }
        Self { shared }
    }

    /// The window went to the background (unfocused or minimised) or came back.
    pub fn set_background(&self, background: bool) {
        let action = {
            let mut state = self.shared.state();
            let was = state.policy.in_background();
            let action = state.policy.set_background(background, Instant::now());
            if state.policy.in_background() != was {
                self.shared.wake.notify_all();
            }
            action
        };
        if action == Some(ReleaseAction::Rewarm) {
            self.shared.grammars.rewarm();
        }
    }
}

impl Drop for BackgroundRelease {
    fn drop(&mut self) {
        self.shared.state().stop = true;
        self.shared.wake.notify_all();
    }
}

fn run(shared: &Shared, after: Duration) {
    let mut state = shared.state();
    loop {
        if state.stop {
            return;
        }
        let now = Instant::now();
        let last_used = shared.grammars.last_used();
        if state.policy.poll(now, last_used) == Some(ReleaseAction::Release) {
            // Decided and done under one lock, so the window can't come back in between: its
            // return either comes first and cancels the release, or comes after it and re-warms.
            let released = shared.grammars.release();
            drop(state);
            drop(released);
            log::info!("released the compiled grammars while the window is in the background");
            state = shared.state();
            continue;
        }
        // Nothing loaded in the background: a render may load the grammars again, so look again
        // after a while.
        let wait = match state.policy.due_at(last_used) {
            Some(due) => Some(due.saturating_duration_since(now)),
            None if state.policy.in_background() => Some(after),
            None => None,
        };
        state = match wait {
            Some(wait) => {
                shared
                    .wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;

    const AFTER: Duration = Duration::from_secs(60);

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    #[test]
    fn the_grammars_go_after_a_minute_in_the_background() {
        let t0 = Instant::now();
        let used = Some(t0);
        let mut policy = ReleasePolicy::new(AFTER);
        assert_eq!(
            policy.poll(at(t0, 120), used),
            None,
            "not in the foreground"
        );
        policy.set_background(true, at(t0, 10));
        assert_eq!(policy.due_at(used), Some(at(t0, 70)));
        assert_eq!(policy.poll(at(t0, 69), used), None);
        assert_eq!(policy.poll(at(t0, 70), used), Some(ReleaseAction::Release));
    }

    #[test]
    fn coming_back_within_the_minute_cancels_the_release() {
        let t0 = Instant::now();
        let used = Some(t0);
        let mut policy = ReleasePolicy::new(AFTER);
        policy.set_background(true, at(t0, 1));
        assert_eq!(policy.set_background(false, at(t0, 59)), None);
        assert_eq!(policy.poll(at(t0, 61), used), None);
        // A new spell in the background counts from its own start.
        policy.set_background(true, at(t0, 62));
        policy.set_background(true, at(t0, 100));
        assert_eq!(policy.poll(at(t0, 121), used), None);
        assert_eq!(policy.poll(at(t0, 122), used), Some(ReleaseAction::Release));
    }

    #[test]
    fn a_highlight_in_the_background_restarts_the_minute() {
        let t0 = Instant::now();
        let mut policy = ReleasePolicy::new(AFTER);
        policy.set_background(true, t0);
        assert_eq!(policy.poll(at(t0, 61), Some(at(t0, 30))), None);
        assert_eq!(
            policy.poll(at(t0, 90), Some(at(t0, 30))),
            Some(ReleaseAction::Release)
        );
    }

    #[test]
    fn nothing_loaded_means_nothing_to_release() {
        let t0 = Instant::now();
        let mut policy = ReleasePolicy::new(AFTER);
        policy.set_background(true, t0);
        assert_eq!(policy.poll(at(t0, 600), None), None);
        assert_eq!(policy.set_background(false, at(t0, 601)), None);
    }

    #[test]
    fn coming_back_after_a_release_rewarms_once() {
        let t0 = Instant::now();
        let mut policy = ReleasePolicy::new(AFTER);
        policy.set_background(true, t0);
        assert_eq!(
            policy.poll(at(t0, 60), Some(t0)),
            Some(ReleaseAction::Release)
        );
        assert_eq!(
            policy.set_background(false, at(t0, 90)),
            Some(ReleaseAction::Rewarm)
        );
        policy.set_background(true, at(t0, 91));
        assert_eq!(policy.set_background(false, at(t0, 92)), None);
    }

    /// Grammars that count releases and re-warms, loaded until released.
    #[derive(Default)]
    struct Fake {
        used: Mutex<Option<Instant>>,
        releases: AtomicUsize,
        rewarms: AtomicUsize,
    }

    impl Grammars for Arc<Fake> {
        fn last_used(&self) -> Option<Instant> {
            *self.used.lock().unwrap()
        }

        fn release(&self) -> Option<Box<dyn Send>> {
            *self.used.lock().unwrap() = None;
            self.releases.fetch_add(1, Ordering::SeqCst);
            None
        }

        fn rewarm(&self) {
            *self.used.lock().unwrap() = Some(Instant::now());
            self.rewarms.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn loaded_fake() -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        *fake.used.lock().unwrap() = Some(Instant::now());
        fake
    }

    fn eventually(what: &str, f: impl Fn() -> bool) {
        let started = Instant::now();
        while !f() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_thread_releases_in_the_background_and_rewarms_on_return() {
        let fake = loaded_fake();
        let release = BackgroundRelease::start(Duration::from_millis(50), Arc::clone(&fake));
        release.set_background(true);
        eventually("the release", || fake.releases.load(Ordering::SeqCst) == 1);
        assert_eq!(fake.rewarms.load(Ordering::SeqCst), 0);
        release.set_background(false);
        assert_eq!(fake.rewarms.load(Ordering::SeqCst), 1);
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            fake.releases.load(Ordering::SeqCst),
            1,
            "not in the foreground"
        );
    }

    #[test]
    fn the_thread_keeps_the_grammars_through_a_quick_switch_away() {
        let fake = loaded_fake();
        let release = BackgroundRelease::start(Duration::from_millis(300), Arc::clone(&fake));
        for _ in 0..3 {
            release.set_background(true);
            thread::sleep(Duration::from_millis(150));
            release.set_background(false);
        }
        thread::sleep(Duration::from_millis(500));
        assert_eq!(fake.releases.load(Ordering::SeqCst), 0);
        assert_eq!(fake.rewarms.load(Ordering::SeqCst), 0);
    }

    /// Grammars whose release, once begun (it says so on `begun`), waits until `open` is called.
    struct Gated {
        fake: Arc<Fake>,
        begun: Mutex<Option<mpsc::Sender<()>>>,
        gate: (Mutex<bool>, Condvar),
    }

    impl Gated {
        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }

    impl Grammars for Arc<Gated> {
        fn last_used(&self) -> Option<Instant> {
            self.fake.last_used()
        }

        fn release(&self) -> Option<Box<dyn Send>> {
            if let Some(begun) = self.begun.lock().unwrap().take() {
                begun.send(()).unwrap();
            }
            drop(
                self.gate
                    .1
                    .wait_while(self.gate.0.lock().unwrap(), |open| !*open)
                    .unwrap(),
            );
            self.fake.release()
        }

        fn rewarm(&self) {
            self.fake.rewarm();
        }
    }

    #[test]
    fn coming_back_while_the_grammars_are_being_released_leaves_them_warm() {
        let (begun, has_begun) = mpsc::channel();
        let gated = Arc::new(Gated {
            fake: loaded_fake(),
            begun: Mutex::new(Some(begun)),
            gate: (Mutex::new(false), Condvar::new()),
        });
        let release = Arc::new(BackgroundRelease::start(
            Duration::from_millis(20),
            Arc::clone(&gated),
        ));
        release.set_background(true);
        has_begun
            .recv_timeout(Duration::from_secs(10))
            .expect("the release began");
        // The window comes back between the decision to release and the grammars going.
        let back = {
            let release = Arc::clone(&release);
            thread::spawn(move || release.set_background(false))
        };
        thread::sleep(Duration::from_millis(100));
        gated.open();
        back.join().unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!(
            gated.fake.last_used().is_some(),
            "the grammars were left released in the foreground"
        );
        assert_eq!(gated.fake.releases.load(Ordering::SeqCst), 1);
        assert_eq!(gated.fake.rewarms.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_thread_releases_grammars_loaded_again_in_the_background() {
        let fake = loaded_fake();
        let release = BackgroundRelease::start(Duration::from_millis(50), Arc::clone(&fake));
        release.set_background(true);
        eventually("the first release", || {
            fake.releases.load(Ordering::SeqCst) == 1
        });
        // A document changed on disk re-renders while the window is away.
        *fake.used.lock().unwrap() = Some(Instant::now());
        eventually("the second release", || {
            fake.releases.load(Ordering::SeqCst) == 2
        });
        drop(release);
    }
}
