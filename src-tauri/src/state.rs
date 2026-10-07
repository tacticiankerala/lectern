//! What the commands work on. `App` holds what every window shares: the settings, the workspaces,
//! the trusted hosts, the image scope, the path mapper, the highlighter's warm-up and release, the
//! saver, and each window's `WindowState`. A `WindowState` holds what one window shows: the
//! workspace it is bound to, its library and index, its render cache, the document on screen and
//! the watcher over them. Each command is one call into a method of the calling window's
//! `WindowState` (or of `App`), which is spread over the submodules by concern.
//!
//! Methods that touch the file system block, and the commands run them on Tauri's blocking pool.
//! Library roots may sit on a NAS that stalls for seconds, so roots are probed with a timeout and
//! scanned on threads of their own, and each window's watcher is driven from a thread of its own
//! too.
//!
//! Locks are taken in this order, and never held across file system calls or emits: the app's
//! `settings`, `workspaces`, `trust`, `assets`, `state`, then a window's `library`, `cache`,
//! `current`. The app's `mapper`, `notice`, `windows` and `focused`, and a window's `workspace`
//! and `placement`, are leaves: nothing else is locked while one is held. The app's `backgrounds`
//! is taken with nothing else held, and stays held while the grammars' release is told, which
//! then may take `focused`, `windows` and a window's `current` to warm them again.

mod assets;
mod doc;
mod follow;
mod grammars;
mod library;
mod open;
mod open_queue;
mod paths;
mod profile;
mod review;
mod saver;
mod scan;
mod startup;
mod sync;
#[cfg(test)]
mod test_support;
mod trust;
mod watch_control;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use lectern_core::cache::RenderCache;
use lectern_core::ipc::{OpenRequest, Settings, SettingsPatch};
use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::LibraryIndex;
use lectern_core::perf::PerfLog;
use lectern_core::render::highlight::{BackgroundRelease, StartupWarmUp};
use lectern_core::render::RenderedDoc;
use lectern_core::search::ContentCache;
use lectern_core::watch::WatchEvent;
use lectern_core::workspace::{
    apply_patch, effective_settings, Layout, PatchEffect, WindowPlacement, Workspace, Workspaces,
};
use tauri::Window;

pub use self::assets::AssetResponse;
pub use self::doc::{render_file, Early, EarlyDoc};
pub use self::open_queue::OpenQueue;
pub use self::profile::{load_profile, mapper_for, Profile};
pub use self::sync::Slot;
pub use self::watch_control::{Watch, WatchControl};

use self::assets::AssetScope;
use self::grammars::AppGrammars;
use self::library::Library;
use self::paths::same_path;
use self::profile::{all_roots, first_workspace, StateFile};
use self::saver::Saver;
use self::startup::spawn_forwarder;
use self::sync::{lock, read, write, Gate};
use self::trust::Trust;
use crate::app::{Rect, MAIN_WINDOW};
use crate::events::{Host, Target, UiEvent};

const CACHE_CAP: usize = 64;

/// How long the app waits on things that may be slow. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// A root that doesn't answer within this is unavailable.
    pub probe: Duration,
    /// How long `follow` waits to learn whether a mapped path exists.
    pub exists: Duration,
    /// How long `startup` waits for the document rendered during boot.
    pub early: Duration,
    /// How long `startup` waits for the library snapshots.
    pub snapshots: Duration,
    /// How long `add_root` and `retry_root` wait for a tree before answering without one.
    pub root: Duration,
    /// Scans and the watcher start once the window has shown its first document, or after this.
    pub scan_delay: Duration,
    /// The compiled grammars are released once every window has been in the background this long.
    pub release_after: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            probe: Duration::from_secs(3),
            exists: Duration::from_secs(3),
            early: Duration::from_secs(2),
            snapshots: Duration::from_millis(1500),
            root: Duration::from_secs(8),
            scan_delay: Duration::from_secs(2),
            release_after: Duration::from_secs(60),
        }
    }
}

/// The document on screen.
struct Current {
    path: PathBuf,
    /// The `open_document` call that made it current; an older call never replaces it.
    seq: u64,
    /// Rendered without its root in the index, or with wikilinks the index couldn't resolve:
    /// worth rendering again once the index changes.
    stale: bool,
    /// The index generation it was rendered against.
    rendered_gen: u64,
    /// The index generation it was last refreshed for, so a refresh happens once per generation.
    refreshed_at: Option<u64>,
    /// What it rendered to, for the languages to warm first.
    doc: Option<Arc<RenderedDoc>>,
}

/// Everything `App` needs from boot.
pub struct Boot {
    pub config_dir: PathBuf,
    pub snapshot_dir: PathBuf,
    pub perf: Arc<PerfLog>,
    pub exit_after_paint: bool,
    pub profile: Profile,
    /// The document boot rendered, for the first window.
    pub early: Arc<Slot<Early>>,
    /// Starts the highlighter's warm-up; the first paint may be what it waits for.
    pub warm: Arc<StartupWarmUp>,
    /// Second launches held for the first window.
    pub opens: Arc<OpenQueue>,
    pub timings: Timings,
    /// Not running from the folder Lectern was installed in (`updater::detect_portable`).
    pub portable: bool,
}

/// What every window shares, and each window's state.
pub struct App {
    host: Arc<dyn Host>,
    snapshot_dir: PathBuf,
    perf: Arc<PerfLog>,
    exit_after_paint: bool,
    timings: Timings,
    portable: bool,
    /// The settings every window shares. Their libraries and layout aren't read: each window has
    /// its workspace's, and `settings.json` mirrors the first workspace's.
    settings: RwLock<Settings>,
    workspaces: RwLock<Workspaces>,
    /// The network hosts Lectern may reach, in any window.
    trust: RwLock<Trust>,
    /// The folders the image protocol may serve from, in any window.
    assets: RwLock<AssetScope>,
    /// The reading positions, shared by every window.
    state: Mutex<StateFile>,
    notice: Mutex<Option<String>>,
    wsl_distro: Option<String>,
    mapper: RwLock<Arc<PathMapper>>,
    warm: Arc<StartupWarmUp>,
    /// Releases the compiled grammars while every window is in the background.
    background: BackgroundRelease,
    /// Whether each window is in the background (unfocused or minimised), by label.
    backgrounds: Mutex<HashMap<String, bool>>,
    saver: Saver,
    /// Builds a window's watch worker, which reports back through the window's state.
    new_watch: Box<dyn Fn(Weak<WindowState>) -> Box<dyn Watch> + Send + Sync>,
    /// Each window's state, by label.
    windows: RwLock<HashMap<String, Arc<WindowState>>>,
    /// The label of the window focused last.
    focused: Mutex<Option<String>>,
}

