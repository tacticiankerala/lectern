//! Windows and the workspaces they show: opening one (blank, or showing a workspace, here or in a
//! new window, with the workspace's last note), closing one, quitting with every window open,
//! reopening the open workspaces' windows at launch, routing second launches, and the workspace
//! list itself (create, rename, delete, theme).
//!
//! A workspace is shown in at most one window, and a window shows at most one workspace (none for
//! a blank window): each `WindowState` in `App.windows` names its workspace. A workspace's `open`
//! flag is what the next launch reopens, so it stays set as Lectern exits, and is cleared only
//! when its window closes with others left or turns to another workspace.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;

use lectern_core::ipc::{
    OpenRequest, OpenWhere, SettingsSnapshot, WorkspaceOutcome, WorkspaceSummary,
};
use lectern_core::library::is_markdown;
use lectern_core::library::scan::probe_with;
use lectern_core::workspace::{
    route_open, OpenWindow, Route, WindowPlacement, WorkspaceTheme, Workspaces,
};

use super::doc::{render_file, Early, EarlyDoc};
use super::open_queue::{HeldLaunches, OpenQueue};
use super::paths::{normalize_root, path_string};
use super::sync::{lock, read, write, Slot};
use super::{mirrored, App, WindowState};
use crate::events::{Target, UiEvent};

/// How far right and down a new window sits from the focused one, in physical pixels.
const CASCADE: i32 = 32;
/// The answer to a window that has no state of its own.
const NOT_READY: &str = "window not ready";
const UNKNOWN_WORKSPACE: &str = "That workspace no longer exists.";

/// How `App::quit_from` names a window that shows no workspace.
const BLANK_WINDOW: &str = "a blank window";

/// What opening a workspace leaves to do once the windows are settled.
enum Step {
    /// Another window shows it: bring that one forward.
    Focus(String),
    /// The window turned to it: its UI reloads.
    Reload,
    /// A new window shows it: build it.
    Build(String),
}

impl App {
    /// Every workspace, in creation order, as the window `label` lists them.
    pub fn list_workspaces(&self, label: &str) -> Vec<WorkspaceSummary> {
        let shown = self.shown();
        read(&self.workspaces)
            .items
            .iter()
            .map(|ws| WorkspaceSummary {
                id: ws.id.clone(),
                name: ws.name.clone(),
                open: shown.contains_key(&ws.id),
                current: shown.get(&ws.id).is_some_and(|shown_in| shown_in == label),
                roots: ws.roots.clone(),
                own_theme: ws.theme.is_some(),
            })
            .collect()
    }

    /// The name a new workspace is offered: "Workspace N", the first one free.
    pub fn suggest_workspace_name(&self) -> String {
        read(&self.workspaces).suggest_name()
    }

    /// Opens a blank window, focused, a little right of and below the focused one.
    pub fn new_window(self: &Arc<Self>) -> Result<(), String> {
        let label = {
            let _life = lock(&self.lifecycle);
            self.bind_window(None, true)
        };
        self.build(&label, true)
    }

    /// Opens the workspace `id` from the window `label`: `Here` turns this window to it (a
    /// blank window just takes it; otherwise the workspace it showed is saved closed), and the
    /// UI reloads; `NewWindow` opens it in a new window. A workspace another window shows is
    /// never opened twice: that window comes forward instead.
    pub fn open_workspace(
        self: &Arc<Self>,
        label: &str,
        id: &str,
        place: OpenWhere,
    ) -> Result<WorkspaceOutcome, String> {
        let step = {
            let _life = lock(&self.lifecycle);
            if read(&self.workspaces).get(id).is_none() {
                return Err(UNKNOWN_WORKSPACE.to_owned());
            }
            match (self.shown().remove(id), place) {
                (Some(shown_in), _) => Step::Focus(shown_in),
                (None, OpenWhere::Here) => {
                    self.rebind(label, id)?;
                    Step::Reload
                }
                (None, OpenWhere::NewWindow) => {
                    Step::Build(self.bind_window(Some(id.to_owned()), true))
                }
            }
        };
        let outcome = match step {
            Step::Focus(shown_in) => {
                self.bring_forward(&shown_in);
                return Ok(WorkspaceOutcome::Focused);
            }
            Step::Reload => WorkspaceOutcome::Reload,
            Step::Build(new) => {
                self.build(&new, true)?;
                WorkspaceOutcome::Opened
            }
        };
        self.workspaces_changed();
        Ok(outcome)
    }

    /// Adds a workspace named `name` (trimmed; a blank name takes the suggested one), with the
    /// folder `root` as its library when given, and opens it as `open_workspace` does. The folder
    /// is the workspace's before the window shows it, so it survives the reload.
    pub fn create_workspace(
        self: &Arc<Self>,
        label: &str,
        name: &str,
        place: OpenWhere,
        root: Option<&str>,
    ) -> Result<WorkspaceOutcome, String> {
        let root = root.map(normalize_root).transpose()?;
        let id = self.change_workspaces(|workspaces| {
            let id = workspaces.create(name);
            if let (Some(root), Some(ws)) = (&root, workspaces.get_mut(&id)) {
                ws.roots.push(path_string(root));
            }
            Ok(id)
        })?;
        if root.is_some() && self.reconfigure_trust() {
            self.forget_renders();
        }
        let opened = self.open_workspace(label, &id, place);
        if opened.is_err() {
            self.workspaces_changed();
        }
        opened
    }

    /// Renames the workspace `id`; returns the workspaces as the window `label` lists them.
    pub fn rename_workspace(
        &self,
        label: &str,
        id: &str,
        name: &str,
    ) -> Result<Vec<WorkspaceSummary>, String> {
        self.change_workspaces(|workspaces| workspaces.rename(id, name))?;
        self.workspaces_changed();
        Ok(self.list_workspaces(label))
    }

    /// Forgets the workspace `id`, unless it is the last one or open in a window. Its folders stay
    /// trusted only if another workspace has them. Returns the workspaces as the window `label`
    /// lists them.
    pub fn delete_workspace(&self, label: &str, id: &str) -> Result<Vec<WorkspaceSummary>, String> {
        {
            let _life = lock(&self.lifecycle);
            self.change_workspaces(|workspaces| workspaces.delete(id))?;
        }
        if self.reconfigure_trust() {
            self.forget_renders();
        }
        self.workspaces_changed();
        Ok(self.list_workspaces(label))
    }

    /// Gives the window `label`'s workspace a theme of its own, starting as the shared one, or
    /// (`own` false) has it follow the shared theme again. Either way the window is sent its
    /// settings, with their revision, which are also returned. A blank window has no workspace to
    /// change.
    pub fn set_workspace_theme(&self, label: &str, own: bool) -> Result<SettingsSnapshot, String> {
        let window = self.window(label).ok_or(NOT_READY)?;
        if let Some(id) = window.workspace_id() {
            let shared = {
                let settings = read(&self.settings);
                WorkspaceTheme {
                    mode: settings.theme_mode.clone(),
                    light: settings.light_theme.clone(),
                    dark: settings.dark_theme.clone(),
                }
            };
            self.change_workspace(&id, |ws| {
                if ws.theme.is_some() == own {
                    return false;
                }
                ws.theme = own.then_some(shared);
                // The window's settings changed: under the workspaces lock, as every bump is.
                self.settings_rev.fetch_add(1, Ordering::SeqCst);
                true
            });
        }
        let snapshot = window.snapshot();
        window.emit(UiEvent::SettingsChanged(snapshot.clone()));
        Ok(snapshot)
    }

    /// The window `label` was focused: it is the most recently focused window, and its workspace
    /// the most recently focused workspace, saved a moment later with whatever else changed.
    pub fn window_focused(&self, label: &str) {
        let Some(window) = self.window(label) else {
            return;
        };
        {
            let mut focus = lock(&self.focus);
            focus.retain(|l| l != label);
            focus.insert(0, label.to_owned());
        }
        let Some(id) = window.workspace_id() else {
            return;
        };
        let mut workspaces = write(&self.workspaces);
        if workspaces.focus.first() != Some(&id) {
            workspaces.touch_focus(&id);
            self.saver.workspaces(&workspaces);
        }
    }

