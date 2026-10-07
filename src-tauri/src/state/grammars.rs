//! The highlighter's compiled grammars while every window is in the background: released after a
//! while there, and warmed again when any window comes back, the languages of the document on
//! screen in the focused window first.

use std::collections::HashMap;
use std::sync::Weak;
use std::time::Instant;

use lectern_core::render::code_languages;
use lectern_core::render::highlight::{self, Grammars};

use super::sync::lock;
use super::{App, WindowState};

/// The process's grammars, with the documents on screen for the re-warm.
pub(super) struct AppGrammars(pub(super) Weak<App>);

impl Grammars for AppGrammars {
    fn last_used(&self) -> Option<Instant> {
        highlight::grammars_last_used()
    }

    fn release(&self) -> Option<Box<dyn Send>> {
        highlight::take_grammars().map(|released| Box::new(released) as Box<dyn Send>)
    }

    fn rewarm(&self) {
        let first = self
            .0
            .upgrade()
            .map(|app| app.current_languages())
            .unwrap_or_default();
        highlight::warm_up_in_background(&first);
    }
}

impl App {
    /// The window `label` went to the background (unfocused or minimised) or came back. The app
    /// is in the background once every window is: the grammars' release starts counting then, and
    /// stops as soon as any window comes back.
    pub fn set_background(&self, label: &str, background: bool) {
        let mut windows = lock(&self.backgrounds);
        windows.insert(label.to_owned(), background);
        self.background.set_background(all_in_background(&windows));
    }

    /// The window `label` closed, so whether the app is in the background no longer depends on it.
    pub fn window_closed(&self, label: &str) {
        let mut windows = lock(&self.backgrounds);
        if windows.remove(label).is_some() {
            self.background.set_background(all_in_background(&windows));
        }
    }

    /// The languages of the code blocks in the document on screen in the focused window, else
    /// in any window.
    fn current_languages(&self) -> Vec<String> {
        let focused = lock(&self.focus).first().cloned();
        let windows = self.windows();
        windows
            .iter()
            .find(|window| focused.as_ref() == Some(&window.label))
            .or_else(|| windows.first())
            .map(|window| window.current_languages())
            .unwrap_or_default()
    }
}

/// Whether every window is in the background; with none left, nothing is on screen.
fn all_in_background(windows: &HashMap<String, bool>) -> bool {
    windows.values().all(|&background| background)
}

impl WindowState {
    /// The languages of the code blocks in the document on screen, in order of first use.
    pub(super) fn current_languages(&self) -> Vec<String> {
        lock(&self.current)
            .as_ref()
            .and_then(|current| current.doc.as_ref())
            .map(|doc| code_languages(&doc.html))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use lectern_core::library::pathmap::PathMapper;
    use lectern_core::render::highlight::StartupWarmUp;

    use crate::state::doc::render_file;
    use crate::state::open_queue::OpenQueue;
    use crate::state::paths::path_string;
    use crate::state::sync::lock;
    use crate::state::test_support::*;

    use super::all_in_background;

    #[test]
    fn the_languages_to_warm_first_are_the_open_documents() {
        let f = fixture(profile(&[]), FakeHost::default());
        assert!(f.state.current_languages().is_empty());
        let plan = f.dir.file(
            "notes/plan.md",
            "# Plan\n\n```ruby\na\n```\n\n```jsx\n<b />\n```\n\n```ruby\nc\n```\n",
        );
        f.state.open_document(&path_string(&plan));
        assert_eq!(f.state.current_languages(), ["ruby", "jsx"]);
        let prose = f.dir.file("notes/prose.md", "# Prose\n\nNo code here.\n");
        f.state.open_document(&path_string(&prose));
        assert!(f.state.current_languages().is_empty());
    }

    /// Coming back from the background, the grammars of the focused window's document are warmed
    /// first; before any window has had the focus, any window's.
    #[test]
    fn the_rewarm_starts_with_the_focused_windows_document() {
        let f = fixture(profile(&[]), FakeHost::default());
        let ruby = f.dir.file("one/tide.md", "```ruby\na\n```\n");
        let jsx = f.dir.file("two/chart.md", "```jsx\n<b />\n```\n");
        let mapper = PathMapper::default();
        let rendered = render_file(&ruby, &mapper, &[]);
        f.state
            .make_current(f.state.next_seq(), &ruby, rendered.as_ref().ok());
        assert_eq!(f.app.current_languages(), ["ruby"]);
        f.app
            .add_window("second", None, Arc::new(OpenQueue::default()), None);
        let second = f.app.window("second").unwrap();
        let rendered = render_file(&jsx, &mapper, &[]);
        second.make_current(second.next_seq(), &jsx, rendered.as_ref().ok());
        f.app.window_focused("second");
        assert_eq!(f.app.current_languages(), ["jsx"]);
        f.app.window_focused("main");
        assert_eq!(f.app.current_languages(), ["ruby"]);
    }

    /// The app is in the background, and the grammars' release counting, only while every window
    /// is unfocused or minimised. A window that closes counts no more.
    #[test]
    fn the_app_is_in_the_background_while_every_window_is() {
        let f = fixture(profile(&[]), FakeHost::default());
        let backgrounded = || all_in_background(&lock(&f.app.backgrounds));
        // main is focused and win-1 minimised.
        f.app.set_background("main", false);
        f.app.set_background("win-1", true);
        assert!(!backgrounded());
        // main loses the focus: the release is scheduled.
        f.app.set_background("main", true);
        assert!(backgrounded());
        // win-1 is restored and focused: the release is cancelled.
        f.app.set_background("win-1", false);
        assert!(!backgrounded());
        // win-1 closes, leaving main in the background.
        f.app.window_closed("win-1");
        assert!(backgrounded());
        assert!(!lock(&f.app.backgrounds).contains_key("win-1"));
        f.app.set_background("main", false);
        assert!(!backgrounded());
    }

    #[test]
    fn the_first_paint_starts_the_warm_up_when_boot_had_no_document() {
        let started = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&started);
        let warm = Arc::new(StartupWarmUp::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        let f = fixture_with_warm(profile(&[]), FakeHost::default(), Arc::clone(&warm));
        warm.boot_finished(false);
        assert_eq!(started.load(Ordering::SeqCst), 0);
        f.app.perf_mark("doc-switch", Some(1.0));
        assert_eq!(started.load(Ordering::SeqCst), 0);
        f.app.perf_mark("first-paint", None);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }
}