impl App {
    /// The app, with the state of its first window, "main", which shows the most recently
    /// focused open workspace. `watch` builds each window's watch worker, which reports back
    /// through the window's state.
    pub fn new(
        boot: Boot,
        host: Arc<dyn Host>,
        watch: impl Fn(Weak<WindowState>) -> Box<dyn Watch> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let trust = boot.profile.trust();
        let Profile {
            settings,
            state,
            mut workspaces,
            migrated,
            notice,
            wsl_distro,
            persist,
        } = boot.profile;
        if !persist {
            log::error!("the settings didn't load in time; nothing will be saved this session");
        }
        let first = first_workspace(&workspaces).map(|ws| ws.id.clone());
        // The first window shows it, so it's open now if it wasn't.
        let opened = match first.as_deref().and_then(|id| workspaces.get_mut(id)) {
            Some(ws) if !ws.open => {
                ws.open = true;
                true
            }
            _ => false,
        };
        // Every workspace's roots from the start, so a document's images load at once.
        let mut assets = AssetScope::default();
        assets.set_roots(&all_roots(&workspaces));
        let app = Arc::new_cyclic(|weak: &Weak<App>| Self {
            background: BackgroundRelease::start(
                boot.timings.release_after,
                AppGrammars(weak.clone()),
            ),
            saver: Saver::new(boot.config_dir, persist),
            host,
            snapshot_dir: boot.snapshot_dir,
            perf: boot.perf,
            exit_after_paint: boot.exit_after_paint,
            timings: boot.timings,
            portable: boot.portable,
            mapper: RwLock::new(Arc::new(mapper_for(&settings, wsl_distro.clone()))),
            trust: RwLock::new(trust),
            assets: RwLock::new(assets),
            settings: RwLock::new(settings),
            workspaces: RwLock::new(workspaces),
            state: Mutex::new(state),
            notice: Mutex::new(notice),
            wsl_distro,
            warm: boot.warm,
            new_watch: Box::new(watch),
            windows: RwLock::new(HashMap::new()),
            focused: Mutex::new(None),
            backgrounds: Mutex::new(HashMap::new()),
        });
        if migrated || opened {
            app.saver.workspaces(&read(&app.workspaces));
        }
        app.add_window(MAIN_WINDOW, first, boot.opens, Some(boot.early));
        app
    }

    /// Gives the window `label` a state of its own, showing the workspace `workspace` (`None` for
    /// a blank window). Only the first window has a document rendered during boot, `early`.
    fn add_window(
        self: &Arc<Self>,
        label: &str,
        workspace: Option<String>,
        opens: Arc<OpenQueue>,
        early: Option<Arc<Slot<Early>>>,
    ) {
        let window = WindowState::new(self, label, workspace, opens, early);
        write(&self.windows).insert(label.to_owned(), window);
    }

    /// The state of the window `label`, once it has one.
    pub fn window(&self, label: &str) -> Option<Arc<WindowState>> {
        read(&self.windows).get(label).cloned()
    }

    /// Every window's state.
    fn windows(&self) -> Vec<Arc<WindowState>> {
        read(&self.windows).values().cloned().collect()
    }

    /// The settings of the window showing the workspace `id` (`None` for a blank window).
    fn settings_for(&self, id: Option<&str>) -> Settings {
        let settings = read(&self.settings);
        let workspaces = read(&self.workspaces);
        effective_settings(&settings, id.and_then(|id| workspaces.get(id)))
    }

    /// Applies a change made in the window showing the workspace `id` (`None` for a blank
    /// window): each field goes to that workspace or to the shared settings, as `apply_patch`
    /// decides, and what changed is saved. Every window whose settings changed is told. Returns
    /// the window's settings.
    fn apply_settings(&self, id: Option<&str>, patch: SettingsPatch) -> Settings {
        let (window_settings, effect) = {
            let mut settings = write(&self.settings);
            let mut workspaces = write(&self.workspaces);
            let mirror = mirrored(&workspaces);
            let effect = apply_patch(
                &mut settings,
                id.and_then(|id| workspaces.get_mut(id)),
                patch,
            );
            if effect.workspace_changed {
                self.saver.workspaces(&workspaces);
            }
            if effect.shared_changed || mirrored(&workspaces) != mirror {
                self.save_settings(&settings, &workspaces);
            }
            let window_settings =
                effective_settings(&settings, id.and_then(|id| workspaces.get(id)));
            (window_settings, effect)
        };
        self.settings_changed(id, effect);
        window_settings
    }

    /// Sends `settings-changed` to each window whose settings a change made in the window showing
    /// the workspace `id` altered, with that window's own: every window when the shared settings
    /// changed, else only the windows showing that workspace.
    fn settings_changed(&self, id: Option<&str>, effect: PatchEffect) {
        for window in self.windows() {
            if effect.shared_changed
                || (effect.workspace_changed && window.workspace_id().as_deref() == id)
            {
                window.emit(UiEvent::SettingsChanged(window.settings()));
            }
        }
    }

    /// Reads the workspace `id`, unless it is gone.
    fn read_workspace<R>(&self, id: &str, f: impl FnOnce(&Workspace) -> R) -> Option<R> {
        read(&self.workspaces).get(id).map(f)
    }