    /// The window `label` is closing, its placement already in its workspace. With other windows
    /// left, its workspace is saved closed (and forgotten when it has no libraries and isn't the
    /// only one), and its state goes. The last window's workspace stays open for the next launch,
    /// as Lectern exits, and so does every window's while Lectern quits. A blank window saves
    /// nothing.
    pub fn window_closing(&self, label: &str) {
        {
            let _life = lock(&self.lifecycle);
            let others = read(&self.windows).keys().any(|l| l != label);
            if !others || self.quitting.load(Ordering::SeqCst) {
                return;
            }
            let Some(state) = self.let_go(label) else {
                return;
            };
            let Some(id) = state.workspace_id() else {
                return;
            };
            let _ = self.change_workspaces(|workspaces| {
                if let Some(ws) = workspaces.get_mut(&id) {
                    ws.open = false;
                    if ws.roots.is_empty() {
                        // Refused for the only workspace, which stays.
                        let _ = workspaces.delete(&id);
                    }
                }
                Ok(())
            });
        }
        self.workspaces_changed();
    }

    /// Quit Lectern, asked for in the window `label`: as `quit`, once `before` has run (each
    /// window's placement saved). Unless `force`, it doesn't while another window's UI holds
    /// comment text that isn't saved yet, and names those windows instead, for the UI to ask: by
    /// their workspace, or "a blank window", the most recently focused first. Empty when it quits.
    pub fn quit_from(&self, label: &str, force: bool, before: impl FnOnce()) -> Vec<String> {
        if !force {
            let unsaved = self.unsaved_elsewhere(label);
            if !unsaved.is_empty() {
                return unsaved;
            }
        }
        before();
        self.quit();
        Vec::new()
    }

    /// The windows other than `label` whose UI holds comment text that isn't saved yet, named as
    /// `quit_from` names them. Asked before Lectern quits, or restarts for an update.
    pub fn unsaved_elsewhere(&self, label: &str) -> Vec<String> {
        let focus = lock(&self.focus).clone();
        let mut unsaved: Vec<Arc<WindowState>> = self
            .windows()
            .into_iter()
            .filter(|w| w.label() != label && w.unsaved.load(Ordering::SeqCst))
            .collect();
        unsaved.sort_by_key(|w| {
            focus
                .iter()
                .position(|l| l == w.label())
                .unwrap_or(usize::MAX)
        });
        unsaved
            .iter()
            .map(|w| {
                w.workspace_id()
                    .and_then(|id| self.read_workspace(&id, |ws| ws.name.clone()))
                    .unwrap_or_else(|| BLANK_WINDOW.to_owned())
            })
            .collect()
    }

    /// Lectern quits with every window open, each placement already in its workspace: every
    /// open workspace stays open for the next launch, which reopens them all, and the app exits,
    /// saving what is waiting as it does.
    pub fn quit(&self) {
        {
            let _life = lock(&self.lifecycle);
            self.quitting.store(true, Ordering::SeqCst);
        }
        self.host.exit();
    }

    /// The window `label` is gone: it no longer counts for the background, and its state, if it
    /// still has one (the last window, as Lectern exits), goes. Its workspace stays as it was.
    pub fn window_destroyed(&self, label: &str) {
        self.window_closed(label);
        let _life = lock(&self.lifecycle);
        self.let_go(label);
    }

    /// Once, when the first window has shown itself: a window for each other open workspace that
    /// no window shows yet, opened in the background, the least recently focused first, so the
    /// first window stays in front.
    pub(super) fn restore_windows(self: &Arc<Self>) {
        if self.exit_after_paint || self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        let labels: Vec<String> = {
            let _life = lock(&self.lifecycle);
            let shown = self.shown();
            let open: Vec<String> = {
                let workspaces = read(&self.workspaces);
                workspaces
                    .focus
                    .iter()
                    .filter(|id| !shown.contains_key(*id))
                    .filter(|id| workspaces.get(id).is_some_and(|ws| ws.open))
                    .cloned()
                    .collect()
            };
            open.into_iter()
                .map(|id| self.bind_window(Some(id), false))
                .collect()
        };
        let mut opened = false;
        for label in labels.iter().rev() {
            opened |= self.build(label, false).is_ok();
        }
        if opened {
            self.workspaces_changed();
        }
    }

    /// Hands a second launch to the thread that routes them, in order. `None`: Lectern was
    /// started again without a file, which brings the most recently focused window forward.
    pub fn second_launch(&self, request: Option<OpenRequest>) {
        let _ = self.launches.send(request);
    }

    /// Routes the second launches `held` while setup made the app, in the order they came, as
    /// any other (`second_launch`); later ones aren't held.
    pub fn route_held(&self, held: &HeldLaunches) {
        for request in held.release() {
            self.second_launch(Some(request));
        }
    }

    /// Routes a second launch (`route_open`): to the window whose libraries hold the file, else
    /// to the closed workspace that does, reopened in a new window, else to the most recently
    /// focused window. The request waits in that window's queue until its UI asks for its
    /// startup payload, and the window comes forward. Touches the file system.
    pub(super) fn launched(self: &Arc<Self>, request: Option<OpenRequest>) {
        let Some(request) = request else {
            let latest = lock(&self.focus).first().cloned();
            if let Some(label) = latest {
                self.bring_forward(&label);
            }
            return;
        };
        let path = PathBuf::from(&request.path);
        let is_dir = !is_markdown(&request.path) && is_folder(&path, self.timings.probe);
        let (label, reopened) = {
            let _life = lock(&self.lifecycle);
            let (label, reopened) = match self.route(&path, is_dir) {
                Route::Window(label) if label.is_empty() => {
                    log::info!("no window is left to open {}", path.display());
                    return;
                }
                Route::Window(label) => (label, false),
                Route::Reopen(id) => (self.bind_window(Some(id), true), true),
            };
            let Some(window) = self.window(&label) else {
                return;
            };
            if let Some(request) = window.opens.offer(request) {
                window.forward(request);
            }
            (label, reopened)
        };
        if reopened {
            if self.build(&label, true).is_err() {
                return;
            }
            self.workspaces_changed();
        }
        self.bring_forward(&label);
    }

    /// Brings the window `label` forward, unminimised. A window restored at launch that hasn't
    /// shown yet then shows in front, as asked, instead of behind the focused window.
    fn bring_forward(&self, label: &str) {
        if let Some(window) = self.window(label) {
            window.quiet.store(false, Ordering::SeqCst);
        }
        self.host.focus_window(label);
    }

    /// Where an open of `path` goes, over the windows and the workspaces none of them shows.
    fn route(&self, path: &Path, is_dir: bool) -> Route {
        let windows: Vec<OpenWindow> = self
            .windows()
            .iter()
            .map(|window| OpenWindow {
                label: window.label.clone(),
                workspace: window.workspace_id(),
                roots: window.roots(),
            })
            .collect();
        let shown: HashSet<&str> = windows
            .iter()
            .filter_map(|w| w.workspace.as_deref())
            .collect();
        let closed: Vec<(String, Vec<String>)> = {
            let workspaces = read(&self.workspaces);
            workspaces
                .focus
                .iter()
                .filter(|id| !shown.contains(id.as_str()))
                .filter_map(|id| workspaces.get(id))
                .map(|ws| (ws.id.clone(), ws.roots.clone()))
                .collect()
        };
        let focus = lock(&self.focus).clone();
        route_open(path, is_dir, &windows, &closed, &focus)
    }

    /// Which window shows each workspace: workspace id to window label.
    fn shown(&self) -> HashMap<String, String> {
        self.windows()
            .iter()
            .filter_map(|window| window.workspace_id().map(|id| (id, window.label.clone())))
            .collect()
    }

    /// Turns the window `label` to the workspace `id` through a new state, with a fresh open
    /// queue and ready gate and that workspace's last document rendering for it: the workspace it
    /// showed is saved closed and its old state retired. The window stays where it is.
    fn rebind(self: &Arc<Self>, label: &str, id: &str) -> Result<(), String> {
        let old = self.window(label).ok_or(NOT_READY)?;
        let previous = old.workspace_id();
        let focused = lock(&self.focus).first().is_some_and(|l| l == label);
        self.change_workspaces(|workspaces| {
            if let Some(ws) = previous.as_deref().and_then(|p| workspaces.get_mut(p)) {
                ws.open = false;
            }
            if let Some(ws) = workspaces.get_mut(id) {
                ws.open = true;
            }
            if focused {
                workspaces.touch_focus(id);
            }
            Ok(())
        })?;
        let state = WindowState::new(
            self,
            label,
            Some(id.to_owned()),
            Arc::new(OpenQueue::default()),
            Some(self.render_last_doc(id)),
        );
        if let Some(placement) = old.saved_placement() {
            state.set_initial_placement(placement);
        }
        write(&self.windows).insert(label.to_owned(), Arc::clone(&state));
        old.retire();
        state.start_library();
        Ok(())
    }

