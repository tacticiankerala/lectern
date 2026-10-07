//! Settings, workspaces and reading state as loaded at boot (`settings.json`, `workspaces.json`,
//! `state.json`), the workspace the first window shows, and the path mapper and trusted hosts
//! built from them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lectern_core::ipc::Settings;
use lectern_core::library::path_key;
use lectern_core::library::pathmap::PathMapper;
use lectern_core::store::{load_json_or_default, Loaded, State};
use lectern_core::workspace::{
    load_workspaces, WindowPlacement, Workspace, Workspaces, WORKSPACES_FILE,
};
use serde::{Deserialize, Serialize};

use super::trust::Trust;

pub const SETTINGS_FILE: &str = "settings.json";

pub const STATE_FILE: &str = "state.json";

/// Shown when setup gave up waiting for the settings.
pub(super) const UNLOADED_NOTICE: &str =
    "Couldn't load your settings in time; changes this session won't be saved.";

/// `state.json`: the core reading state plus the window placement. Only the positions change now;
/// the recent files, last document and placement stay as an older Lectern last wrote them, and the
/// workspaces keep their own.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct StateFile {
    #[serde(flatten)]
    pub reading: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPlacement>,
}

/// Settings, workspaces and state as loaded at boot, before the app exists.
pub struct Profile {
    pub settings: Settings,
    pub state: StateFile,
    pub workspaces: Workspaces,
    /// The workspaces were made from the other two files, so `workspaces.json` doesn't hold them
    /// yet: it was missing, or unreadable and kept aside.
    pub migrated: bool,
    /// Shown once in the UI, when a file had to be reset.
    pub notice: Option<String>,
    pub wsl_distro: Option<String>,
    /// False when the files couldn't be loaded in time: nothing is saved this session, so the
    /// defaults never replace the user's real settings.
    pub persist: bool,
}

impl Profile {
    /// The defaults, used when the settings didn't load in time.
    pub fn unloaded() -> Self {
        let settings = Settings::default();
        Self {
            workspaces: Workspaces::migrate(&settings, None, Vec::new(), None),
            migrated: true,
            settings,
            state: StateFile::default(),
            notice: Some(UNLOADED_NOTICE.to_owned()),
            wsl_distro: None,
            persist: false,
        }
    }

    /// The document the first window reopens: the last one open in its workspace.
    pub fn last_doc(&self) -> Option<String> {
        first_workspace(&self.workspaces).and_then(|ws| ws.last_doc.clone())
    }

    /// The network hosts Lectern may reach: those of every workspace's roots and of the path
    /// mappings, and WSL's.
    pub fn trust(&self) -> Trust {
        Trust::new(&all_roots(&self.workspaces), &self.settings.path_mappings)
    }
}

/// Reads `settings.json`, `state.json` and `workspaces.json` from `config_dir`. A corrupt file is
/// backed up and replaced with defaults, and a notice says so. Missing or corrupt workspaces are
/// made from the other two files.
pub fn load_profile(config_dir: &Path, wsl_distro: Option<String>) -> Profile {
    let mut notices = Vec::new();
    let mut settings: Settings = loaded(
        load_json_or_default(&config_dir.join(SETTINGS_FILE)),
        "settings",
        &mut notices,
    );
    settings.clamp();
    let state: StateFile = loaded(
        load_json_or_default(&config_dir.join(STATE_FILE)),
        "reading positions and recent files",
        &mut notices,
    );
    let (workspaces, notice) = load_workspaces(
        config_dir,
        &settings,
        state.reading.last_doc.clone(),
        state.reading.recent.clone(),
        state.window,
    );
    notices.extend(notice);
    Profile {
        settings,
        state,
        workspaces,
        migrated: !config_dir.join(WORKSPACES_FILE).is_file(),
        notice: (!notices.is_empty()).then(|| notices.join(" ")),
        wsl_distro,
        persist: true,
    }
}