    /// Changes the workspace `id` with `change`, which says whether it changed anything, and
    /// saves it, with `settings.json` too when its mirror changed. False when nothing changed or
    /// the workspace is gone.
    fn change_workspace(&self, id: &str, change: impl FnOnce(&mut Workspace) -> bool) -> bool {
        let settings = read(&self.settings);
        let mut workspaces = write(&self.workspaces);
        let mirror = mirrored(&workspaces);
        if !workspaces.get_mut(id).is_some_and(change) {
            return false;
        }
        self.saver.workspaces(&workspaces);
        if mirrored(&workspaces) != mirror {
            self.save_settings(&settings, &workspaces);
        }
        true
    }

    /// Saves the shared settings, with the first workspace's libraries and layout, where an
    /// older Lectern reads them.
    fn save_settings(&self, settings: &Settings, workspaces: &Workspaces) {
        let mut mirrored = settings.clone();
        workspaces.mirror_into(&mut mirrored);
        self.saver.settings(&mirrored);
    }

    fn mapper(&self) -> Arc<PathMapper> {
        Arc::clone(&read(&self.mapper))
    }

    /// Whether Lectern may touch `path`: local, or on a network host the user chose.
    fn trusts(&self, path: &str) -> bool {
        read(&self.trust).allows(path)
    }

    /// Answers an image-protocol request for `request_path` (the URL path, percent-encoded).
    /// Every check is made in memory, under brief read locks, before the file is read.
    pub fn serve_asset(&self, request_path: &str) -> AssetResponse {
        assets::serve(
            request_path,
            |raw| assets::decide(raw, &read(&self.trust), &read(&self.assets)),
            assets::read_capped,
        )
    }

    /// Rebuilds the trusted hosts and the image scope's roots from every workspace's roots, open
    /// or closed, and the path mappings; true when the trusted hosts changed.
    fn reconfigure_trust(&self) -> bool {
        let (roots, mappings) = {
            let settings = read(&self.settings);
            let roots = all_roots(&read(&self.workspaces));
            (roots, settings.path_mappings.clone())
        };
        let changed = write(&self.trust).configure(&roots, &mappings);
        write(&self.assets).set_roots(&roots);
        changed
    }

    /// The trusted hosts or the path mappings changed, and with them what a render shows: no
    /// window serves a render it cached before.
    fn forget_renders(&self) {
        for window in self.windows() {
            window.index_changed(None);
        }
    }

    /// The path mappings changed: a link or image any window resolved may point elsewhere now,
    /// so every window's document is rendered again.
    fn remap(&self) {
        let mapper = mapper_for(&read(&self.settings), self.wsl_distro.clone());
        *write(&self.mapper) = Arc::new(mapper);
        self.reconfigure_trust();
        self.forget_renders();
        for window in self.windows() {
            window.refresh_current_now();
        }
    }

    pub fn perf_mark(&self, name: &str, ms: Option<f64>) {
        self.perf.mark(name, ms);
        if name == "first-paint" {
            if self.exit_after_paint {
                self.host.exit();
            }
            self.warm.first_paint();
        }
    }

    /// Writes pending workspaces, settings and state now; called as the app exits.
    pub fn flush(&self) {
        self.saver.flush(Duration::from_secs(2));
    }
}

/// What `settings.json` mirrors: the first workspace's libraries and layout.
fn mirrored(workspaces: &Workspaces) -> Option<(Vec<String>, Layout)> {
    workspaces
        .items
        .first()
        .map(|ws| (ws.roots.clone(), ws.layout.clone()))
}

/// What one window shows: the workspace it is bound to, its library and index, its render cache,
/// the document on screen and the watcher over them. `App` keeps one for each window, by label.
pub struct WindowState {
    app: Arc<App>,
    label: String,
    /// The id of the workspace the window shows; `None` for a blank window.
    workspace: Mutex<Option<String>>,
    library: Mutex<Library>,
    /// Bumped, under the cache lock, whenever the index changes: a render begun before then
    /// isn't cached.
    index_gen: AtomicU64,
    cache: Mutex<RenderCache>,
    content: ContentCache,
    current: Mutex<Option<Current>>,
    /// Numbers opens, so only the latest becomes the current document.
    open_seq: AtomicU64,
    snapshots_loaded: Gate,
    /// The window's ready gate: it is showing its first document, so scans may start.
    ui_shown: Gate,
    /// The document rendered during boot; only the first window has one.
    early: Option<Arc<Slot<Early>>>,
    /// Second launches held until the window's UI asks for its startup payload.
    opens: Arc<OpenQueue>,
    /// Second launches after startup, resolved one at a time on a thread of their own.
    forwards: Sender<OpenRequest>,
    watch: Box<dyn Watch>,
    /// The window's last normal (not maximised, minimised or full screen) placement.
    placement: Mutex<Option<WindowPlacement>>,
}

impl WindowState {
    /// The state of the window `label`, showing the workspace `workspace`, with its roots and
    /// where that workspace's window was last placed.
    fn new(
        app: &Arc<App>,
        label: &str,
        workspace: Option<String>,
        opens: Arc<OpenQueue>,
        early: Option<Arc<Slot<Early>>>,
    ) -> Arc<Self> {
        let (roots, placement) = workspace
            .as_deref()
            .and_then(|id| app.read_workspace(id, |ws| (ws.roots.clone(), ws.placement)))
            .unwrap_or_default();
        let mut library = Library::default();
        for root in &roots {
            let root = PathBuf::from(root);
            if !library.roots.iter().any(|r| same_path(&r.path, &root)) {
                library.push_root(root, false);
            }
        }
        Arc::new_cyclic(|weak: &Weak<WindowState>| Self {
            forwards: spawn_forwarder(weak.clone()),
            watch: (app.new_watch)(weak.clone()),
            app: Arc::clone(app),
            label: label.to_owned(),
            workspace: Mutex::new(workspace),
            library: Mutex::new(library),
            index_gen: AtomicU64::new(0),
            cache: Mutex::new(RenderCache::new(CACHE_CAP)),
            content: ContentCache::new(),
            current: Mutex::new(None),
            open_seq: AtomicU64::new(0),
            snapshots_loaded: Gate::default(),
            ui_shown: Gate::default(),
            early,
            opens,
            placement: Mutex::new(placement),
        })
    }

