//! Updates from GitHub Releases, and whether this copy of Lectern is installed or portable.
//!
//! An installed copy downloads the signed installer and runs it in passive mode: the updater
//! saves the state and exits Lectern, and the installer starts Lectern again once it is done. A
//! portable copy can't replace itself, so it opens the Releases page instead.
//!
//! **Installed or portable.** The per-user NSIS installer records the folder it installed into as
//! `InstallLocation` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<productName>`,
//! and an update installs into that folder again. So a copy is installed when it runs from that
//! folder. Without the record (registry trouble, or an install it didn't make) the installer's
//! default folder, `%LOCALAPPDATA%\<productName>`, counts instead. Anything else (Downloads, a USB
//! stick, a share) is portable. The installer launches Lectern from `$INSTDIR\lectern.exe` (its
//! shortcuts, the file associations and the restart after an update), the same string it records,
//! so the paths compare as strings, as Windows compares them, with no file system call.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use lectern_core::ipc::UpdateInfo;
use lectern_core::library::path_key;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Error, Update, UpdaterExt};

use crate::app::MAIN_WINDOW;
use crate::state::AppState;
use crate::{shell, win};

/// Where a portable copy sends the user for a new version.
const RELEASES_URL: &str = "https://github.com/tacticiankerala/lectern/releases/latest";
/// The per-user uninstall records; the installer names Lectern's after the product name.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
/// How long a check waits for GitHub.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long downloading the installer may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// A second `install_update` while one is downloading or starting the installer.
const ALREADY_INSTALLING: &str = "An update is already being installed";

/// What the update commands share: whether this copy is portable, the update the last check found
/// (which `install` then installs), and whether an install is under way.
pub struct Updates {
    portable: bool,
    pending: Mutex<Option<Update>>,
    installing: AtomicBool,
}

/// Holds the one install allowed at a time (`Updates::claim_install`), until it drops.
struct InstallClaim<'a>(&'a AtomicBool);

impl Drop for InstallClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Updates {
    pub fn new(portable: bool) -> Self {
        Self {
            portable,
            pending: Mutex::new(None),
            installing: AtomicBool::new(false),
        }
    }

    /// Claims the install, unless one is already under way. A failed install drops the claim; a
    /// successful one ends with Lectern exiting.
    fn claim_install(&self) -> Result<InstallClaim<'_>, String> {
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(ALREADY_INSTALLING.to_owned());
        }
        Ok(InstallClaim(&self.installing))
    }

    fn pending(&self) -> MutexGuard<'_, Option<Update>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether this process is a portable copy of `product` (the product name, which the installer
/// names its record and default folder after). A registry read, no file system call.
pub fn detect_portable(product: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return true;
    };
    let location =
        win::current_user_string(&format!(r"{UNINSTALL_KEY}\{product}"), "InstallLocation");
    let default_dir = dirs::data_local_dir().map(|local| local.join(product));
    is_portable(&exe, location.as_deref(), default_dir.as_deref())
}

/// Asks GitHub Releases for a newer version. A found update is kept for `install`.
pub async fn check(app: &AppHandle, updates: &Updates) -> Result<Option<UpdateInfo>, String> {
    let found = find(app).await?;
    let info = found.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        notes: update.body.clone(),
        portable: updates.portable,
    });
    *updates.pending() = found;
    Ok(info)
}

/// A portable copy opens the Releases page. An installed one downloads the update the last check
/// found (checking again if there is none), verifies its signature and runs the installer, which
/// exits Lectern and starts it again once done. One install at a time.
pub async fn install(app: &AppHandle, updates: &Updates) -> Result<(), String> {
    if updates.portable {
        return shell::open_url(RELEASES_URL);
    }
    let _claim = updates.claim_install()?;
    let pending = updates.pending().take();
    let mut update = match pending {
        Some(update) => update,
        None => find(app)
            .await?
            .ok_or_else(|| "Lectern is up to date".to_owned())?,
    };
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("couldn't download Lectern {}: {e}", update.version);
            let message = describe(&e);
            *updates.pending() = Some(update);
            return Err(message);
        }
    };
    log::info!("installing Lectern {}", update.version);
    let launched = launch_installer(
        || {
            update.install(bytes).map_err(|e| {
                log::warn!(
                    "couldn't start the installer for Lectern {}: {e}",
                    update.version
                );
                describe(&e)
            })
        },
        || show_main_window(app),
    );
    if launched.is_err() {
        // The installer didn't start; a retry installs the same update.
        *updates.pending() = Some(update);
    }
    launched
}

