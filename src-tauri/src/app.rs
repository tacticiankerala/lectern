//! Building and running the Tauri app: plugins, commands, setup, the main window (placement,
//! background and title-bar colours) and single-instance forwarding.

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lectern_core::cli::Args;
use lectern_core::ipc::{OpenRequest, Settings, ThemeId, ThemeMode};
use lectern_core::perf::PerfLog;
use lectern_core::render::highlight::StartupWarmUp;
use lectern_core::workspace::WindowPlacement;
use tauri::window::Color;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, RunEvent, Theme, WebviewWindow, Window,
    WindowEvent,
};

use crate::events::TauriHost;
use crate::state::{
    App, AssetResponse, Boot, Early, OpenQueue, Profile, Slot, Timings, WatchControl, WindowState,
};
use crate::updater::{self, Updates};
use crate::{commands, win};

/// The label of the first window, as in `tauri.conf.json`.
pub const MAIN_WINDOW: &str = "main";

/// How long setup, on the main thread, waits for the boot thread to load the settings.
const PROFILE_WAIT: Duration = Duration::from_secs(3);
/// The part of a window's top edge that must be on a monitor for the window to be reachable.
const TITLE_BAR_HEIGHT: u32 = 32;
const MIN_VISIBLE_WIDTH: u32 = 64;
/// How much of a restored window's width must stay on its monitor.
const MIN_ON_SCREEN: u32 = 100;
const MIN_WIDTH: u32 = 480;
const MIN_HEIGHT: u32 = 320;

/// Where Lectern keeps its files: Tauri's app directories, worked out before Tauri starts so the
/// boot thread can read the settings while WebView2 starts up.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// `%APPDATA%\<identifier>`: `settings.json` and `state.json`.
    pub config: PathBuf,
    /// `%LOCALAPPDATA%\<identifier>\library`: the library snapshots.
    pub snapshots: PathBuf,
    /// `%LOCALAPPDATA%\<identifier>\logs`.
    pub logs: PathBuf,
}

impl Dirs {
    /// The same folders as Tauri's `app_config_dir`, `app_local_data_dir` and `app_log_dir`.
    pub fn for_app(identifier: &str) -> Self {
        let roaming = dirs::config_dir()
            .unwrap_or_else(env::temp_dir)
            .join(identifier);
        let local = dirs::data_local_dir()
            .unwrap_or_else(env::temp_dir)
            .join(identifier);
        Self {
            config: roaming,
            snapshots: local.join("library"),
            logs: local.join("logs"),
        }
    }
}

/// Everything `main` prepares before Tauri builds.
pub struct Launch {
    pub args: Args,
    pub perf: Arc<PerfLog>,
    pub dirs: Dirs,
    pub profile: Arc<Slot<Profile>>,
    pub early: Arc<Slot<Early>>,
    /// Starts the highlighter's warm-up once boot has rendered, or at the first paint.
    pub warm: Arc<StartupWarmUp>,
    /// The boot thread ran. It doesn't when another Lectern was already running, and then this
    /// process only hands its arguments over and exits, unless that Lectern quit meanwhile.
    pub booted: bool,
}