    /// The id of the workspace the window shows; `None` for a blank window.
    fn workspace_id(&self) -> Option<String> {
        lock(&self.workspace).clone()
    }

    /// The window's settings: the shared ones, with its workspace's libraries, layout and theme.
    pub fn settings(&self) -> Settings {
        self.app.settings_for(self.workspace_id().as_deref())
    }

    /// The library roots of the window's workspace, in order.
    fn roots(&self) -> Vec<String> {
        self.workspace_id()
            .and_then(|id| self.app.read_workspace(&id, |ws| ws.roots.clone()))
            .unwrap_or_default()
    }

    fn index(&self) -> Arc<LibraryIndex> {
        Arc::clone(&lock(&self.library).index)
    }

    fn mapper(&self) -> Arc<PathMapper> {
        self.app.mapper()
    }

    fn next_seq(&self) -> u64 {
        self.open_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Whether Lectern may touch `path`: local, or on a network host the user chose.
    fn trusts(&self, path: &str) -> bool {
        self.app.trusts(path)
    }

    /// Sends `event` to this window's UI alone.
    fn emit(&self, event: UiEvent) {
        self.app
            .host
            .emit(Target::Window(self.label.clone()), event);
    }

    /// The window is showing the first document: scans may start, and that document is
    /// re-rendered if the index can now do better.
    pub fn window_shown(&self) {
        self.app.perf.mark("window-shown", None);
        self.ui_shown.open();
        self.refresh_if_stale(None);
    }

    /// Applies a settings change made in this window: the libraries, the layout and (when the
    /// workspace has its own) the theme go to its workspace, the rest to every window's settings.
    /// Returns the window's settings.
    pub fn set_settings(self: &Arc<Self>, patch: SettingsPatch) -> Settings {
        let roots_changed = patch.library_roots.is_some();
        let mappings_changed = patch.path_mappings.is_some();
        let settings = self
            .app
            .apply_settings(self.workspace_id().as_deref(), patch);
        if mappings_changed {
            self.app.remap();
        }
        if roots_changed {
            for (root, gen) in self.sync_roots() {
                self.start_root(&root, gen, false);
            }
            self.watch_user_roots();
        }
        settings
    }

    /// Reports a watcher event: document and review changes go to the UI, library changes rescan
    /// the root. A review change also rescans the library roots holding the note, for its comment
    /// count: on a share without change notifications the poll that saw it is all there is. Runs
    /// on the watcher's thread, so it never calls back into the watcher.
    pub fn on_watch_event(self: &Arc<Self>, event: WatchEvent) {
        match event {
            WatchEvent::DocChanged(path) => self.emit(UiEvent::DocChanged(path)),
            WatchEvent::DocRemoved(path) => self.emit(UiEvent::DocRemoved(path)),
            WatchEvent::LibraryChanged(root) => self.request_scan(&root, None),
            // The watcher stats the sidecar either way; with the feature off nothing comes of it.
            WatchEvent::ReviewChanged(path) => {
                if self.reviews_on() {
                    for root in self.user_roots_holding(&path) {
                        self.request_scan(&root, None);
                    }
                    self.emit(UiEvent::ReviewChanged(path));
                }
            }
        }
    }

    pub fn saved_placement(&self) -> Option<WindowPlacement> {
        *lock(&self.placement)
    }

    /// Remembers the window's normal placement after a move or resize.
    pub fn track_window(&self, window: &Window) {
        let shape = WindowShape::of(window);
        let rect = match (window.outer_position(), window.inner_size()) {
            (Ok(pos), Ok(size)) if shape.is_normal() => Some(Rect {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
            }),
            _ => None,
        };
        self.track_placement(shape, rect);
    }

    /// Takes `rect` as the normal placement when the window is in its normal shape.
    fn track_placement(&self, shape: WindowShape, rect: Option<Rect>) {
        if let (true, Some(rect)) = (shape.is_normal(), rect) {
            *lock(&self.placement) = Some(WindowPlacement::from(rect));
        }
    }

    /// Puts the window placement into its workspace, as the window closes.
    pub fn remember_window(&self, window: &Window) {
        // A window already destroyed has nothing to tell.
        let Ok(maximized) = window.is_maximized() else {
            return;
        };
        self.track_window(window);
        self.remember_placement(maximized);
    }

    /// Puts the normal placement, maximised or not, into the window's workspace.
    fn remember_placement(&self, maximized: bool) {
        let Some(placement) = *lock(&self.placement) else {
            return;
        };
        let Some(id) = self.workspace_id() else {
            return;
        };
        self.app.change_workspace(&id, |ws| {
            ws.placement = Some(WindowPlacement {
                maximized,
                ..placement
            });
            true
        });
    }

    pub fn set_initial_placement(&self, placement: WindowPlacement) {
        *lock(&self.placement) = Some(placement);
    }
}

/// The window's state, for telling its normal placement from a passing one.
#[derive(Clone, Copy, Debug)]
struct WindowShape {
    visible: bool,
    maximized: bool,
    minimized: bool,
    fullscreen: bool,
}

impl WindowShape {
    /// What can't be read counts against: a placement is kept only when surely normal.
    fn of(window: &Window) -> Self {
        Self {
            visible: window.is_visible().unwrap_or(false),
            maximized: window.is_maximized().unwrap_or(true),
            minimized: window.is_minimized().unwrap_or(true),
            fullscreen: window.is_fullscreen().unwrap_or(true),
        }
    }

