//! The highlighter's compiled grammars while the window is in the background: released after a
//! while there, and warmed again on return, the languages of the document on screen in the
//! focused window first.

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
    /// The window went to the background (unfocused or minimised) or came back.
    pub fn set_background(&self, background: bool) {
        self.background.set_background(background);
    }

    /// The window `label` was focused: its document's languages are the first to warm again.
    pub fn window_focused(&self, label: &str) {
        *lock(&self.focused) = Some(label.to_owned());
    }

    /// The languages of the code blocks in the document on screen in the focused window, else
    /// in any window.
    fn current_languages(&self) -> Vec<String> {
        let focused = lock(&self.focused).clone();
        let windows = self.windows();
        windows
            .iter()
            .find(|window| focused.as_ref() == Some(&window.label))
            .or_else(|| windows.first())
            .map(|window| window.current_languages())
            .unwrap_or_default()
    }
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
    use crate::state::test_support::*;

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