pub fn run(context: tauri::Context, launch: Launch) {
    let opens = Arc::new(OpenQueue::default());
    let forwarded = Arc::clone(&opens);
    let perf = Arc::clone(&launch.perf);
    let app = tauri::Builder::default()
        // First, so a second instance hands over its arguments and exits before doing anything.
        .plugin(tauri_plugin_single_instance::init(move |app, argv, cwd| {
            on_second_launch(app, &forwarded, argv, &cwd);
        }))
        .plugin(tauri_plugin_dialog::init())
        // Endpoint, public key and install mode come from `plugins.updater` in tauri.conf.json.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Local images in documents, at `http://lxasset.localhost/<encoded path>`.
        .register_asynchronous_uri_scheme_protocol("lxasset", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            // The checks are in memory; only an allowed file is read, off the main thread.
            tauri::async_runtime::spawn_blocking(move || {
                let response = match app.try_state::<Arc<App>>() {
                    Some(state) => state.serve_asset(&path),
                    None => AssetResponse::refused(),
                };
                responder.respond(response.into_http());
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::startup,
            commands::open_document,
            commands::open_user_path,
            commands::get_library,
            commands::add_root,
            commands::remove_root,
            commands::retry_root,
            commands::quick_open_candidates,
            commands::search,
            commands::follow,
            commands::reveal_in_explorer,
            commands::open_in_editor,
            commands::load_review,
            commands::review_op,
            commands::get_settings,
            commands::set_settings,
            commands::save_position,
            commands::remove_recent,
            commands::set_chrome_colors,
            commands::list_system_fonts,
            commands::perf_mark,
            commands::show_window,
            commands::check_update,
            commands::install_update,
        ])
        .on_window_event(on_window_event)
        .setup(move |app| setup(app, launch, opens))
        .build(context)
        .expect("error while building Lectern");
    perf.mark("built", None);
    app.run(on_run_event);
}

fn setup(
    app: &mut tauri::App,
    launch: Launch,
    opens: Arc<OpenQueue>,
) -> Result<(), Box<dyn Error>> {
    launch.perf.mark("setup", None);
    if !launch.booted {
        // The Lectern this launch found has quit, so this one is first after all: start the
        // boot work now, off the main thread as usual.
        crate::start_boot(
            launch.args.path.clone(),
            &launch.dirs,
            &launch.perf,
            &launch.profile,
            &launch.early,
            &launch.warm,
        );
    }
    let profile = launch
        .profile
        .take(PROFILE_WAIT)
        .unwrap_or_else(Profile::unloaded);
    if let Ok(config) = app.path().app_config_dir() {
        if config != launch.dirs.config {
            log::warn!(
                "Tauri's config folder {} differs from {}",
                config.display(),
                launch.dirs.config.display()
            );
        }
    }
    let portable = updater::detect_portable(&app.package_info().name);
    app.manage(Updates::new(portable));
    let state = App::new(
        Boot {
            config_dir: launch.dirs.config,
            snapshot_dir: launch.dirs.snapshots,
            perf: launch.perf,
            exit_after_paint: launch.args.exit_after_paint,
            profile,
            early: launch.early,
            warm: launch.warm,
            opens,
            timings: Timings::default(),
            portable,
        },
        Arc::new(TauriHost(app.handle().clone())),
        |weak| {
            Box::new(WatchControl::new(move |event| {
                if let Some(window) = weak.upgrade() {
                    window.on_watch_event(event);
                }
            }))
        },
    );
    app.manage(Arc::clone(&state));
    let main = state.window(MAIN_WINDOW);
    match (app.get_webview_window(MAIN_WINDOW), &main) {
        (Some(window), Some(main)) => prepare_window(&window, main),
        _ => log::error!("the main window is missing"),
    }
    if let Some(main) = main {
        main.start_library();
    }
    Ok(())
}

/// Places the hidden window where its workspace's window was last time, if that is still on a
/// monitor, and gives it the theme's background and title-bar colours before it is shown.
fn prepare_window(window: &WebviewWindow, state: &WindowState) {
    let work_areas: Vec<Rect> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            Rect {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
            }
        })
        .collect();
    let restored = state.saved_placement().and_then(|saved| {
        clamp_to_work_areas(Rect::from(saved), &work_areas).map(|rect| (rect, saved.maximized))
    });
    match restored {
        Some((rect, maximized)) => {
            let _ = window.set_position(PhysicalPosition::new(rect.x, rect.y));
            let _ = window.set_size(PhysicalSize::new(rect.width, rect.height));
            // A hidden window only records this; it shows maximised.
            if maximized {
                let _ = window.maximize();
            }
            state.set_initial_placement(WindowPlacement::from(rect));
        }
        None => {
            if let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) {
                state.set_initial_placement(WindowPlacement::from(Rect {
                    x: pos.x,
                    y: pos.y,
                    width: size.width,
                    height: size.height,
                }));
            }
        }
    }
    let system_dark = matches!(window.theme(), Ok(Theme::Dark));
    let colors = theme_colors(&active_theme(&state.settings(), system_dark));
    if let Err(e) = apply_chrome(window, colors.bg, colors.fg, colors.dark) {
        log::warn!("couldn't theme the window: {e}");
    }
}

/// Sets the window background and the title bar to a theme's colours (`#rrggbb`).
pub fn apply_chrome(window: &WebviewWindow, bg: &str, fg: &str, dark: bool) -> Result<(), String> {
    let caption = win::parse_colorref(bg).ok_or_else(|| format!("{bg} isn't a #rrggbb colour"))?;
    let text = win::parse_colorref(fg).ok_or_else(|| format!("{fg} isn't a #rrggbb colour"))?;
    let (r, g, b) = win::colorref_rgb(caption);
    window
        .set_background_color(Some(Color(r, g, b, 255)))
        .map_err(|e| e.to_string())?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    win::set_title_bar_colors(hwnd, caption, text, dark)
}