    /// Shown, and not maximised, minimised or full screen (focus mode).
    fn is_normal(self) -> bool {
        self.visible && !self.maximized && !self.minimized && !self.fullscreen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::*;
    use lectern_core::ipc::OpenResult;
    use lectern_core::library::pathmap::asset_url;
    use std::path::Path;
    use std::{fs, thread};

    use crate::state::paths::path_string;
    use std::time::Duration;

    use lectern_core::ipc::SavedPosition;
    use lectern_core::workspace::WORKSPACES_FILE;

    use crate::state::profile::{SETTINGS_FILE, STATE_FILE, UNLOADED_NOTICE};
    use crate::state::saver::SAVE_DEBOUNCE;

    /// Focus mode's full screen, like maximised and minimised, is not the window's normal
    /// placement: closing in focus mode must keep the rect to restore.
    #[test]
    fn only_a_normal_window_updates_the_remembered_placement() {
        let f = fixture(Profile::unloaded(), FakeHost::default());
        let normal = WindowShape {
            visible: true,
            maximized: false,
            minimized: false,
            fullscreen: false,
        };
        let rect = Rect {
            x: 120,
            y: 80,
            width: 1280,
            height: 860,
        };
        f.state.track_placement(normal, Some(rect));
        assert_eq!(f.state.saved_placement(), Some(WindowPlacement::from(rect)));
        let screen = Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        };
        let passing = [
            WindowShape {
                fullscreen: true,
                ..normal
            },
            WindowShape {
                maximized: true,
                ..normal
            },
            WindowShape {
                minimized: true,
                ..normal
            },
            WindowShape {
                visible: false,
                ..normal
            },
        ];
        for shape in passing {
            f.state.track_placement(shape, Some(screen));
            assert_eq!(
                f.state.saved_placement(),
                Some(WindowPlacement::from(rect)),
                "{shape:?}"
            );
        }
    }

    #[test]
    fn an_unloaded_profile_saves_nothing() {
        let f = fixture(Profile::unloaded(), FakeHost::default());
        let doc = f.dir.file("a.md", "# A");
        f.state.set_settings(SettingsPatch {
            font_size: Some(20),
            ..SettingsPatch::default()
        });
        f.state.save_position(
            &path_string(&doc),
            SavedPosition {
                heading_id: None,
                offset: 0.0,
                line: Some(1),
                fraction: 0.5,
            },
        );
        f.state.open_document(&path_string(&doc));
        f.app.flush();
        thread::sleep(SAVE_DEBOUNCE * 2);
        assert!(!f.config.join(SETTINGS_FILE).exists());
        assert!(!f.config.join(STATE_FILE).exists());
        assert!(!f.config.join(WORKSPACES_FILE).exists());
        let payload = f.state.startup();
        assert_eq!(payload.startup_notice.as_deref(), Some(UNLOADED_NOTICE));
    }

    #[test]
    fn a_loaded_profile_saves_its_changes() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.state.set_settings(SettingsPatch {
            font_size: Some(20),
            ..SettingsPatch::default()
        });
        f.app.flush();
        let saved = fs::read_to_string(f.config.join(SETTINGS_FILE)).unwrap();
        assert!(saved.contains("\"fontSize\":20"), "{saved}");
    }