/// The newest release, if it is newer than this one.
async fn find(app: &AppHandle) -> Result<Option<Update>, String> {
    let saver = app.clone();
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .on_before_exit(move || save_before_exit(&saver))
        .build()
        .map_err(|e| describe(&e))?;
    updater.check().await.map_err(|e| {
        log::warn!("couldn't check for updates: {e}");
        describe(&e)
    })
}

/// Runs just before the updater starts the installer, after which it exits Lectern without Tauri's
/// exit events: the window placement and reading state are saved, as on a normal exit. Only saved:
/// unlike the updater's default (`cleanup_before_exit`, which hides the window), nothing here
/// needs undoing if the installer then fails to start.
fn save_before_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            state.remember_window(&window.as_ref().window());
        }
        state.flush();
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(e) = window.show() {
            log::warn!("couldn't show the window again: {e}");
        }
    }
}

/// Starts the installer with `launch`, which exits Lectern once the installer runs. If it can't
/// start, Lectern carries on: the window is shown, in case anything hid it, and the error goes
/// back to the UI.
fn launch_installer(
    launch: impl FnOnce() -> Result<(), String>,
    show_window: impl FnOnce(),
) -> Result<(), String> {
    launch().inspect_err(|_| show_window())
}

/// An updater error for the user.
fn describe(error: &Error) -> String {
    match error {
        // GitHub answers 404 until a release with update information is published.
        Error::ReleaseNotFound => "GitHub has no update information for Lectern".to_owned(),
        Error::Reqwest(e) if e.is_timeout() => "GitHub didn't answer in time".to_owned(),
        Error::Reqwest(e) if e.is_connect() => "couldn't reach GitHub".to_owned(),
        other => other.to_string(),
    }
}

/// Whether the copy at `exe` is portable: not in the folder the installer recorded
/// (`install_location`, as the registry holds it, quotes and all), or, without a record, not in
/// `default_dir`, the installer's default folder.
pub fn is_portable(exe: &Path, install_location: Option<&str>, default_dir: Option<&Path>) -> bool {
    let Some(folder) = exe.parent() else {
        return true;
    };
    let recorded = install_location
        .map(|location| location.trim().trim_matches('"').trim())
        .filter(|location| !location.is_empty())
        .map(dir_key);
    let installed = match recorded {
        Some(location) => location,
        None => match default_dir {
            Some(dir) => dir_key(&dir.to_string_lossy()),
            None => return true,
        },
    };
    dir_key(&folder.to_string_lossy()) != installed
}