fn on_second_launch(app: &AppHandle, opens: &OpenQueue, argv: Vec<String>, cwd: &str) {
    let args = Args::parse(argv);
    if let Some(path) = args.path {
        // A relative path is relative to where the second launch ran.
        let path = if path.is_absolute() {
            path
        } else {
            Path::new(cwd).join(path)
        };
        let request = OpenRequest {
            path: path.to_string_lossy().into_owned(),
            t0_ms: args.perf_t0_ms,
        };
        // Held for startup, or resolved (a folder becomes a library root) off the main thread.
        if let Some(request) = opens.offer(request) {
            if let Some(main) = app
                .try_state::<Arc<App>>()
                .and_then(|state| state.window(MAIN_WINDOW))
            {
                main.forward(request);
            }
        }
    }
    // A window still waiting for its first paint shows itself.
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

fn on_window_event(window: &Window, event: &WindowEvent) {
    let Some(app) = window.try_state::<Arc<App>>() else {
        return;
    };
    let state = app.window(window.label());
    match event {
        WindowEvent::Moved(_) => {
            if let Some(state) = &state {
                state.track_window(window);
            }
        }
        WindowEvent::Resized(_) => {
            if let Some(state) = &state {
                state.track_window(window);
            }
            app.set_background(in_background(window, None));
        }
        WindowEvent::Focused(focused) => {
            if *focused {
                app.window_focused(window.label());
            }
            app.set_background(in_background(window, Some(*focused)));
        }
        WindowEvent::CloseRequested { .. } => {
            if let Some(state) = &state {
                state.remember_window(window);
            }
        }
        _ => {}
    }
}

/// Whether the window is in the background: unfocused or minimised. `focused` is what a focus
/// event just said, which the window may not report yet.
fn in_background(window: &Window, focused: Option<bool>) -> bool {
    let focused = focused.unwrap_or_else(|| window.is_focused().unwrap_or(true));
    !focused || window.is_minimized().unwrap_or(false)
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    let Some(state) = app.try_state::<Arc<App>>() else {
        return;
    };
    match event {
        // `exit` (as after `--exit-after-paint`) skips CloseRequested.
        RunEvent::ExitRequested { .. } => {
            if let (Some(window), Some(main)) = (
                app.get_webview_window(MAIN_WINDOW),
                state.window(MAIN_WINDOW),
            ) {
                main.remember_window(&window.as_ref().window());
            }
        }
        RunEvent::Exit => state.flush(),
        _ => {}
    }
}

/// The placement's normal rect.
impl From<WindowPlacement> for Rect {
    fn from(placement: WindowPlacement) -> Self {
        Self {
            x: placement.x,
            y: placement.y,
            width: placement.width,
            height: placement.height,
        }
    }
}

/// A normal (not maximised) placement at `rect`.
impl From<Rect> for WindowPlacement {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            maximized: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The overlap of two spans `[a, a + a_len)` and `[b, b + b_len)`.
fn overlap(a: i32, a_len: u32, b: i32, b_len: u32) -> u32 {
    let start = i64::from(a.max(b));
    let end = (i64::from(a) + i64::from(a_len)).min(i64::from(b) + i64::from(b_len));
    u32::try_from((end - start).max(0)).unwrap_or(0)
}

/// `saved` made reachable on the current monitors: its title bar must overlap a work area by at
/// least 64 px, else `None` (the monitor it was on is gone). The window is then shrunk to fit
/// that work area (lining up with its left edge when it had to narrow), kept at least 100 px on
/// it horizontally, and moved down if its title bar is above it.
pub fn clamp_to_work_areas(saved: Rect, work_areas: &[Rect]) -> Option<Rect> {
    let title_bar = |area: &Rect| {
        let width = overlap(saved.x, saved.width, area.x, area.width);
        let height = overlap(saved.y, TITLE_BAR_HEIGHT, area.y, area.height);
        (width >= MIN_VISIBLE_WIDTH && height > 0).then_some(width)
    };
    let area = work_areas
        .iter()
        .filter_map(|area| title_bar(area).map(|w| (w, area)))
        .max_by_key(|&(w, _)| w)
        .map(|(_, area)| area)?;
    let width = saved.width.clamp(MIN_WIDTH.min(area.width), area.width);
    let height = saved.height.clamp(MIN_HEIGHT.min(area.height), area.height);
    let x = if width < saved.width { area.x } else { saved.x };
    let on_screen = i64::from(MIN_ON_SCREEN.min(width));
    let leftmost = i64::from(area.x) + on_screen - i64::from(width);
    let rightmost = i64::from(area.x) + i64::from(area.width) - on_screen;
    let x = i64::from(x).clamp(leftmost, rightmost);
    Some(Rect {
        x: i32::try_from(x).unwrap_or(area.x),
        y: saved.y.max(area.y),
        width,
        height,
    })
}

/// A theme's background and text colours, mirroring `ui/styles/themes.css`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    pub bg: &'static str,
    pub fg: &'static str,
    pub dark: bool,
}

pub fn theme_colors(id: &ThemeId) -> ThemeColors {
    let (bg, fg, dark) = match id {
        ThemeId::Paper => ("#f8f5ee", "#2b2a27", false),
        ThemeId::Daylight => ("#ffffff", "#1f2328", false),
        ThemeId::Sepia => ("#f4ecd8", "#433422", false),
        ThemeId::Latte => ("#eff1f5", "#4c4f69", false),
        ThemeId::Graphite => ("#1e1f22", "#d7d8db", true),
        ThemeId::Midnight => ("#000000", "#cfcfd4", true),
        ThemeId::Nord => ("#2e3440", "#e5e9f0", true),
        ThemeId::Mocha => ("#1e1e2e", "#cdd6f4", true),
    };
    ThemeColors { bg, fg, dark }
}

/// The theme in use: the dark one of the pair in dark mode (or when following a dark system).
pub fn active_theme(settings: &Settings, system_dark: bool) -> ThemeId {
    let dark = match settings.theme_mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => system_dark,
    };
    if dark {
        settings.dark_theme.clone()
    } else {
        settings.light_theme.clone()
    }
}