    #[test]
    fn documents_on_untrusted_network_hosts_are_refused_untouched() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = r"\\lectern-untrusted.invalid\share\a.md";
        let refusal = trust::refusal(doc);
        let started = std::time::Instant::now();
        match f.state.open_document(doc) {
            OpenResult::Err { error } => assert_eq!(error.message, refusal),
            OpenResult::Ok { .. } => panic!("opened an untrusted host"),
        }
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(lock(&f.state.current).is_none());
        assert!(!lock(&f.watched)
            .iter()
            .any(|w| matches!(w, Watched::Doc(_))));
        assert_eq!(f.state.open_in_editor(doc, Some(3)), Err(refusal.clone()));
        assert_eq!(f.state.reveal_in_explorer(doc), Err(refusal));
    }

    #[test]
    fn images_render_against_the_trusted_hosts() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = f
            .dir
            .file("a.md", "![nas](\\\\\\\\lectern-mapped\\share\\x.png)\n");
        let html = |f: &Fixture| match f.state.open_document(&path_string(&doc)) {
            OpenResult::Ok { doc } => doc.html,
            OpenResult::Err { error } => panic!("{}", error.message),
        };
        assert!(html(&f).contains("img-blocked"));
        f.state.set_settings(SettingsPatch {
            path_mappings: Some(vec![lectern_core::ipc::PathMapping {
                from: "/home/me/shared".to_owned(),
                to: r"\\lectern-mapped\share".to_owned(),
            }]),
            ..SettingsPatch::default()
        });
        assert!(f.state.trusts(r"\\LECTERN-MAPPED\share\x.png"));
        let trusted = html(&f);
        assert!(!trusted.contains("img-blocked"), "{trusted}");
        assert!(trusted.contains("lectern-mapped"), "{trusted}");
    }

    /// A new mapping changes where the open document's links point, so it is rendered again even
    /// though the index already covers it and nothing marked it stale.
    #[test]
    fn changing_the_path_mappings_refreshes_the_open_document() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/a.md", "[x](/home/me/shared/x.md)\n");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        f.state.window_shown();
        wait_until("the root is indexed", || settled(&f, &root));
        let html = |f: &Fixture| match f.state.open_document(&path_string(&doc)) {
            OpenResult::Ok { doc } => doc.html,
            OpenResult::Err { error } => panic!("{}", error.message),
        };
        assert!(html(&f).contains(r#"data-target="/home/me/shared/x.md""#));
        assert!(!current(&f).1, "the document is stale");
        let before = f.host.doc_changes();
        f.state.set_settings(SettingsPatch {
            path_mappings: Some(vec![lectern_core::ipc::PathMapping {
                from: "/home/me/shared".to_owned(),
                to: r"S:\Notes\My Vault".to_owned(),
            }]),
            ..SettingsPatch::default()
        });
        assert_eq!(f.host.doc_changes(), before + 1);
        let mapped = html(&f);
        assert!(
            mapped.contains(r#"data-target="S:\Notes\My Vault\x.md""#),
            "{mapped}"
        );
    }

    #[test]
    fn images_under_a_root_are_served_from_the_start() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let logo = dir.file("vault/img/logo.png", "png bytes");
        dir.file("vault/notes/doc.md", "![logo](../img/logo.png)\n");
        let outside = dir.file("elsewhere/x.png", "nope");
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let encoded = |path: &std::path::Path| asset_url("", path);
        // No scan or open is needed: the root is in scope as soon as the settings load.
        let response = f.app.serve_asset(&encoded(&logo));
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("image/png"));
        assert_eq!(response.body, b"png bytes");
        assert_eq!(f.app.serve_asset(&encoded(&outside)).status, 403);
        assert_eq!(
            f.app
                .serve_asset(&encoded(&root.join("img").join("gone.png")))
                .status,
            404
        );
    }

    /// A raw `<img>` naming an image by its drive path loads through the image protocol when the
    /// image lies under a library root.
    #[cfg(windows)]
    #[test]
    fn raw_html_images_by_drive_path_load_under_a_root() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let logo = dir.file("vault/img/logo.png", "png bytes");
        let src = path_string(&logo).replace('\\', "/");
        let doc = dir.file(
            "vault/notes/doc.md",
            &format!("<img src=\"{src}\" alt=\"logo\">\n"),
        );
        let f = fixture_in(dir, profile(&[&root]), FakeHost::default());
        let html = match f.state.open_document(&path_string(&doc)) {
            OpenResult::Ok { doc } => doc.html,
            OpenResult::Err { error } => panic!("{}", error.message),
        };
        let logo = std::path::Path::new(&src);
        let url = asset_url(crate::state::doc::ASSET_BASE, logo);
        assert!(html.contains(&format!(r#"src="{url}""#)), "{html}");
        let response = f.app.serve_asset(&asset_url("", logo));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"png bytes");
    }

    #[test]
    fn an_opened_documents_folder_comes_into_scope() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = f.dir.file("trip/notes.md", "# Trip");
        let photo = f.dir.file("trip/photos/day1.jpg", "jpg bytes");
        let path = asset_url("", &photo);
        assert_eq!(f.app.serve_asset(&path).status, 403);
        f.state.open_document(&path_string(&doc));
        assert_eq!(f.app.serve_asset(&path).status, 200);
        // A failed open adds nothing.
        let other = f.dir.file("other/pic.png", "png");
        f.state
            .open_document(&path_string(&f.dir.0.join("other").join("missing.md")));
        assert_eq!(f.app.serve_asset(&asset_url("", &other)).status, 403);
    }

    /// A window that is shown, and not maximised, minimised or full screen.
    const NORMAL: WindowShape = WindowShape {
        visible: true,
        maximized: false,
        minimized: false,
        fullscreen: false,
    };

    fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Writes the `settings.json` and `state.json` of an older Lectern into `config`: one library,
    /// `root`, a hidden library, a wider outline, a bigger font, and `doc` open last.
    fn write_v021_profile(config: &Path, root: &Path, doc: &Path) {
        let settings = serde_json::json!({
            "libraryRoots": [path_string(root)],
            "libraryVisible": false,
            "outlineWidth": 300,
            "fontSize": 20,
        });
        fs::write(config.join(SETTINGS_FILE), settings.to_string()).unwrap();
        let state = serde_json::json!({
            "positions": [],
            "recent": [{"path": path_string(doc), "title": "Tide sync", "openedMs": 1}],
            "lastDoc": path_string(doc),
            "window": {"x": 120, "y": 80, "width": 1280, "height": 860, "maximized": false},
        });
        fs::write(config.join(STATE_FILE), state.to_string()).unwrap();
    }

    /// An older Lectern's profile boots as one workspace, "Main", written to `workspaces.json`:
    /// the window has its libraries, layout and place, and reopens its last document.
    #[test]
    fn a_v021_profile_boots_as_one_workspace() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/notes/tide.md", "# Tide sync");
        let config = dir.folder("config");
        write_v021_profile(&config, &root, &doc);
        let profile = load_profile(&config, None);
        let last = profile.last_doc().map(PathBuf::from);
        assert_eq!(last.as_deref(), Some(doc.as_path()));
        let f = fixture_in(dir, profile, FakeHost::default());
        // What boot renders, as `main.rs` does.
        f.early.fill(Early {
            doc: last.map(|path| EarlyDoc {
                outcome: render_file(&path, &PathMapper::default(), &[]),
                path,
                from_args: false,
            }),
            folder: None,
        });
        let payload = f.state.startup();
        assert_eq!(opened_path(payload.initial.as_ref().unwrap()), doc);
        let recent: Vec<&str> = payload.recent.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(recent, [path_string(&doc)]);
        // What `get_settings` answers.
        let settings = f.state.settings();
        assert_eq!(settings.library_roots, [path_string(&root)]);
        assert!(!settings.library_visible);
        assert_eq!(settings.outline_width, 300);
        assert_eq!(settings.font_size, 20);
        assert_eq!(
            f.state.saved_placement(),
            Some(WindowPlacement {
                x: 120,
                y: 80,
                width: 1280,
                height: 860,
                maximized: false,
            })
        );
        f.app.flush();
        let saved: Workspaces = read_json(&f.config.join(WORKSPACES_FILE));
        let [main] = saved.items.as_slice() else {
            panic!("{saved:?}");
        };
        assert_eq!(main.name, "Main");
        assert_eq!(main.roots, [path_string(&root)]);
        assert!(main.open);
        assert_eq!(main.last_doc, Some(path_string(&doc)));
    }

    /// `state.json` keeps the recent files, last document and placement an older Lectern wrote,
    /// and only its reading positions change; the workspace keeps its own.
    #[test]
    fn state_json_keeps_what_an_older_lectern_wrote() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        let doc = dir.file("vault/notes/tide.md", "# Tide sync");
        let other = path_string(&dir.file("vault/notes/seeds.md", "# Seeds"));
        let config = dir.folder("config");
        write_v021_profile(&config, &root, &doc);
        let f = fixture_in(dir, load_profile(&config, None), FakeHost::default());
        opened_path(&f.state.open_document(&other));
        f.state.save_position(
            &other,
            SavedPosition {
                heading_id: None,
                offset: 0.0,
                line: Some(1),
                fraction: 0.5,
            },
        );
        let rect = Rect {
            x: 300,
            y: 200,
            width: 900,
            height: 700,
        };
        f.state.track_placement(NORMAL, Some(rect));
        f.state.remember_placement(false);
        f.app.flush();
        let state: serde_json::Value = read_json(&f.config.join(STATE_FILE));
        assert_eq!(state["lastDoc"], path_string(&doc));
        assert_eq!(state["recent"].as_array().map(Vec::len), Some(1));
        assert_eq!(state["window"]["x"], 120);
        assert_eq!(state["positions"][0][0], other);
        assert_eq!(last_doc(&f), Some(other));
    }

    /// The window's placement goes into its workspace as it closes.
    #[test]
    fn the_window_placement_is_saved_into_its_workspace() {
        let f = fixture(profile(&[]), FakeHost::default());
        let rect = Rect {
            x: 120,
            y: 80,
            width: 1280,
            height: 860,
        };
        f.state.track_placement(NORMAL, Some(rect));
        f.state.remember_placement(true);
        let placement = f.app.read_workspace("w1", |ws| ws.placement).flatten();
        assert_eq!(
            placement,
            Some(WindowPlacement {
                maximized: true,
                ..WindowPlacement::from(rect)
            })
        );
        f.app.flush();
        let saved: Workspaces = read_json(&f.config.join(WORKSPACES_FILE));
        assert_eq!(saved.items[0].placement, placement);
    }

    /// The first window shows the most recently focused open workspace, and only its libraries.
    #[test]
    fn the_first_window_shows_the_most_recently_focused_open_workspace() {
        let dir = TempDir::new();
        let work = dir.folder("work");
        dir.file("work/plan.md", "# Plan");
        let garden = dir.folder("garden");
        let seeds = dir.file("garden/seeds.md", "# Seeds");
        let mut profile = profile(&[&work]);
        let w2 = profile.workspaces.create("Garden");
        let ws = profile.workspaces.get_mut(&w2).unwrap();
        ws.roots = vec![path_string(&garden)];
        ws.open = true;
        profile.workspaces.touch_focus(&w2);
        let f = fixture_in(dir, profile, FakeHost::default());
        assert_eq!(f.state.workspace_id(), Some(w2));
        assert_eq!(f.state.settings().library_roots, [path_string(&garden)]);
        wait_until("the garden is indexed", || settled(&f, &garden));
        let files: Vec<PathBuf> = f
            .state
            .quick_open_candidates()
            .iter()
            .map(|c| PathBuf::from(&c.path))
            .collect();
        assert_eq!(files, [seeds]);
    }

    /// With none open, the first window shows the most recently focused workspace, which is
    /// then open.
    #[test]
    fn with_no_workspace_open_the_first_window_opens_the_latest_focused() {
        let mut profile = profile(&[]);
        profile.workspaces.items[0].open = false;
        let w2 = profile.workspaces.create("Garden");
        profile.workspaces.touch_focus(&w2);
        let f = fixture(profile, FakeHost::default());
        assert_eq!(f.state.workspace_id().as_ref(), Some(&w2));
        f.app.flush();
        let saved: Workspaces = read_json(&f.config.join(WORKSPACES_FILE));
        assert!(saved.get(&w2).unwrap().open);
        assert!(!saved.get("w1").unwrap().open);
    }

    fn hide_library() -> SettingsPatch {
        SettingsPatch {
            library_visible: Some(false),
            ..SettingsPatch::default()
        }
    }

    /// A layout change goes to the window's workspace, and to the mirror in `settings.json` when
    /// that is the first workspace.
    #[test]
    fn a_layout_change_goes_to_the_workspace_and_the_first_ones_mirror() {
        let mut profile = profile(&[]);
        let w2 = profile.workspaces.create("Garden");
        let f = fixture(profile, FakeHost::default());
        assert!(!f.state.set_settings(hide_library()).library_visible);
        f.app.flush();
        let saved: Workspaces = read_json(&f.config.join(WORKSPACES_FILE));
        assert!(!saved.get("w1").unwrap().layout.library_visible);
        assert!(saved.get(&w2).unwrap().layout.library_visible);
        let settings: Settings = read_json(&f.config.join(SETTINGS_FILE));
        assert!(!settings.library_visible);
    }

    /// `settings.json` mirrors the first workspace only, so a later one's layout leaves it be.
    #[test]
    fn a_layout_change_in_a_later_workspace_leaves_the_mirror_alone() {
        let mut profile = profile(&[]);
        let w2 = profile.workspaces.create("Garden");
        profile.workspaces.get_mut(&w2).unwrap().open = true;
        profile.workspaces.touch_focus(&w2);
        let f = fixture(profile, FakeHost::default());
        assert!(!f.state.set_settings(hide_library()).library_visible);
        f.app.flush();
        let saved: Workspaces = read_json(&f.config.join(WORKSPACES_FILE));
        assert!(saved.get("w1").unwrap().layout.library_visible);
        assert!(!saved.get(&w2).unwrap().layout.library_visible);
        assert!(!f.config.join(SETTINGS_FILE).exists());
    }

    /// A setting every window shares goes to `settings.json` alone. (The fixture's
    /// `workspaces.json` counts as saved already, so it would be written only if it changed.)
    #[test]
    fn a_shared_change_leaves_the_workspaces_alone() {
        let f = fixture(profile(&[]), FakeHost::default());
        f.state.set_settings(SettingsPatch {
            font_size: Some(18),
            ..SettingsPatch::default()
        });
        f.app.flush();
        let settings: Settings = read_json(&f.config.join(SETTINGS_FILE));
        assert_eq!(settings.font_size, 18);
        assert!(!f.config.join(WORKSPACES_FILE).exists());
    }

    /// Every workspace's roots, open or closed, are trusted and their images served in any
    /// window, and they stay so when a workspace's roots change.
    #[test]
    fn a_root_in_a_closed_workspace_is_trusted_in_any_window() {
        let mut profile = profile(&[]);
        let garden = profile.workspaces.create("Garden");
        profile.workspaces.get_mut(&garden).unwrap().roots = vec![r"\\nas\share\garden".to_owned()];
        let f = fixture(profile, FakeHost::default());
        let image = r"\\nas\share\garden\img\seed.png";
        let served =
            |f: &Fixture| assets::decide(image, &read(&f.app.trust), &read(&f.app.assets)).is_ok();
        assert!(f.state.trusts(image));
        assert!(served(&f));
        let vault = f.dir.folder("vault");
        let doc = f.dir.file(
            "vault/seeds.md",
            "![seed](\\\\\\\\nas\\share\\garden\\img\\seed.png)\n",
        );
        f.state.add_root(&path_string(&vault)).unwrap();
        assert!(f.state.trusts(image));
        assert!(served(&f));
        let html = match f.state.open_document(&path_string(&doc)) {
            OpenResult::Ok { doc } => doc.html,
            OpenResult::Err { error } => panic!("{}", error.message),
        };
        assert!(!html.contains("img-blocked"), "{html}");
        assert!(html.contains("seed.png"), "{html}");
    }

    /// Trust is the app's: a root on a new network host, added in one window, clears every
    /// window's renders, so a note another window rendered with that host's images blocked shows
    /// them when it is opened again.
    #[test]
    fn a_root_on_a_new_host_clears_every_windows_renders() {
        let dir = TempDir::new();
        let garden = dir.folder("garden");
        let doc = dir.file(
            "garden/seeds.md",
            "![seed](\\\\\\\\nas\\share\\garden\\seed.png)\n",
        );
        let mut profile = profile(&[]);
        let w2 = profile.workspaces.create("Garden");
        profile.workspaces.get_mut(&w2).unwrap().roots = vec![path_string(&garden)];
        let f = fixture_in(dir, profile, FakeHost::default());
        // A second window, whose library is never started: no scan of its own clears its cache.
        f.app
            .add_window("second", Some(w2), Arc::new(OpenQueue::default()), None);
        let second = f.app.window("second").unwrap();
        let html = |window: &Arc<WindowState>| match window.open_document(&path_string(&doc)) {
            OpenResult::Ok { doc } => doc.html,
            OpenResult::Err { error } => panic!("{}", error.message),
        };
        assert!(html(&second).contains("img-blocked"));
        // The first window's workspace gains a root on that host. `sync_roots` is what adding
        // one does before scanning it, which would reach the share.
        let main = f.state.workspace_id().unwrap();
        f.app.change_workspace(&main, |ws| {
            ws.roots.push(r"\\nas\share\garden".to_owned());
            true
        });
        f.state.sync_roots();
        let trusted = html(&second);
        assert!(!trusted.contains("img-blocked"), "{trusted}");
    }

    fn window(label: &str) -> Target {
        Target::Window(label.to_owned())
    }

    /// A window's events go to that window alone: the scan of a root in the first window is
    /// news to it only, and a second window's watcher speaks to the second window only.
    #[test]
    fn each_windows_events_go_to_that_window_alone() {
        let dir = TempDir::new();
        let root = dir.folder("vault");
        dir.file("vault/plan.md", "# Plan");
        let seeds = dir.file("garden/seeds.md", "# Seeds");
        let mut profile = profile(&[&root]);
        let garden = profile.workspaces.create("Garden");
        let f = fixture_in(dir, profile, FakeHost::default());
        f.app
            .add_window("win-1", Some(garden), Arc::new(OpenQueue::default()), None);
        f.state.window_shown();
        wait_until("the root is indexed", || f.host.indexed(&root));
        let scan = f
            .host
            .targets(|e| matches!(e, UiEvent::LibraryUpdated(_) | UiEvent::IndexReady(_)));
        assert!(!scan.is_empty());
        assert!(scan.iter().all(|t| *t == window(MAIN_WINDOW)), "{scan:?}");
        let second = f.app.window("win-1").unwrap();
        second.on_watch_event(WatchEvent::DocChanged(seeds));
        assert_eq!(
            f.host.targets(|e| matches!(e, UiEvent::DocChanged(_))),
            [window("win-1")]
        );
    }

    /// A setting every window shares, changed in any window, reaches every window, each with its
    /// own settings; a change to one workspace reaches only its window.
    #[test]
    fn a_settings_change_reaches_the_windows_whose_settings_it_changes() {
        let mut profile = profile(&[]);
        let garden = profile.workspaces.create("Garden");
        profile
            .workspaces
            .get_mut(&garden)
            .unwrap()
            .layout
            .outline_width = 300;
        let f = fixture(profile, FakeHost::default());
        f.app
            .add_window("win-1", Some(garden), Arc::new(OpenQueue::default()), None);
        let second = f.app.window("win-1").unwrap();
        second.set_settings(SettingsPatch {
            font_size: Some(18),
            ..SettingsPatch::default()
        });
        let told = f.host.settings_changes();
        let sent_to = |label: &str| {
            let sent: Vec<&Settings> = told
                .iter()
                .filter(|(target, _)| *target == window(label))
                .map(|(_, settings)| settings)
                .collect();
            let [settings] = sent.as_slice() else {
                panic!("{label}: {told:?}");
            };
            (*settings).clone()
        };
        assert_eq!(told.len(), 2, "{told:?}");
        let (main, win1) = (sent_to(MAIN_WINDOW), sent_to("win-1"));
        assert_eq!((main.font_size, win1.font_size), (18, 18));
        assert_ne!(main.outline_width, 300);
        assert_eq!(win1.outline_width, 300);
        lock(&f.host.events).clear();
        second.set_settings(hide_library());
        let told = f.host.settings_changes();
        let [(target, settings)] = told.as_slice() else {
            panic!("{told:?}");
        };
        assert_eq!(*target, window("win-1"));
        assert!(!settings.library_visible);
        assert!(f.state.settings().library_visible);
        lock(&f.host.events).clear();
        second.set_settings(SettingsPatch::default());
        assert!(f.host.settings_changes().is_empty());
    }
}