    /// Gives a new window, `win-<n>`, a state showing `workspace` (`None` for a blank window),
    /// which is then open, with its last document rendering for it. It goes where that
    /// workspace's window was last, else a little right of and below the focused window. `focus`: it takes the focus, so it counts as the most
    /// recently focused window; otherwise it is restored at launch and counts as the least.
    /// Returns its label; `build` then opens the window itself.
    fn bind_window(self: &Arc<Self>, workspace: Option<String>, focus: bool) -> String {
        let label = format!("win-{}", self.next_label.fetch_add(1, Ordering::SeqCst));
        if let Some(id) = &workspace {
            self.change_workspace(id, |ws| !std::mem::replace(&mut ws.open, true));
        }
        let cascade = self.cascade();
        let early = workspace.as_deref().map(|id| self.render_last_doc(id));
        let state = WindowState::new(
            self,
            &label,
            workspace,
            Arc::new(OpenQueue::default()),
            early,
        );
        if state.saved_placement().is_none() {
            if let Some(placement) = cascade {
                state.set_initial_placement(placement);
            }
        }
        state.quiet.store(!focus, Ordering::SeqCst);
        write(&self.windows).insert(label.clone(), Arc::clone(&state));
        {
            let mut order = lock(&self.focus);
            if focus {
                order.insert(0, label.clone());
            } else {
                order.push(label.clone());
            }
        }
        state.start_library();
        label
    }

    /// The workspace `id`'s last document, rendered on a thread of its own as boot renders the
    /// first window's: unless its network host is no longer trusted, with the path mappings and
    /// no index yet. The window's startup waits a moment for it, and a render landing later is
    /// opened then, as for the first window. A document that is gone renders nothing.
    fn render_last_doc(&self, id: &str) -> Arc<Slot<Early>> {
        let slot = Arc::new(Slot::default());
        let last = self
            .read_workspace(id, |ws| ws.last_doc.clone())
            .flatten()
            .filter(|doc| self.trusts(doc))
            .map(PathBuf::from);
        let Some(path) = last else {
            slot.fill(Early::default());
            return slot;
        };
        let (mapper, hosts) = (self.mapper(), read(&self.trust).hosts());
        let filled = Arc::clone(&slot);
        let spawned = thread::Builder::new()
            .name("lectern-render".to_owned())
            .spawn(move || {
                let outcome = render_file(&path, &mapper, &hosts);
                filled.fill(Early {
                    doc: Some(EarlyDoc {
                        path,
                        from_args: false,
                        outcome,
                    }),
                    folder: None,
                });
            });
        if let Err(e) = spawned {
            log::error!("couldn't start rendering the last document: {e}");
            slot.fill(Early::default());
        }
        slot
    }

    /// A little right of and below the focused window's normal placement.
    fn cascade(&self) -> Option<WindowPlacement> {
        let focused = lock(&self.focus).first().cloned()?;
        let placement = self.window(&focused)?.saved_placement()?;
        Some(WindowPlacement {
            x: placement.x.saturating_add(CASCADE),
            y: placement.y.saturating_add(CASCADE),
            maximized: false,
            ..placement
        })
    }

    /// Builds the window `label`. If that fails, its state goes and its workspace is closed.
    fn build(&self, label: &str, focus: bool) -> Result<(), String> {
        self.host.open_window(label, focus).inspect_err(|e| {
            log::error!("couldn't open the window {label}: {e}");
            let _life = lock(&self.lifecycle);
            let id = self.let_go(label).and_then(|state| state.workspace_id());
            if let Some(id) = id {
                self.change_workspace(&id, |ws| std::mem::replace(&mut ws.open, false));
            }
        })
    }

    /// Lets go of the window `label`'s state, if it has one: out of the map and the focus order,
    /// with its watcher stopped and nothing more sent to its UI. The state itself drops once the
    /// threads still working for it finish.
    fn let_go(&self, label: &str) -> Option<Arc<WindowState>> {
        let state = write(&self.windows).remove(label)?;
        lock(&self.focus).retain(|l| l != label);
        state.retire();
        Some(state)
    }

    /// Changes the workspaces with `change` and, unless it fails, saves them, with
    /// `settings.json` too when the first workspace's libraries or layout changed.
    fn change_workspaces<R>(
        &self,
        change: impl FnOnce(&mut Workspaces) -> Result<R, String>,
    ) -> Result<R, String> {
        let settings = read(&self.settings);
        let mut workspaces = write(&self.workspaces);
        let mirror = mirrored(&workspaces);
        let result = change(&mut workspaces)?;
        self.saver.workspaces(&workspaces);
        if mirrored(&workspaces) != mirror {
            self.save_settings(&settings, &workspaces);
        }
        Ok(result)
    }

    /// Tells every window the workspaces changed.
    fn workspaces_changed(&self) {
        self.host.emit(Target::All, UiEvent::WorkspacesChanged);
    }
}

impl WindowState {
    /// The workspace the window shows, as its header names it; `None` for a blank window.
    pub(super) fn workspace_summary(&self) -> Option<WorkspaceSummary> {
        let id = self.workspace_id()?;
        let (name, roots, own_theme) = self.app.read_workspace(&id, |ws| {
            (ws.name.clone(), ws.roots.clone(), ws.theme.is_some())
        })?;
        Some(WorkspaceSummary {
            id,
            name,
            open: true,
            current: true,
            roots,
            own_theme,
        })
    }

    /// For a window restored at launch and not shown yet: the window that keeps the focus as it
    /// shows, the most recently focused other one. `None` for any other window, once it has
    /// shown behind, or once something asked to bring it forward.
    pub fn keeps_focus(&self) -> Option<String> {
        if !self.quiet.load(Ordering::SeqCst) {
            return None;
        }
        lock(&self.app.focus)
            .iter()
            .find(|l| **l != self.label)
            .cloned()
    }

    /// As the window restored at launch has shown: whether it still goes behind, which it does
    /// once, unless something asked to bring it forward meanwhile.
    pub fn stays_behind(&self) -> bool {
        self.quiet.swap(false, Ordering::SeqCst)
    }

    /// The window closed, or turned to another workspace through a new state: this one stops its
    /// watcher, starts no scan, sends nothing more to the UI, whose label may be another state's
    /// now, and changes nothing more in its workspace.
    pub(super) fn retire(&self) {
        {
            // Under the workspaces lock, so a change of this state's workspace lands before this
            // or not at all (`change_own_workspace`).
            let _workspaces = write(&self.app.workspaces);
            self.retired.store(true, Ordering::SeqCst);
        }
        let watch = lock(&self.watch).take();
        drop(watch);
    }

    pub(super) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Its UI holds comment text that isn't saved yet (`on`), or no longer does.
    pub fn set_unsaved(&self, on: bool) {
        self.unsaved.store(on, Ordering::SeqCst);
    }

    /// The window's close button was pressed: true when it may close. While its UI holds comment
    /// text that isn't saved yet, it stays, and its UI is asked (`close-requested`); once the user
    /// lets the text go, the UI closes it (`set_unsaved(false)` first, so it isn't asked again).
    /// Quitting closes every window without asking.
    pub fn close_requested(&self) -> bool {
        if !self.unsaved.load(Ordering::SeqCst) || self.app.quitting.load(Ordering::SeqCst) {
            return true;
        }
        self.emit(UiEvent::CloseRequested);
        false
    }
}

/// Whether `path` is a folder, giving up (as not one) after `timeout`: a stalled share must not
/// hold up a launch.
pub fn is_folder(path: &Path, timeout: Duration) -> bool {
    let path = path.to_path_buf();
    probe_with(
        move || {
            if fs::metadata(&path)?.is_dir() {
                Ok(())
            } else {
                Err(io::ErrorKind::NotADirectory.into())
            }
        },
        timeout,
    )
    .is_ok()
}