/// A folder as a comparison key: without a verbatim prefix (`\\?\`, `\\?\UNC\`), lowercase,
/// `/`-separated and without a trailing separator.
fn dir_key(path: &str) -> String {
    let path = match (path.strip_prefix(r"\\?\UNC\"), path.strip_prefix(r"\\?\")) {
        (Some(share), _) => format!(r"\\{share}"),
        (None, Some(local)) => local.to_owned(),
        (None, None) => path.to_owned(),
    };
    path_key(Path::new(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn only_one_install_runs_at_a_time() {
        let updates = Updates::new(false);
        let first = updates.claim_install().unwrap();
        assert_eq!(
            updates.claim_install().err().as_deref(),
            Some("An update is already being installed")
        );
        drop(first);
        assert!(updates.claim_install().is_ok());
    }

    #[test]
    fn an_installer_that_fails_to_start_leaves_lectern_showing() {
        let updates = Updates::new(false);
        let shown = Cell::new(0);
        let result = {
            let _claim = updates.claim_install().unwrap();
            launch_installer(
                || Err("Windows denied access".to_owned()),
                || shown.set(shown.get() + 1),
            )
        };
        assert_eq!(result, Err("Windows denied access".to_owned()));
        assert_eq!(shown.get(), 1, "the window is shown again");
        assert!(updates.claim_install().is_ok(), "a retry may install");
    }

    #[test]
    fn an_installer_that_starts_leaves_the_window_alone() {
        let shown = Cell::new(false);
        assert_eq!(launch_installer(|| Ok(()), || shown.set(true)), Ok(()));
        assert!(!shown.get());
    }

    const LOCAL: &str = r"C:\Users\me\AppData\Local\Lectern";
    /// `InstallLocation` as the installer writes it: in quotes.
    const RECORDED: &str = r#""C:\Users\me\AppData\Local\Lectern""#;

    fn portable(exe: &str, location: Option<&str>, default_dir: Option<&str>) -> bool {
        is_portable(Path::new(exe), location, default_dir.map(Path::new))
    }

    #[test]
    fn a_copy_in_the_recorded_install_folder_is_installed() {
        let exe = r"C:\Users\me\AppData\Local\Lectern\lectern.exe";
        assert!(!portable(exe, Some(RECORDED), Some(LOCAL)));
        assert!(!portable(exe, Some(LOCAL), None));
        let elsewhere = r"D:\Apps\Lectern\lectern.exe";
        assert!(!portable(
            elsewhere,
            Some(r#""D:\Apps\Lectern""#),
            Some(LOCAL)
        ));
    }

    #[test]
    fn a_copy_in_downloads_is_portable() {
        let exe = r"C:\Users\me\Downloads\lectern.exe";
        assert!(portable(exe, Some(RECORDED), Some(LOCAL)));
        assert!(portable(exe, None, Some(LOCAL)));
        assert!(portable(exe, None, None));
    }

    #[test]
    fn paths_compare_case_insensitively_with_either_separator() {
        assert!(!portable(
            r"c:\Users\me\appdata\local\lectern\Lectern.EXE",
            Some(r"C:\Users\me\AppData\Local\Lectern\"),
            None,
        ));
        assert!(!portable(
            "C:/Users/me/AppData/Local/Lectern/lectern.exe",
            Some(RECORDED),
            None,
        ));
        assert!(!portable(
            r"C:\Users\me\APPDATA\LOCAL\LECTERN\lectern.exe",
            None,
            Some(LOCAL),
        ));
    }

    #[test]
    fn a_copy_on_a_share_is_portable_unless_installed_there() {
        let exe = r"\\nas\Shared\Tools\lectern.exe";
        assert!(portable(exe, Some(RECORDED), Some(LOCAL)));
        assert!(portable(exe, None, Some(LOCAL)));
        assert!(!portable(
            exe,
            Some(r"\\nas\shared\tools"),
            Some(LOCAL)
        ));
        assert!(!portable(
            r"\\?\UNC\nas\Shared\Tools\lectern.exe",
            Some(r"\\nas\Shared\Tools"),
            None,
        ));
    }

    #[test]
    fn a_verbatim_exe_path_matches_its_plain_folder() {
        assert!(!portable(
            r"\\?\C:\Users\me\AppData\Local\Lectern\lectern.exe",
            Some(RECORDED),
            None,
        ));
        assert!(!portable(
            r"\\?\C:\Users\me\AppData\Local\Lectern\lectern.exe",
            None,
            Some(LOCAL),
        ));
    }

    #[test]
    fn the_install_record_wins_over_the_default_folder() {
        let in_default = r"C:\Users\me\AppData\Local\Lectern\lectern.exe";
        // Installed elsewhere: an update would go there, not replace this copy.
        assert!(portable(
            in_default,
            Some(r#""D:\Apps\Lectern""#),
            Some(LOCAL)
        ));
    }

    #[test]
    fn an_empty_install_record_falls_back_to_the_default_folder() {
        let exe = r"C:\Users\me\AppData\Local\Lectern\lectern.exe";
        assert!(!portable(exe, Some(""), Some(LOCAL)));
        assert!(!portable(exe, Some(r#""""#), Some(LOCAL)));
        assert!(!portable(exe, Some("  "), Some(LOCAL)));
        assert!(portable(r"C:\Temp\lectern.exe", Some(""), Some(LOCAL)));
    }

    #[test]
    fn only_the_install_folder_itself_counts() {
        assert!(portable(
            r"C:\Users\me\AppData\Local\Lectern2\lectern.exe",
            Some(RECORDED),
            Some(LOCAL),
        ));
        assert!(portable(
            r"C:\Users\me\AppData\Local\Lectern\old\lectern.exe",
            Some(RECORDED),
            Some(LOCAL),
        ));
        assert!(portable(
            r"C:\Users\me\AppData\Local\lectern.exe",
            Some(RECORDED),
            Some(LOCAL),
        ));
    }
}
