//! What the commands work on: settings, reading state, the library and its index, the render
//! cache and the watcher. Each command is one call into a method of `AppState`, which is spread
//! over the submodules by concern.
//!
//! Methods that touch the file system block, and the commands run them on Tauri's blocking pool.
//! Library roots may sit on a NAS that stalls for seconds, so roots are probed with a timeout and
//! scanned on threads of their own, and the watcher is driven from a thread of its own too.
//!
//! Locks are taken in this order, and never held across file system calls or emits: `settings`,
//! `trust`, `assets`, `state`, `library`, `cache`, `current`.

mod assets;
mod doc;
mod follow;
mod grammars;
mod library;
mod open;
mod open_queue;
mod paths;
mod profile;
mod saver;
mod scan;
mod startup;
mod sync;
#[cfg(test)]
mod test_support;
mod trust;
mod watch_control;

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
use tauri::Window;

pub use self::assets::AssetResponse;
pub use self::doc::{render_file, Early, EarlyDoc};
pub use self::open_queue::OpenQueue;
pub use self::profile::{load_profile, mapper_for, Profile};
pub use self::sync::Slot;
pub use self::trust::Trust;
pub use self::watch_control::{Watch, WatchControl};

use self::assets::AssetScope;
use self::grammars::AppGrammars;
use self::library::Library;
use self::paths::same_path;
use self::profile::StateFile;
use self::saver::Saver;
use self::startup::spawn_forwarder;
use self::sync::{lock, read, write, Gate};
use crate::app::{Rect, WindowPlacement};
use crate::events::{Host, UiEvent};

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
    /// The compiled grammars are released once the window has been in the background this long.
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

/// Everything AppState needs from boot.
pub struct Boot {
    pub config_dir: PathBuf,
    pub snapshot_dir: PathBuf,
    pub perf: Arc<PerfLog>,
    pub exit_after_paint: bool,
    pub profile: Profile,
    pub early: Arc<Slot<Early>>,
    /// Starts the highlighter's warm-up; the first paint may be what it waits for.
    pub warm: Arc<StartupWarmUp>,
    pub opens: Arc<OpenQueue>,
    pub timings: Timings,
    /// Not running from the folder Lectern was installed in (`updater::detect_portable`).
    pub portable: bool,
}

pub struct AppState {
    host: Arc<dyn Host>,
    snapshot_dir: PathBuf,
    perf: Arc<PerfLog>,
    exit_after_paint: bool,
    timings: Timings,
    portable: bool,
    settings: RwLock<Settings>,
    /// The network hosts Lectern may reach.
    trust: RwLock<Trust>,
    /// The folders the image protocol may serve from.
    assets: RwLock<AssetScope>,
    state: Mutex<StateFile>,
    notice: Mutex<Option<String>>,
    wsl_distro: Option<String>,
    mapper: RwLock<Arc<PathMapper>>,
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
    ui_shown: Gate,
    early: Arc<Slot<Early>>,
    warm: Arc<StartupWarmUp>,
    /// Releases the compiled grammars while the window is in the background.
    background: BackgroundRelease,
    opens: Arc<OpenQueue>,
    /// Second launches after startup, resolved one at a time on a thread of their own.
    forwards: Sender<OpenRequest>,
    watch: Box<dyn Watch>,
    saver: Saver,
    /// The window's last normal (not maximised, minimised or full screen) placement.
    window: Mutex<Option<WindowPlacement>>,
}

impl AppState {
    /// The app state. `watch` builds the watch worker, which reports back through the state.
    pub fn new(
        boot: Boot,
        host: Arc<dyn Host>,
        watch: impl FnOnce(Weak<AppState>) -> Box<dyn Watch>,
    ) -> Arc<Self> {
        let Profile {
            settings,
            state,
            notice,
            wsl_distro,
            persist,
        } = boot.profile;
        if !persist {
            log::error!("the settings didn't load in time; nothing will be saved this session");
        }
        let mut library = Library::default();
        for root in &settings.library_roots {
            let root = PathBuf::from(root);
            if !library.roots.iter().any(|r| same_path(&r.path, &root)) {
                library.push_root(root, false);
            }
        }
        Arc::new_cyclic(|weak: &Weak<AppState>| Self {
            forwards: spawn_forwarder(weak.clone()),
            watch: watch(weak.clone()),
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
            trust: RwLock::new(Trust::new(&settings)),
            // Every configured root from the start, so a document's images load at once.
            assets: RwLock::new({
                let mut scope = AssetScope::default();
                scope.set_roots(&settings.library_roots);
                scope
            }),
            settings: RwLock::new(settings),
            window: Mutex::new(state.window),
            state: Mutex::new(state),
            notice: Mutex::new(notice),
            wsl_distro,
            library: Mutex::new(library),
            index_gen: AtomicU64::new(0),
            cache: Mutex::new(RenderCache::new(CACHE_CAP)),
            content: ContentCache::new(),
            current: Mutex::new(None),
            open_seq: AtomicU64::new(0),
            snapshots_loaded: Gate::default(),
            ui_shown: Gate::default(),
            early: boot.early,
            warm: boot.warm,
            opens: boot.opens,
        })
    }