/// The thread behind `App::second_launch`: launches are routed in order, one at a time, so the
/// last one still wins when several arrive together.
pub(super) fn spawn_launcher(app: Weak<App>) -> Sender<Option<OpenRequest>> {
    let (tx, rx) = mpsc::channel::<Option<OpenRequest>>();
    let spawned = thread::Builder::new()
        .name("lectern-launch".to_owned())
        .spawn(move || {
            while let Ok(request) = rx.recv() {
                let Some(app) = app.upgrade() else {
                    break;
                };
                app.launched(request);
            }
        });
    if let Err(e) = spawned {
        log::error!("couldn't start the launch thread; second launches won't open: {e}");
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Rect, MAIN_WINDOW};
    use crate::state::paths::path_string;
    use crate::state::profile::{load_profile, Profile, SETTINGS_FILE};
    use crate::state::test_support::*;
    use crate::state::WindowShape;
    use lectern_core::ipc::{OpenResult, Settings, SettingsPatch, ThemeId, ThemeMode};
    use lectern_core::watch::WatchEvent;
    use lectern_core::workspace::WORKSPACES_FILE;

    const NORMAL: WindowShape = WindowShape {
        visible: true,
        maximized: false,
        minimized: false,
        fullscreen: false,
    };

    const PLACE: Rect = Rect {
        x: 120,
        y: 80,
        width: 1280,
        height: 860,
    };

    /// Adds a workspace named `name` over `roots` to `profile`, open or closed; returns its id.
    fn add(profile: &mut Profile, name: &str, roots: &[&Path], open: bool) -> String {
        let id = profile.workspaces.create(name);
        let ws = profile.workspaces.get_mut(&id).unwrap();
        ws.roots = roots.iter().map(|r| path_string(r)).collect();
        ws.open = open;
        id
    }

    fn request(path: &Path) -> OpenRequest {
        OpenRequest {
            path: path_string(path),
            t0_ms: None,
            folder: false,
        }
    }

    fn current_of(window: &WindowState) -> PathBuf {
        let current = lock(&window.current);
        current.as_ref().expect("a current document").path.clone()
    }

    fn window(f: &Fixture, label: &str) -> Arc<WindowState> {
        f.app
            .window(label)
            .unwrap_or_else(|| panic!("{label} has no state"))
    }

    /// Closes the window `label` as Tauri does, and waits for its state to drop.
    fn close(f: &Fixture, label: &str) {
        let gone = Arc::downgrade(&window(f, label));
        f.app.window_closing(label);
        f.app.window_destroyed(label);
        wait_until("the closed window's state drops", || {
            gone.upgrade().is_none()
        });
    }

    fn open_ids(workspaces: &Workspaces) -> Vec<&str> {
        workspaces
            .items
            .iter()
            .filter(|ws| ws.open)
            .map(|ws| ws.id.as_str())
            .collect()
    }

    /// Work (main, the first workspace) and Personal (open too, focused less recently), each
    /// with a library.
    fn work_and_personal() -> (TempDir, Profile, String) {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let personal_root = dir.folder("personal");
        let mut profile = profile(&[&work]);
        let personal = add(&mut profile, "Personal", &[&personal_root], true);
        (dir, profile, personal)
    }

    /// Close Personal's window, then quit from Work's: only Work comes back, and Personal stays
    /// in the list. The closed window's state drops.
    #[test]
    fn closing_a_window_then_quitting_reopens_only_the_last() {
        let (dir, profile, personal) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        assert_eq!(f.host.opened_windows(), ["win-1"]);
        assert_eq!(window(&f, "win-1").workspace_id(), Some(personal.clone()));
        close(&f, "win-1");
        assert!(f.app.window("win-1").is_none());
        // The last window: Lectern exits with it, its workspace open.
        f.app.window_closing(MAIN_WINDOW);
        let saved = saved_workspaces(&f);
        assert_eq!(open_ids(&saved), ["w1"]);
        assert!(saved.get(&personal).is_some());
        let next = load_profile(&f.config, None);
        assert_eq!(next.first().map(|ws| ws.id.as_str()), Some("w1"));
        assert_eq!(open_ids(&next.workspaces), ["w1"]);
    }

    /// Quitting with both windows open reopens both, the most recently focused as "main" and the
    /// other behind it, without the focus.
    #[test]
    fn quitting_with_every_window_open_reopens_them_all() {
        let (dir, profile, personal) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        f.app.window_focused("win-1");
        // `ExitRequested` saves each window's placement; nothing closes.
        let saved = saved_workspaces(&f);
        assert_eq!(saved.focus.first(), Some(&personal));
        let next = load_profile(&f.config, None);
        assert_eq!(open_ids(&next.workspaces), ["w1", personal.as_str()]);
        let g = fixture(next, FakeHost::default());
        assert_eq!(g.state.workspace_id(), Some(personal));
        assert!(
            lock(&g.host.opened).is_empty(),
            "only after the first paint"
        );
        g.state.window_shown();
        assert_eq!(*lock(&g.host.opened), [("win-1".to_owned(), false)]);
        assert_eq!(window(&g, "win-1").workspace_id().as_deref(), Some("w1"));
    }

    /// The windows of the other open workspaces open once, after the first window shows, the
    /// least recently focused first and none taking the focus; each hands it back when it shows.
    #[test]
    fn the_other_open_workspaces_reopen_behind_the_first_window() {
        let mut profile = profile(&[]);
        let older = add(&mut profile, "Garden", &[], true);
        let newer = add(&mut profile, "Personal", &[], true);
        profile.workspaces.touch_focus(&newer);
        profile.workspaces.touch_focus("w1");
        let f = fixture(profile, FakeHost::default());
        f.state.window_shown();
        f.state.window_shown();
        let opened = lock(&f.host.opened).clone();
        assert_eq!(
            opened,
            [("win-2".to_owned(), false), ("win-1".to_owned(), false)]
        );
        assert_eq!(window(&f, "win-1").workspace_id(), Some(newer));
        assert_eq!(window(&f, "win-2").workspace_id(), Some(older));
        assert_eq!(*lock(&f.app.focus), [MAIN_WINDOW, "win-1", "win-2"]);
        let restored = window(&f, "win-1");
        assert_eq!(restored.keeps_focus().as_deref(), Some(MAIN_WINDOW));
        assert!(restored.stays_behind());
        assert!(!restored.stays_behind());
        assert_eq!(restored.keeps_focus(), None);
        assert_eq!(f.state.keeps_focus(), None);
    }

    /// A second launch, or Switch to window, for a workspace whose window is being restored
    /// brings that window forward: it shows in front, not behind the focused window.
    #[test]
    fn a_window_asked_for_before_it_shows_comes_forward() {
        let dir = TempDir::new();
        let mut profile = profile(&[]);
        let (personal, _) = with_last_note(&mut profile, &dir, "Personal", true, "tide.md");
        let garden = add(&mut profile, "Garden", &[], true);
        let launched = dir.file("personal/seeds.md", "# Seeds");
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        let (win1, win2) = (window(&f, "win-1"), window(&f, "win-2"));
        assert_eq!(win1.workspace_id(), Some(personal));
        assert_eq!(win2.workspace_id(), Some(garden.clone()));
        assert!(win1.keeps_focus().is_some() && win2.keeps_focus().is_some());
        f.app.launched(Some(request(&launched)));
        assert_eq!(win1.keeps_focus(), None);
        assert!(!win1.stays_behind());
        let outcome = f
            .app
            .open_workspace(MAIN_WINDOW, &garden, OpenWhere::NewWindow);
        assert_eq!(outcome, Ok(WorkspaceOutcome::Focused));
        assert_eq!(win2.keeps_focus(), None);
        assert_eq!(*lock(&f.host.focused), ["win-1", "win-2"]);
    }

    /// Every `open-request` sent, in order: the window, the path, and whether it is a folder to
    /// name a new workspace for.
    fn open_requests_sent(f: &Fixture) -> Vec<(Target, String, bool)> {
        lock(&f.host.events)
            .iter()
            .filter_map(|(target, e)| match e {
                UiEvent::OpenRequest(r) => Some((target.clone(), r.path.clone(), r.folder)),
                _ => None,
            })
            .collect()
    }

    /// A folder launched when only a blank window is left makes no workspace and no root: the
    /// window is sent the folder to name a workspace for, once its UI is ready. A file still
    /// opens there as a loose file.
    #[test]
    fn a_folder_launched_into_a_blank_window_asks_for_a_workspace() {
        let f = fixture(profile(&[]), FakeHost::default());
        let vault = f.dir.folder("vault");
        f.dir.file("vault/notes.md", "# Notes");
        let loose = f.dir.file("loose/tide.md", "# Tide");
        f.app.new_window().unwrap();
        f.app.window_closing(MAIN_WINDOW);
        f.app.window_destroyed(MAIN_WINDOW);
        f.app.launched(Some(request(&vault)));
        assert!(open_requests_sent(&f).is_empty(), "held for its startup");
        let blank = window(&f, "win-1");
        assert!(blank.startup().initial.is_none());
        let folder = (
            Target::Window("win-1".to_owned()),
            path_string(&vault),
            true,
        );
        assert_eq!(open_requests_sent(&f), std::slice::from_ref(&folder));
        // Its UI is ready now, so the next ones go straight through.
        f.app.launched(Some(request(&vault)));
        f.app.launched(Some(request(&loose)));
        wait_until("both are sent", || open_requests_sent(&f).len() == 3);
        let file = (
            Target::Window("win-1".to_owned()),
            path_string(&loose),
            false,
        );
        assert_eq!(open_requests_sent(&f), [folder.clone(), folder, file]);
        assert_eq!(blank.workspace_id(), None);
        let workspaces = f.app.list_workspaces("win-1");
        assert_eq!(workspaces.len(), 1);
        assert!(workspaces[0].roots.is_empty());
    }

    /// A folder chosen in a blank window (a drop, Add folder) opens and joins nothing, not even its
    /// README, and the answer says it is a folder, for the UI to ask for a workspace. A file opens
    /// as a loose file.
    #[test]
    fn a_folder_chosen_in_a_blank_window_asks_for_a_workspace() {
        let f = fixture(profile(&[]), FakeHost::default());
        let vault = f.dir.folder("vault");
        f.dir.file("vault/README.md", "# Vault");
        let loose = f.dir.file("loose/tide.txt", "Tide");
        f.app.new_window().unwrap();
        let blank = window(&f, "win-1");
        let opened = blank.open_user_path(&path_string(&vault));
        assert!(opened.folder);
        assert!(opened.doc.is_none());
        assert!(opened.library.roots.is_empty());
        assert_eq!(blank.workspace_id(), None);
        let opened = blank.open_user_path(&path_string(&loose));
        assert!(!opened.folder);
        assert!(matches!(opened.doc, Some(OpenResult::Ok { .. })));
        assert_eq!(current_of(&blank), loose);
        let workspaces = f.app.list_workspaces("win-1");
        assert_eq!(workspaces.len(), 1);
        assert!(workspaces[0].roots.is_empty());
    }

    /// A settings change already under way as its window turns to another workspace lands
    /// nowhere: not in the workspace the window left, nor in the shared settings.
    #[test]
    fn a_settings_change_racing_its_windows_retirement_applies_nothing() {
        let f = fixture(profile(&[]), FakeHost::default());
        let held = write(&f.app.settings);
        let state = Arc::clone(&f.state);
        let change = thread::spawn(move || {
            state.set_settings(SettingsPatch {
                font_size: Some(19),
                library_visible: Some(false),
                ..SettingsPatch::default()
            })
        });
        // The change waits for the settings lock while the window's state is retired.
        thread::sleep(Duration::from_millis(100));
        f.state.retire();
        drop(held);
        let returned = change.join().unwrap();
        assert_ne!(returned.settings.font_size, 19);
        assert_ne!(read(&f.app.settings).font_size, 19);
        let shown = f.app.read_workspace("w1", |ws| ws.layout.library_visible);
        assert_eq!(shown, Some(true));
        assert!(f.host.settings_changes().is_empty());
    }

    /// An open that finishes after its window turned to another workspace changes nothing in the
    /// workspace it left, which a newer window may have opened a newer note in meanwhile.
    #[test]
    fn a_retired_window_never_overwrites_its_old_workspace() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let older = dir.file("work/older.md", "# Older");
        let newer = dir.file("work/newer.md", "# Newer");
        let mut profile = profile(&[&work]);
        let garden = add(&mut profile, "Garden", &[], false);
        let f = fixture_in(dir, profile, FakeHost::default());
        // An open of the older note is rendering...
        let seq = f.state.next_seq();
        let rendered = f.state.load(&older);
        // ...when the window turns to Garden, and Work opens in a new window, on the newer note.
        f.app
            .open_workspace(MAIN_WINDOW, &garden, OpenWhere::Here)
            .unwrap();
        f.app
            .open_workspace(MAIN_WINDOW, "w1", OpenWhere::NewWindow)
            .unwrap();
        opened_path(&window(&f, "win-1").open_document(&path_string(&newer)));
        // The older open finishes in the retired state.
        opened_path(&f.state.finish_open(seq, &older, rendered));
        let saved = saved_workspaces(&f);
        let work_ws = saved.get("w1").unwrap();
        assert_eq!(work_ws.last_doc, Some(path_string(&newer)));
        let recent: Vec<&str> = work_ws.recent.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(recent, [path_string(&newer)]);
        // Nor does anything else it is still asked to do.
        f.state.remove_recent(&path_string(&newer));
        f.state.track_placement(NORMAL, Some(PLACE));
        f.state.remember_placement(true);
        let after = f
            .app
            .read_workspace("w1", |ws| (ws.recent.len(), ws.placement));
        assert_eq!(after, Some((1, None)));
    }

    /// A launch file in a closed workspace: the first window shows that workspace, which is
    /// open now; the workspace open before comes back behind it after the first paint.
    #[test]
    fn a_launch_file_in_a_closed_workspace_opens_it_in_the_first_window() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let garden = dir.folder("garden");
        let seeds = dir.file("garden/seeds.md", "# Seeds");
        let mut profile = profile(&[&work]);
        let id = add(&mut profile, "Garden", &[&garden], false);
        profile.route_launch(&seeds, false);
        let f = fixture_in(dir, profile, FakeHost::default());
        assert_eq!(f.state.workspace_id(), Some(id.clone()));
        assert_eq!(open_ids(&saved_workspaces(&f)), ["w1", id.as_str()]);
        f.state.window_shown();
        assert_eq!(*lock(&f.host.opened), [("win-1".to_owned(), false)]);
        assert_eq!(window(&f, "win-1").workspace_id().as_deref(), Some("w1"));
    }

    /// Open here turns the window to a closed workspace through a new state: the workspace it
    /// showed is saved closed with its placement, and after the reload the window shows only the
    /// new workspace's library, in its theme. The old state sends nothing more.
    #[test]
    fn open_here_turns_the_window_to_a_closed_workspace() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let plan = dir.file("work/plan.md", "# Plan");
        let personal_root = dir.folder("personal");
        let seeds = dir.file("personal/seeds.md", "# Seeds");
        let mut profile = profile(&[&work]);
        let personal = add(&mut profile, "Personal", &[&personal_root], false);
        profile.workspaces.get_mut(&personal).unwrap().theme = Some(WorkspaceTheme {
            mode: ThemeMode::Dark,
            light: ThemeId::Sepia,
            dark: ThemeId::Nord,
        });
        let f = fixture_in(dir, profile, FakeHost::default());
        assert!(f.state.startup().primary);
        f.state.window_shown();
        wait_until("work is indexed", || settled(&f, &work));
        f.state.track_placement(NORMAL, Some(PLACE));
        // What the command does before the window turns.
        f.state.remember_placement(false);
        let outcome = f
            .app
            .open_workspace(MAIN_WINDOW, &personal, OpenWhere::Here);
        assert_eq!(outcome, Ok(WorkspaceOutcome::Reload));
        let main = window(&f, MAIN_WINDOW);
        assert!(!Arc::ptr_eq(&main, &f.state));
        assert_eq!(main.workspace_id(), Some(personal.clone()));
        assert!(!Arc::ptr_eq(main.early.as_ref().unwrap(), &f.early));
        assert!(!main.ui_shown.is_open());
        assert_eq!(main.saved_placement(), Some(WindowPlacement::from(PLACE)));
        let saved = saved_workspaces(&f);
        let work_ws = saved.get("w1").unwrap();
        assert!(!work_ws.open);
        assert_eq!(work_ws.placement, Some(WindowPlacement::from(PLACE)));
        assert!(saved.get(&personal).unwrap().open);
        assert_eq!(f.host.workspace_changes(), [Target::All]);
        // The old state is retired: its watcher's news reaches no one.
        let sent = lock(&f.host.events).len();
        f.state.on_watch_event(WatchEvent::DocChanged(plan));
        assert_eq!(lock(&f.host.events).len(), sent);
        // The UI reloads and asks again. It is the window that asked first, and the automatic
        // update check hasn't run: this page runs it.
        let payload = main.startup();
        assert!(payload.primary);
        let summary = payload.workspace.unwrap();
        assert_eq!(
            (summary.id.as_str(), summary.current),
            (personal.as_str(), true)
        );
        assert!(matches!(payload.settings.theme_mode, ThemeMode::Dark));
        assert!(matches!(payload.settings.dark_theme, ThemeId::Nord));
        assert_eq!(
            payload.settings.library_roots,
            [path_string(&personal_root)]
        );
        main.window_shown();
        wait_until("personal is indexed", || settled_in(&main, &personal_root));
        let files: Vec<PathBuf> = main
            .quick_open_candidates()
            .iter()
            .map(|c| PathBuf::from(&c.path))
            .collect();
        assert_eq!(files, [seeds]);
    }

    /// A launch held for the window's old state is not the new state's: the new one has its own
    /// queue, waiting for its own startup.
    #[test]
    fn a_turned_window_waits_for_its_new_startup() {
        let mut profile = profile(&[]);
        let personal = add(&mut profile, "Personal", &[], false);
        let f = fixture(profile, FakeHost::default());
        f.state.startup();
        f.app
            .open_workspace(MAIN_WINDOW, &personal, OpenWhere::Here)
            .unwrap();
        let note = f.dir.file("notes/tide.md", "# Tide");
        let main = window(&f, MAIN_WINDOW);
        assert!(main.opens.offer(request(&note)).is_none(), "held");
        let payload = main.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), note);
        assert!(f.host.open_requests().is_empty());
    }

    /// Adds a workspace named `name` over the folder `root` of `dir`, open or closed, whose last
    /// document is `root/<note>`, written there. Returns its id and that document.
    fn with_last_note(
        profile: &mut Profile,
        dir: &TempDir,
        name: &str,
        open: bool,
        note: &str,
    ) -> (String, PathBuf) {
        let root = dir.folder(&name.to_lowercase());
        let doc = dir.file(&format!("{}/{note}", name.to_lowercase()), "# Note");
        let id = add(profile, name, &[&root], open);
        profile.workspaces.get_mut(&id).unwrap().last_doc = Some(path_string(&doc));
        (id, doc)
    }

    /// Turned to another workspace, a window reopens that workspace's last note, rendered for it
    /// as it reloads.
    #[test]
    fn a_turned_window_opens_its_new_workspaces_last_note() {
        let dir = TempDir::new();
        let mut profile = profile(&[]);
        let (personal, note) = with_last_note(&mut profile, &dir, "Personal", false, "tide.md");
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.startup();
        f.app
            .open_workspace(MAIN_WINDOW, &personal, OpenWhere::Here)
            .unwrap();
        let payload = window(&f, MAIN_WINDOW).startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), note);
        assert!(f.host.open_requests().is_empty());
    }

    /// A window restored at launch, or opened in a new window, reopens its workspace's last note;
    /// a blank window has none to reopen, and a note that is gone opens nothing.
    #[test]
    fn a_restored_window_opens_its_last_note() {
        let dir = TempDir::new();
        let mut profile = profile(&[]);
        let (_, note) = with_last_note(&mut profile, &dir, "Personal", true, "tide.md");
        let (garden, gone) = with_last_note(&mut profile, &dir, "Garden", false, "seeds.md");
        std::fs::remove_file(&gone).unwrap();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        let payload = window(&f, "win-1").startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), note);
        f.app
            .open_workspace(MAIN_WINDOW, &garden, OpenWhere::NewWindow)
            .unwrap();
        assert!(window(&f, "win-2").startup().initial.is_none());
        f.app.new_window().unwrap();
        assert!(window(&f, "win-3").early.is_none());
        assert!(f.host.open_requests().is_empty());
    }

    /// A second launch held for a window that is starting wins over the window's last note,
    /// which then never opens.
    #[test]
    fn a_launch_held_for_a_window_wins_over_its_last_note() {
        let dir = TempDir::new();
        let mut profile = profile(&[]);
        let (_, _last) = with_last_note(&mut profile, &dir, "Personal", true, "tide.md");
        let launched = dir.file("personal/seeds.md", "# Seeds");
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        f.app.launched(Some(request(&launched)));
        let win1 = window(&f, "win-1");
        let payload = win1.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), launched);
        thread::sleep(Duration::from_millis(100));
        assert!(f.host.open_requests().is_empty());
        assert_eq!(current_of(&win1), launched);
    }

    /// Quit keeps every open workspace open, even as the windows then close, and the next launch
    /// reopens them all.
    #[test]
    fn quitting_keeps_every_window_for_the_next_launch() {
        let (dir, profile, personal) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        // What the command does first: each window's placement goes into its workspace.
        for label in [MAIN_WINDOW, "win-1"] {
            let state = window(&f, label);
            state.track_placement(NORMAL, Some(PLACE));
            state.remember_placement(false);
        }
        f.app.quit();
        assert!(*lock(&f.host.exited));
        // Should the windows close one by one as Lectern exits, nothing changes.
        f.app.window_closing("win-1");
        f.app.window_closing(MAIN_WINDOW);
        assert!(f.app.window("win-1").is_some());
        let saved = saved_workspaces(&f);
        assert_eq!(open_ids(&saved), ["w1", personal.as_str()]);
        assert!(saved
            .items
            .iter()
            .all(|ws| ws.placement == Some(WindowPlacement::from(PLACE))));
        let next = load_profile(&f.config, None);
        assert_eq!(open_ids(&next.workspaces), ["w1", personal.as_str()]);
        let g = fixture(next, FakeHost::default());
        g.state.window_shown();
        assert_eq!(g.host.opened_windows(), ["win-1"]);
    }

    /// Quitting from one window doesn't while another window's UI holds comment text that isn't
    /// saved yet: those windows are named instead, by workspace (or as a blank window), the most
    /// recently focused first, unless the quit is forced. The caller's own text is for its own UI
    /// to ask about.
    #[test]
    fn quitting_names_other_windows_with_unsaved_comments_unless_forced() {
        let (dir, profile, _) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        let mut before = 0;
        f.state.set_unsaved(true);
        window(&f, "win-1").set_unsaved(true);
        let refused = f.app.quit_from(MAIN_WINDOW, false, || before += 1);
        assert_eq!(refused, ["Personal"]);
        f.app.new_window().unwrap();
        window(&f, "win-2").set_unsaved(true);
        let refused = f.app.quit_from(MAIN_WINDOW, false, || before += 1);
        assert_eq!(refused, ["a blank window", "Personal"]);
        // Saved since, it no longer counts.
        window(&f, "win-1").set_unsaved(false);
        let refused = f.app.quit_from(MAIN_WINDOW, false, || before += 1);
        assert_eq!(refused, ["a blank window"]);
        assert_eq!(before, 0);
        assert!(!*lock(&f.host.exited));
        // Forced, it quits all the same, once the placements are saved.
        let refused = f.app.quit_from(MAIN_WINDOW, true, || before += 1);
        assert!(refused.is_empty());
        assert_eq!(before, 1);
        assert!(*lock(&f.host.exited));
    }

    /// With no unsaved comment elsewhere, quitting goes ahead at once.
    #[test]
    fn quitting_with_nothing_unsaved_elsewhere_goes_ahead() {
        let (dir, profile, _) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        f.state.set_unsaved(true);
        assert!(f.app.quit_from(MAIN_WINDOW, false, || ()).is_empty());
        assert!(*lock(&f.host.exited));
    }

    /// A workspace open in another window is never opened twice: that window comes forward.
    #[test]
    fn opening_a_workspace_open_elsewhere_focuses_its_window() {
        let (dir, profile, personal) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        let changes = f.host.workspace_changes().len();
        for place in [OpenWhere::NewWindow, OpenWhere::Here] {
            let outcome = f.app.open_workspace(MAIN_WINDOW, &personal, place);
            assert_eq!(outcome, Ok(WorkspaceOutcome::Focused));
        }
        assert_eq!(f.host.opened_windows(), ["win-1"]);
        assert_eq!(*lock(&f.host.focused), ["win-1", "win-1"]);
        assert_eq!(f.state.workspace_id().as_deref(), Some("w1"));
        assert_eq!(f.host.workspace_changes().len(), changes);
        assert_eq!(
            f.app.open_workspace(MAIN_WINDOW, "w9", OpenWhere::Here),
            Err(UNKNOWN_WORKSPACE.to_owned())
        );
    }

    /// A new workspace with a blank name is "Workspace 2", in a new window placed a little right
    /// of and below the focused one. With no folders, it is forgotten when that window closes.
    #[test]
    fn a_new_workspace_without_folders_is_forgotten_when_its_window_closes() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.state.track_placement(NORMAL, Some(PLACE));
        let outcome = f
            .app
            .create_workspace(MAIN_WINDOW, "  ", OpenWhere::NewWindow, None);
        assert_eq!(outcome, Ok(WorkspaceOutcome::Opened));
        assert_eq!(*lock(&f.host.opened), [("win-1".to_owned(), true)]);
        assert_eq!(f.host.workspace_changes(), [Target::All]);
        let list = f.app.list_workspaces(MAIN_WINDOW);
        let [main, new] = list.as_slice() else {
            panic!("{list:?}");
        };
        assert!(main.current && main.open);
        assert_eq!(new.name, "Workspace 2");
        assert!(new.open && !new.current && new.roots.is_empty());
        assert!(f.app.list_workspaces("win-1")[1].current);
        let win1 = window(&f, "win-1");
        assert_eq!(
            win1.saved_placement(),
            Some(WindowPlacement {
                x: 152,
                y: 112,
                width: 1280,
                height: 860,
                maximized: false,
            })
        );
        assert_eq!(*lock(&f.app.focus), ["win-1", MAIN_WINDOW]);
        drop(win1);
        close(&f, "win-1");
        let names: Vec<String> = f
            .app
            .list_workspaces(MAIN_WINDOW)
            .into_iter()
            .map(|ws| ws.name)
            .collect();
        assert_eq!(names, ["Main"]);
        assert_eq!(saved_workspaces(&f).items.len(), 1);
        assert_eq!(f.host.workspace_changes().len(), 2);
    }

    /// The only workspace stays, folders or not, when its window closes before a blank one.
    #[test]
    fn the_only_workspace_is_never_forgotten_on_close() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.app.new_window().unwrap();
        f.app.window_closing(MAIN_WINDOW);
        f.app.window_destroyed(MAIN_WINDOW);
        assert!(f.app.window(MAIN_WINDOW).is_none());
        let saved = saved_workspaces(&f);
        assert_eq!(saved.items.len(), 1);
        assert!(!saved.items[0].open);
    }

    /// A blank window lists the workspaces with none current; Open here makes it show one,
    /// closing nothing; closing a blank window saves nothing.
    #[test]
    fn a_blank_window_takes_a_workspace_here_and_saves_nothing_on_close() {
        let mut profile = profile(&[]);
        let personal = add(&mut profile, "Personal", &[], false);
        let f = fixture(profile, FakeHost::default());
        f.app.new_window().unwrap();
        let blank = window(&f, "win-1");
        assert_eq!(blank.workspace_id(), None);
        assert!(f.app.list_workspaces("win-1").iter().all(|ws| !ws.current));
        assert!(blank.startup().workspace.is_none());
        let outcome = f.app.open_workspace("win-1", &personal, OpenWhere::Here);
        assert_eq!(outcome, Ok(WorkspaceOutcome::Reload));
        assert_eq!(window(&f, "win-1").workspace_id(), Some(personal.clone()));
        let saved = saved_workspaces(&f);
        assert_eq!(open_ids(&saved), ["w1", personal.as_str()]);
        f.app.new_window().unwrap();
        let changes = f.host.workspace_changes().len();
        close(&f, "win-2");
        assert_eq!(f.host.workspace_changes().len(), changes);
        assert_eq!(open_ids(&saved_workspaces(&f)), ["w1", personal.as_str()]);
    }

    /// A blank window's Add folder or drop names the new workspace first: the folder is its
    /// library before the reload, so the reloaded window shows it.
    #[test]
    fn a_new_workspace_keeps_the_folder_it_was_made_with() {
        let f = fixture(profile(&[]), FakeHost::default());
        let garden = f.dir.folder("garden");
        f.dir.file("garden/seeds.md", "# Seeds");
        f.app.new_window().unwrap();
        let outcome = f.app.create_workspace(
            "win-1",
            "Garden",
            OpenWhere::Here,
            Some(&path_string(&garden)),
        );
        assert_eq!(outcome, Ok(WorkspaceOutcome::Reload));
        let win1 = window(&f, "win-1");
        let payload = win1.startup();
        assert_eq!(payload.workspace.unwrap().name, "Garden");
        assert_eq!(payload.settings.library_roots, [path_string(&garden)]);
        win1.window_shown();
        wait_until("the garden is indexed", || settled_in(&win1, &garden));
        assert_eq!(
            f.app
                .create_workspace("win-1", "Loose", OpenWhere::Here, Some("notes")),
            Err("notes isn't a full folder path".to_owned())
        );
    }

    /// A second launch for a note in a closed workspace reopens it in a new window, which opens
    /// the note once its UI is ready; nothing reaches the window already open.
    #[test]
    fn a_second_launch_for_a_closed_workspace_opens_it_in_a_new_window() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let personal_root = dir.folder("personal");
        let note = dir.file("personal/tide.md", "# Tide");
        let mut profile = profile(&[&work]);
        let personal = add(&mut profile, "Personal", &[&personal_root], false);
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.startup();
        f.app.launched(Some(request(&note)));
        assert_eq!(*lock(&f.host.opened), [("win-1".to_owned(), true)]);
        assert_eq!(*lock(&f.host.focused), ["win-1"]);
        let win1 = window(&f, "win-1");
        assert_eq!(win1.workspace_id(), Some(personal.clone()));
        assert!(f.app.read_workspace(&personal, |ws| ws.open).unwrap());
        assert!(f.host.open_requests().is_empty());
        let payload = win1.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), note);
        assert!(f.host.open_requests().is_empty(), "nothing reaches Work");
        // Once its UI is ready, the next one is sent to it alone.
        f.app.launched(Some(request(&note)));
        wait_until("the note is requested", || {
            !f.host.open_requests().is_empty()
        });
        assert_eq!(
            f.host.targets(|e| matches!(e, UiEvent::OpenRequest(_))),
            [Target::Window("win-1".to_owned())]
        );
    }

    /// A second launch while the first window is still before its startup waits for it, and
    /// arrives once, in the startup payload.
    #[test]
    fn a_second_launch_before_startup_waits_for_the_first_window() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.file("notes/tide.md", "# Tide");
        f.app.second_launch(Some(request(&note)));
        wait_until("the launch is held", || f.opens.has_pending());
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), note);
        assert!(f.host.open_requests().is_empty());
        assert!(f.host.opened_windows().is_empty());
    }

    /// With no window left, as Lectern exits, a second launch is dropped.
    #[test]
    fn a_second_launch_with_no_window_left_is_dropped() {
        let f = fixture(profile(&[]), FakeHost::default());
        let note = f.dir.file("notes/tide.md", "# Tide");
        f.app.window_destroyed(MAIN_WINDOW);
        f.app.launched(Some(request(&note)));
        assert!(f.host.opened_windows().is_empty());
        assert!(!f.opens.has_pending());
    }

    #[test]
    fn a_workspace_open_in_a_window_cannot_be_deleted() {
        let mut profile = profile(&[]);
        let share = Path::new(r"\\nas\share\personal");
        let personal = add(&mut profile, "Personal", &[share], false);
        let f = fixture(profile, FakeHost::default());
        assert!(f.app.trusts(r"\\nas\share\personal\a.md"));
        assert_eq!(
            f.app.delete_workspace(MAIN_WINDOW, "w1").err().as_deref(),
            Some("Close its window first.")
        );
        let left = f.app.delete_workspace(MAIN_WINDOW, &personal).unwrap();
        assert_eq!(left.len(), 1);
        assert!(!f.app.trusts(r"\\nas\share\personal\a.md"));
        assert_eq!(f.host.workspace_changes(), [Target::All]);
        assert_eq!(saved_workspaces(&f).items.len(), 1);
    }

    #[test]
    fn renaming_a_workspace_tells_every_window() {
        let f = fixture(profile(&[]), FakeHost::default());
        let list = f
            .app
            .rename_workspace(MAIN_WINDOW, "w1", "  Work ")
            .unwrap();
        assert_eq!(list[0].name, "Work");
        assert_eq!(f.host.workspace_changes(), [Target::All]);
        assert_eq!(saved_workspaces(&f).items[0].name, "Work");
        assert!(f.app.rename_workspace(MAIN_WINDOW, "w1", " ").is_err());
        assert_eq!(f.host.workspace_changes().len(), 1);
        assert_eq!(f.app.suggest_workspace_name(), "Workspace 2");
    }

    /// A theme of its own starts as the shared one and then changes alone; turned off, the
    /// window follows the shared theme again. The window hears its settings each time.
    #[test]
    fn a_workspace_theme_of_its_own_starts_from_the_shared_one() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.state.set_settings(SettingsPatch {
            dark_theme: Some(ThemeId::Mocha),
            ..SettingsPatch::default()
        });
        lock(&f.host.events).clear();
        let own = f.app.set_workspace_theme(MAIN_WINDOW, true).unwrap();
        assert!(matches!(own.settings.dark_theme, ThemeId::Mocha));
        assert!(f.app.list_workspaces(MAIN_WINDOW)[0].own_theme);
        let theme = f.app.read_workspace("w1", |ws| ws.theme.clone()).flatten();
        assert!(matches!(theme.map(|t| t.dark), Some(ThemeId::Mocha)));
        f.state.set_settings(SettingsPatch {
            dark_theme: Some(ThemeId::Nord),
            ..SettingsPatch::default()
        });
        assert!(matches!(read(&f.app.settings).dark_theme, ThemeId::Mocha));
        assert!(matches!(f.state.settings().dark_theme, ThemeId::Nord));
        let shared = f.app.set_workspace_theme(MAIN_WINDOW, false).unwrap();
        assert!(matches!(shared.settings.dark_theme, ThemeId::Mocha));
        assert!(f.app.read_workspace("w1", |ws| ws.theme.is_none()).unwrap());
        let told: Vec<Target> = f
            .host
            .settings_changes()
            .into_iter()
            .map(|(target, _)| target)
            .collect();
        let main = Target::Window(MAIN_WINDOW.to_owned());
        assert_eq!(told, [main.clone(), main.clone(), main]);
    }

    /// A blank window keeps its layout for itself while it lasts: a change stays (the answer
    /// doesn't undo it), only that window hears it, and nothing is saved. Once the window takes a
    /// workspace, the workspace's layout shows.
    #[test]
    fn a_blank_window_keeps_its_own_layout_until_it_takes_a_workspace() {
        let mut profile = profile(&[]);
        let personal = add(&mut profile, "Personal", &[], false);
        profile
            .workspaces
            .get_mut(&personal)
            .unwrap()
            .layout
            .library_width = 333;
        let f = fixture(profile, FakeHost::default());
        f.app.new_window().unwrap();
        f.app.new_window().unwrap();
        let blank = window(&f, "win-1");
        let files = || {
            f.app.flush();
            [WORKSPACES_FILE, SETTINGS_FILE].map(|name| fs::read(f.config.join(name)).ok())
        };
        let saved = files();
        lock(&f.host.events).clear();
        let answer = blank.set_settings(SettingsPatch {
            library_visible: Some(false),
            library_width: Some(400),
            comments_visible: Some(false),
            ..SettingsPatch::default()
        });
        let layout = |s: &Settings| (s.library_visible, s.library_width, s.comments_visible);
        assert_eq!(layout(&answer.settings), (false, 400, false));
        assert_eq!(layout(&blank.settings()), (false, 400, false));
        assert!(answer.rev > 0);
        // The other blank window, and the workspace's, keep theirs.
        assert_eq!(layout(&window(&f, "win-2").settings()), (true, 280, true));
        assert_eq!(layout(&f.state.settings()), (true, 280, true));
        let told: Vec<Target> = f
            .host
            .settings_changes()
            .into_iter()
            .map(|(target, _)| target)
            .collect();
        assert_eq!(told, [Target::Window("win-1".to_owned())]);
        assert_eq!(files(), saved, "a blank window's layout is never saved");
        f.app
            .open_workspace("win-1", &personal, OpenWhere::Here)
            .unwrap();
        assert_eq!(layout(&window(&f, "win-1").settings()), (true, 333, true));
    }

    /// The process's one automatic update check runs in the window that asked first: again after
    /// it turns to another workspace (its page reloads before the check), never in another
    /// window, and no more once it has run.
    #[test]
    fn the_first_window_runs_the_update_check_until_it_has() {
        let (dir, mut profile, _) = work_and_personal();
        let garden = add(&mut profile, "Garden", &[], false);
        let f = fixture_in(dir, profile, FakeHost::default());
        assert!(f.state.startup().primary);
        f.state.window_shown();
        assert!(!window(&f, "win-1").startup().primary);
        f.app
            .open_workspace(MAIN_WINDOW, &garden, OpenWhere::Here)
            .unwrap();
        assert!(window(&f, MAIN_WINDOW).startup().primary);
        f.app.update_check_ran();
        f.app
            .open_workspace(MAIN_WINDOW, "w1", OpenWhere::Here)
            .unwrap();
        assert!(!window(&f, MAIN_WINDOW).startup().primary);
    }

    /// The startup payload lists every workspace, the window's own marked current, so the UI can
    /// word the title before its first paint without asking.
    #[test]
    fn the_startup_payload_lists_every_workspace() {
        let (dir, profile, _) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        let listed = |window: &Arc<WindowState>| -> Vec<(String, bool, bool)> {
            window
                .startup()
                .workspaces
                .into_iter()
                .map(|ws| (ws.name, ws.open, ws.current))
                .collect()
        };
        let named = |name: &str, open: bool, current: bool| (name.to_owned(), open, current);
        assert_eq!(
            listed(&f.state),
            [named("Main", true, true), named("Personal", false, false)]
        );
        f.state.window_shown();
        assert_eq!(
            listed(&window(&f, "win-1")),
            [named("Main", true, false), named("Personal", true, true)]
        );
    }

    /// Launches that came before setup made the app go where any later one would: a note in an
    /// open workspace whose window isn't restored yet opens in a window of its own, not in the
    /// first window, which then doesn't restore that workspace again.
    #[test]
    fn launches_held_before_setup_are_routed_like_any_other() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        let personal_root = dir.folder("personal");
        let note = dir.file("personal/tide.md", "# Tide");
        let mut profile = profile(&[&work]);
        let personal = add(&mut profile, "Personal", &[&personal_root], true);
        let f = fixture_in(dir, profile, FakeHost::default());
        let held = HeldLaunches::default();
        assert!(held.hold(request(&note)).is_none());
        f.app.route_held(&held);
        wait_until("Personal's window opens", || {
            !f.host.opened_windows().is_empty()
        });
        let win1 = window(&f, "win-1");
        assert_eq!(win1.workspace_id(), Some(personal));
        assert!(!f.opens.has_pending(), "nothing waits for the first window");
        assert_eq!(opened_path(win1.startup().initial.as_ref().unwrap()), note);
        f.state.startup();
        f.state.window_shown();
        assert_eq!(f.host.opened_windows(), ["win-1"]);
    }

    /// Closing a window whose UI holds an unsaved comment asks it first: the window stays, and its
    /// UI alone hears `close-requested`. Without a draft, once the UI let it go, or while Lectern
    /// quits, it closes.
    #[test]
    fn closing_a_window_with_an_unsaved_comment_asks_first() {
        let (dir, profile, _) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        let personal = window(&f, "win-1");
        let asked = || f.host.targets(|e| matches!(e, UiEvent::CloseRequested));
        assert!(personal.close_requested());
        personal.set_unsaved(true);
        assert!(!personal.close_requested());
        assert_eq!(asked(), [Target::Window("win-1".to_owned())]);
        personal.set_unsaved(false);
        assert!(personal.close_requested());
        f.state.set_unsaved(true);
        f.app.quit();
        assert!(f.state.close_requested());
        assert_eq!(asked().len(), 1);
    }

    /// Focusing a window makes its workspace the most recently focused, saved.
    #[test]
    fn focusing_a_window_puts_its_workspace_first() {
        let (dir, profile, personal) = work_and_personal();
        let f = fixture_in(dir, profile, FakeHost::default());
        f.state.window_shown();
        f.app.window_focused("win-1");
        assert_eq!(*lock(&f.app.focus), ["win-1", MAIN_WINDOW]);
        assert_eq!(saved_workspaces(&f).focus[0], personal);
        f.app.window_focused(MAIN_WINDOW);
        assert_eq!(saved_workspaces(&f).focus[0], "w1");
    }
}