#[cfg(test)]
mod capability_tests {
    /// The UI sets the native title as documents open, so the window must allow it.
    #[test]
    fn the_main_window_may_set_its_title() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let permissions: Vec<&str> = caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str())
            .collect();
        assert_eq!(caps["windows"][0], super::MAIN_WINDOW);
        assert!(
            permissions.contains(&"core:window:allow-set-title"),
            "{permissions:?}"
        );
    }

    /// Ctrl+O and Add folder use the dialog plugin's file and folder pickers.
    #[test]
    fn the_main_window_may_pick_files_and_folders() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        // `dialog:default` grants `allow-open`, among others.
        assert!(caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "dialog:default" || p == "dialog:allow-open"));
    }

    /// The UI updates through Lectern's own `check_update` and `install_update`, which decide
    /// between installing and opening the Releases page; it never reaches the updater directly.
    #[test]
    fn the_ui_has_no_direct_access_to_the_updater() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let permissions = caps["permissions"].as_array().unwrap();
        assert!(
            !permissions.iter().any(|p| p
                .as_str()
                .is_some_and(|p| p.starts_with("updater:") || p.starts_with("process:"))),
            "{permissions:?}"
        );
    }

    /// Focus mode puts the window in full screen.
    #[test]
    fn the_main_window_may_go_full_screen() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert!(caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "core:window:allow-set-fullscreen"));
    }
}

