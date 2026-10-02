//! Lectern's entry point. Before Tauri builds the WebView, a boot thread loads the settings and
//! reads and renders the document to open, so that work overlaps WebView2's start-up. Once that
//! render is done (or, without a document, once the window first paints) the highlighter warms
//! the grammars the vault uses most, at low priority. A second launch skips all of it: the
//! single-instance plugin hands its arguments to the running Lectern and exits.
//!
//! Lectern targets Windows only.

// Release builds run without a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod events;
mod logging;
mod shell;
mod state;
mod updater;
mod win;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lectern_core::cli::Args;
use lectern_core::perf::PerfLog;
use lectern_core::render::highlight::StartupWarmUp;

use crate::app::{Dirs, Launch};
use crate::state::{Early, EarlyDoc, Profile, Slot, Trust};

fn main() {
    let args = Args::parse(std::env::args());
    let started_ms = win::process_start_unix_ms().unwrap_or_else(unix_now_ms);
    let perf = Arc::new(PerfLog::new(args.perf_log.clone(), started_ms));
    perf.mark("main", None);

    let context = tauri::generate_context!();
    let dirs = Dirs::for_app(&context.config().identifier);
    let profile = Arc::new(Slot::default());
    let early = Arc::new(Slot::default());
    let warm = Arc::new(StartupWarmUp::default());
    // A second launch must reach the single-instance hand-over fast, and must not race the
    // running Lectern on the log, the settings or a render.
    let booted = !win::another_instance_is_running(&context.config().identifier);
    if booted {
        start_boot(args.path.clone(), &dirs, &perf, &profile, &early, &warm);
    }

    app::run(
        context,
        Launch {
            args,
            perf,
            dirs,
            profile,
            early,
            warm,
            booted,
        },
    );
}

/// Starts the boot thread. `main` does this unless another Lectern is running; setup does it when
/// that Lectern quit before handing over.
pub(crate) fn start_boot(
    arg: Option<PathBuf>,
    dirs: &Dirs,
    perf: &Arc<PerfLog>,
    profile: &Arc<Slot<Profile>>,
    early: &Arc<Slot<Early>>,
    warm: &Arc<StartupWarmUp>,
) {
    let (dirs, perf, profile, early, warm) = (
        dirs.clone(),
        Arc::clone(perf),
        Arc::clone(profile),
        Arc::clone(early),
        Arc::clone(warm),
    );
    let doc = arg.map(|path| std::path::absolute(&path).unwrap_or(path));
    thread::Builder::new()
        .name("lectern-boot".to_owned())
        .spawn(move || {
            let rendered = boot(&dirs, doc, &perf, &profile, &early);
            // Only now: the warm-up must never compete with the boot render.
            warm.boot_finished(rendered);
        })
        .expect("couldn't start the boot thread");
}

/// Starts logging, loads the settings for setup, then reads and renders the document to open
/// with the user's path mappings and no index yet: the one given on the command line (a folder's
/// README for a folder), else the last one open, unless that is on a network host the user no
/// longer trusts. True when it had a document to render, whether or not reading it worked.
fn boot(
    dirs: &Dirs,
    arg: Option<PathBuf>,
    perf: &PerfLog,
    profile_slot: &Slot<Profile>,
    early_slot: &Slot<Early>,
) -> bool {
    logging::init_logging(&dirs.logs);
    let profile = state::load_profile(&dirs.config, win::wsl_default_distro());
    let mapper = state::mapper_for(&profile.settings, profile.wsl_distro.clone());
    let mut trust = Trust::new(&profile.settings);
    let last_doc = profile
        .state
        .reading
        .last_doc
        .clone()
        .filter(|doc| trust.allows(doc))
        .map(PathBuf::from);
    profile_slot.fill(profile);
    // The user chose the argument, so its network host is theirs.
    if let Some(path) = &arg {
        trust.opened_by_user(path);
    }
    let hosts = trust.hosts();
    let mut early = Early::default();
    let doc = match arg {
        Some(path) if path.is_dir() => {
            let readme = path.join("README.md");
            early.folder = Some(path);
            readme.is_file().then_some((readme, true))
        }
        Some(path) => Some((path, true)),
        None => last_doc.map(|path| (path, false)),
    };
    early.doc = doc.map(|(path, from_args)| {
        let started = Instant::now();
        let outcome = state::render_file(&path, &mapper, &hosts);
        perf.mark(
            "render-done",
            Some(started.elapsed().as_secs_f64() * 1000.0),
        );
        EarlyDoc {
            path,
            from_args,
            outcome,
        }
    });
    let rendered = early.doc.is_some();
    early_slot.fill(early);
    rendered
}

fn unix_now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