    pub fn settings(&self) -> Settings {
        read(&self.settings).clone()
    }

    fn index(&self) -> Arc<LibraryIndex> {
        Arc::clone(&lock(&self.library).index)
    }

    fn mapper(&self) -> Arc<PathMapper> {
        Arc::clone(&read(&self.mapper))
    }

    fn next_seq(&self) -> u64 {
        self.open_seq.fetch_add(1, Ordering::SeqCst) + 1
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

    /// Re-reads the trusted hosts from the settings; true when they changed.
    fn reconfigure_trust(&self) -> bool {
        let settings = self.settings();
        write(&self.trust).configure(&settings)
    }

    /// The window is showing the first document: scans may start, and that document is
    /// re-rendered if the index can now do better.
    pub fn window_shown(&self) {
        self.perf.mark("window-shown", None);
        self.ui_shown.open();
        self.refresh_if_stale(None);
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

    pub fn set_settings(self: &Arc<Self>, patch: SettingsPatch) -> Settings {
        let roots_changed = patch.library_roots.is_some();
        let mappings_changed = patch.path_mappings.is_some();
        let settings = {
            let mut settings = write(&self.settings);
            settings.apply(patch);
            self.saver.settings(&settings);
            settings.clone()
        };
        if mappings_changed {
            *write(&self.mapper) = Arc::new(mapper_for(&settings, self.wsl_distro.clone()));
            self.reconfigure_trust();
            self.index_changed(None);
            self.refresh_current_now();
        }
        if roots_changed {
            for (root, gen) in self.sync_roots() {
                self.start_root(&root, gen, false);
            }
            self.watch_user_roots();
        }
        settings
    }

    /// Reports a watcher event: document changes go to the UI, library changes rescan the root.
    /// Runs on the watcher's thread, so it never calls back into the watcher.
    pub fn on_watch_event(self: &Arc<Self>, event: WatchEvent) {
        match event {
            WatchEvent::DocChanged(path) => self.host.emit(UiEvent::DocChanged(path)),
            WatchEvent::DocRemoved(path) => self.host.emit(UiEvent::DocRemoved(path)),
            WatchEvent::LibraryChanged(root) => self.request_scan(&root, None),
        }
    }

    pub fn saved_placement(&self) -> Option<WindowPlacement> {
        *lock(&self.window)
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
            *lock(&self.window) = Some(WindowPlacement::from_rect(rect));
        }
    }

    /// Puts the window placement into `state.json`, as the window closes.
    pub fn remember_window(&self, window: &Window) {
        // A window already destroyed has nothing to tell.
        let Ok(maximized) = window.is_maximized() else {
            return;
        };
        self.track_window(window);
        let Some(placement) = *lock(&self.window) else {
            return;
        };
        let mut state = lock(&self.state);
        state.window = Some(WindowPlacement {
            maximized,
            ..placement
        });
        self.saver.state(&state);
    }

    pub fn set_initial_placement(&self, placement: WindowPlacement) {
        *lock(&self.window) = Some(placement);
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

impl AppState {
    /// Writes pending settings and state now; called as the app exits.
    pub fn flush(&self) {
        self.saver.flush(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::*;
    use lectern_core::ipc::OpenResult;
    use lectern_core::library::pathmap::asset_url;
    use std::{fs, thread};

    use crate::state::paths::path_string;
    use std::time::Duration;

    use lectern_core::ipc::SavedPosition;

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
        assert_eq!(
            f.state.saved_placement(),
            Some(WindowPlacement::from_rect(rect))
        );
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
                Some(WindowPlacement::from_rect(rect)),
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
        f.state.flush();
        thread::sleep(SAVE_DEBOUNCE * 2);
        assert!(!f.config.join(SETTINGS_FILE).exists());
        assert!(!f.config.join(STATE_FILE).exists());
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
        f.state.flush();
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
        let response = f.state.serve_asset(&encoded(&logo));
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("image/png"));
        assert_eq!(response.body, b"png bytes");
        assert_eq!(f.state.serve_asset(&encoded(&outside)).status, 403);
        assert_eq!(
            f.state
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
        let response = f.state.serve_asset(&asset_url("", logo));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"png bytes");
    }

    #[test]
    fn an_opened_documents_folder_comes_into_scope() {
        let f = fixture(profile(&[]), FakeHost::default());
        let doc = f.dir.file("trip/notes.md", "# Trip");
        let photo = f.dir.file("trip/photos/day1.jpg", "jpg bytes");
        let path = asset_url("", &photo);
        assert_eq!(f.state.serve_asset(&path).status, 403);
        f.state.open_document(&path_string(&doc));
        assert_eq!(f.state.serve_asset(&path).status, 200);
        // A failed open adds nothing.
        let other = f.dir.file("other/pic.png", "png");
        f.state
            .open_document(&path_string(&f.dir.0.join("other").join("missing.md")));
        assert_eq!(f.state.serve_asset(&asset_url("", &other)).status, 403);
    }
}