#[cfg(test)]
mod bundle_tests {
    fn config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap()
    }

    /// A per-user installer (no admin rights; `updater::is_portable` relies on its folder) that
    /// offers Lectern as a viewer for Markdown files.
    #[test]
    fn the_installer_is_per_user_and_registers_markdown_files() {
        let bundle = &config()["bundle"];
        assert_eq!(bundle["targets"], serde_json::json!(["nsis"]));
        assert_eq!(bundle["windows"]["nsis"]["installMode"], "currentUser");
        let association = &bundle["fileAssociations"][0];
        // The same list the core scans, links and opens as Markdown.
        assert_eq!(
            association["ext"],
            serde_json::json!(lectern_core::library::MARKDOWN_EXTENSIONS)
        );
        assert_eq!(association["role"], "Viewer");
        assert_eq!(association["mimeType"], "text/markdown");
    }

    /// Lectern's own class (a generic name could be another app's, which either uninstaller would
    /// delete), shown in Explorer as "Markdown document".
    #[test]
    fn the_association_has_a_class_of_its_own() {
        let association = &config()["bundle"]["fileAssociations"][0];
        assert_eq!(association["name"], "Lectern.Markdown");
        assert_eq!(association["description"], "Markdown document");
    }

    /// For every extension, the installer hooks remember the default class to restore (before an
    /// install or uninstall), add Lectern to "Open with", and put everything back after an
    /// uninstall, all under the association's class.
    #[test]
    fn the_installer_hooks_cover_every_associated_extension() {
        let bundle = &config()["bundle"];
        assert_eq!(
            bundle["windows"]["nsis"]["installerHooks"],
            "windows/installer-hooks.nsh"
        );
        let hooks = include_str!("../windows/installer-hooks.nsh");
        let association = &bundle["fileAssociations"][0];
        let class = association["name"].as_str().unwrap();
        for used in [
            format!(r#"OpenWithProgids" "{class}""#),
            format!(r#""{class}_original""#),
            format!(r#""{class}_backup""#),
            format!(r#""={class}""#),
        ] {
            assert!(hooks.contains(&used), "{used}");
        }
        let hook = |name: &str| {
            let start = hooks.find(&format!("!macro {name}\n")).unwrap();
            &hooks[start..start + hooks[start..].find("!macroend").unwrap()]
        };
        for ext in association["ext"].as_array().unwrap() {
            let ext = ext.as_str().unwrap();
            for (name, step) in [
                ("NSIS_HOOK_PREINSTALL", "LECTERN_REMEMBER_DEFAULT"),
                ("NSIS_HOOK_POSTINSTALL", "LECTERN_OPEN_WITH"),
                ("NSIS_HOOK_PREUNINSTALL", "LECTERN_REMEMBER_DEFAULT"),
                ("NSIS_HOOK_POSTUNINSTALL", "LECTERN_RESTORE_DEFAULT"),
            ] {
                assert!(
                    hook(name).contains(&format!(r#"{step} "{ext}""#)),
                    "{name} {step} {ext}"
                );
            }
        }
    }

    /// Updates come signed from the latest GitHub release and install with a progress bar only.
    #[test]
    fn updates_come_signed_from_github_releases() {
        let config = config();
        assert_eq!(config["bundle"]["createUpdaterArtifacts"], true);
        let updater = &config["plugins"]["updater"];
        assert_eq!(
            updater["endpoints"],
            serde_json::json!([
                "https://github.com/tacticiankerala/lectern/releases/latest/download/latest.json"
            ])
        );
        assert!(updater["pubkey"]
            .as_str()
            .is_some_and(|key| key.len() > 100));
        assert_eq!(updater["windows"]["installMode"], "passive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1392,
    };
    const PORTRAIT: Rect = Rect {
        x: 2560,
        y: -400,
        width: 1440,
        height: 2512,
    };

    #[test]
    fn a_placement_on_a_monitor_is_kept() {
        let saved = Rect {
            x: 200,
            y: 100,
            width: 1280,
            height: 860,
        };
        assert_eq!(
            clamp_to_work_areas(saved, &[PRIMARY, PORTRAIT]),
            Some(saved)
        );
        let on_portrait = Rect {
            x: 2700,
            y: -300,
            width: 1000,
            height: 1600,
        };
        assert_eq!(
            clamp_to_work_areas(on_portrait, &[PRIMARY, PORTRAIT]),
            Some(on_portrait)
        );
    }

    #[test]
    fn a_placement_on_a_monitor_that_is_gone_is_dropped() {
        let saved = Rect {
            x: 2700,
            y: 100,
            width: 1000,
            height: 800,
        };
        assert_eq!(clamp_to_work_areas(saved, &[PRIMARY]), None);
        assert_eq!(clamp_to_work_areas(saved, &[]), None);
        let sliver = Rect {
            x: 2540,
            y: 100,
            width: 1000,
            height: 800,
        };
        assert_eq!(clamp_to_work_areas(sliver, &[PRIMARY]), None);
    }

    #[test]
    fn an_oversized_or_too_high_placement_is_pulled_in() {
        let saved = Rect {
            x: 10,
            y: -20,
            width: 4000,
            height: 3000,
        };
        assert_eq!(
            clamp_to_work_areas(saved, &[PRIMARY]),
            Some(Rect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1392
            })
        );
        let tiny = Rect {
            x: 10,
            y: 10,
            width: 100,
            height: 50,
        };
        assert_eq!(
            clamp_to_work_areas(tiny, &[PRIMARY]),
            Some(Rect {
                x: 10,
                y: 10,
                width: MIN_WIDTH,
                height: MIN_HEIGHT
            })
        );
    }

    #[test]
    fn a_shrunk_placement_stays_on_its_monitor() {
        let monitor = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let wide = Rect {
            x: -2000,
            y: 0,
            width: 3000,
            height: 800,
        };
        assert_eq!(
            clamp_to_work_areas(wide, &[monitor]),
            Some(Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 800
            })
        );
        // The title bar shows 70 px, enough to keep it, but not enough on screen: pulled in.
        let edge = Rect {
            x: 1850,
            y: 100,
            width: 800,
            height: 600,
        };
        assert_eq!(
            clamp_to_work_areas(edge, &[monitor]),
            Some(Rect { x: 1820, ..edge })
        );
        let left_edge = Rect {
            x: -730,
            y: 100,
            width: 800,
            height: 600,
        };
        assert_eq!(
            clamp_to_work_areas(left_edge, &[monitor]),
            Some(Rect {
                x: -700,
                ..left_edge
            })
        );
    }

    #[test]
    fn every_theme_has_valid_colours_of_its_mode() {
        let themes = [
            (ThemeId::Paper, false),
            (ThemeId::Daylight, false),
            (ThemeId::Sepia, false),
            (ThemeId::Latte, false),
            (ThemeId::Graphite, true),
            (ThemeId::Midnight, true),
            (ThemeId::Nord, true),
            (ThemeId::Mocha, true),
        ];
        for (id, dark) in themes {
            let colors = theme_colors(&id);
            assert!(win::parse_colorref(colors.bg).is_some(), "{id:?}");
            assert!(win::parse_colorref(colors.fg).is_some(), "{id:?}");
            assert_eq!(colors.dark, dark, "{id:?}");
        }
        assert_eq!(theme_colors(&ThemeId::Graphite).bg, "#1e1f22");
    }

    /// The window and title bar take a theme's colours before the page paints, so they must be
    /// the page's: `--bg` and `--fg` in each theme's block of themes.css.
    #[test]
    fn theme_colours_match_the_stylesheet() {
        let css = include_str!("../../ui/styles/themes.css");
        let themes = [
            ("paper", ThemeId::Paper),
            ("daylight", ThemeId::Daylight),
            ("sepia", ThemeId::Sepia),
            ("latte", ThemeId::Latte),
            ("graphite", ThemeId::Graphite),
            ("midnight", ThemeId::Midnight),
            ("nord", ThemeId::Nord),
            ("mocha", ThemeId::Mocha),
        ];
        for (name, id) in themes {
            let start = css
                .find(&format!("html[data-theme=\"{name}\"]"))
                .unwrap_or_else(|| panic!("themes.css has no {name} block"));
            let block = &css[start..start + css[start..].find('}').unwrap()];
            let value = |prop: &str| {
                let at = block.find(&format!("{prop}:")).unwrap() + prop.len() + 1;
                block[at..at + block[at..].find(';').unwrap()]
                    .trim()
                    .to_string()
            };
            let colors = theme_colors(&id);
            assert_eq!(value("--bg"), colors.bg, "{name} --bg");
            assert_eq!(value("--fg"), colors.fg, "{name} --fg");
            let scheme = if colors.dark { "dark" } else { "light" };
            assert_eq!(value("color-scheme"), scheme, "{name} color-scheme");
        }
    }

    #[test]
    fn the_active_theme_follows_the_mode_and_the_system() {
        let mut settings = Settings::default();
        assert!(matches!(active_theme(&settings, true), ThemeId::Graphite));
        assert!(matches!(active_theme(&settings, false), ThemeId::Paper));
        settings.theme_mode = ThemeMode::Dark;
        settings.dark_theme = ThemeId::Nord;
        assert!(matches!(active_theme(&settings, false), ThemeId::Nord));
        settings.theme_mode = ThemeMode::Light;
        assert!(matches!(active_theme(&settings, true), ThemeId::Paper));
    }
}