pub(super) fn loaded<T>(loaded: Loaded<T>, what: &str, notices: &mut Vec<String>) -> T {
    match loaded {
        Loaded::Fresh(value) | Loaded::Ok(value) => value,
        Loaded::RecoveredFromCorrupt { value, backup } => {
            log::warn!("{what} were unreadable; kept as {}", backup.display());
            notices.push(format!(
                "Lectern couldn't read its {what}, so they were reset. The old file was kept as {}.",
                backup.display()
            ));
            value
        }
    }
}

/// The workspace the first window shows: the most recently focused open one, else the most
/// recently focused one. `None` only when there are no workspaces.
pub(super) fn first_workspace(workspaces: &Workspaces) -> Option<&Workspace> {
    let focused = || workspaces.focus.iter().filter_map(|id| workspaces.get(id));
    focused().find(|ws| ws.open).or_else(|| focused().next())
}

/// Every workspace's roots, open or closed, each once (compared as Windows compares paths).
pub(super) fn all_roots(workspaces: &Workspaces) -> Vec<String> {
    let mut seen = HashSet::new();
    workspaces
        .items
        .iter()
        .flat_map(|ws| &ws.roots)
        .filter(|root| seen.insert(path_key(Path::new(root))))
        .cloned()
        .collect()
}

/// The path mapper for `settings`: its prefix mappings, plus the WSL fallback.
pub fn mapper_for(settings: &Settings, wsl_distro: Option<String>) -> PathMapper {
    PathMapper {
        mappings: settings
            .path_mappings
            .iter()
            .map(|m| (m.from.clone(), PathBuf::from(&m.to)))
            .collect(),
        wsl_distro,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_json_keeps_the_core_fields_beside_the_window() {
        let json = r#"{"recent":[],"lastDoc":"C:\\a.md","positions":[],"window":{"x":-8,"y":0,"width":1280,"height":860,"maximized":true}}"#;
        let state: StateFile = serde_json::from_str(json).unwrap();
        assert_eq!(state.reading.last_doc.as_deref(), Some(r"C:\a.md"));
        assert!(state.window.is_some_and(|w| w.maximized && w.x == -8));
        let back = serde_json::to_value(&state).unwrap();
        assert_eq!(back["lastDoc"], r"C:\a.md");
        assert_eq!(back["window"]["width"], 1280);
        let old: StateFile = serde_json::from_str(r#"{"lastDoc":null}"#).unwrap();
        assert!(old.window.is_none());
    }

    fn first(workspaces: &Workspaces) -> Option<&str> {
        first_workspace(workspaces).map(|ws| ws.id.as_str())
    }

    #[test]
    fn the_first_window_shows_the_latest_focused_open_workspace_else_the_latest_focused() {
        let mut workspaces = Workspaces::migrate(&Settings::default(), None, Vec::new(), None);
        let garden = workspaces.create("Garden");
        let work = workspaces.create("Work");
        workspaces.touch_focus(&work);
        // Only "Main" is open, though "Work" was focused since.
        assert_eq!(first(&workspaces), Some("w1"));
        workspaces.get_mut(&garden).unwrap().open = true;
        workspaces.touch_focus(&garden);
        assert_eq!(first(&workspaces), Some(garden.as_str()));
        for ws in &mut workspaces.items {
            ws.open = false;
        }
        assert_eq!(first(&workspaces), Some(garden.as_str()));
        assert_eq!(first(&Workspaces::default()), None);
    }

    #[test]
    fn every_workspaces_roots_count_once() {
        let mut workspaces = Workspaces::migrate(
            &Settings {
                library_roots: vec![r"S:\Notes\My Vault".to_owned()],
                ..Settings::default()
            },
            None,
            Vec::new(),
            None,
        );
        let garden = workspaces.create("Garden");
        workspaces.get_mut(&garden).unwrap().roots =
            vec![r"\\nas\share".to_owned(), r"s:\notes\my vault\".to_owned()];
        assert_eq!(
            all_roots(&workspaces),
            [r"S:\Notes\My Vault".to_owned(), r"\\nas\share".to_owned()]
        );
    }
}
