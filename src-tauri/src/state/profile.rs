//! Settings and reading state as loaded at boot (`settings.json`, `state.json`), and the path
//! mapper built from the settings.

use std::path::{Path, PathBuf};

use lectern_core::ipc::Settings;
use lectern_core::library::pathmap::PathMapper;
use lectern_core::store::{load_json_or_default, Loaded, State};
use lectern_core::workspace::WindowPlacement;
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";

pub const STATE_FILE: &str = "state.json";

/// Shown when setup gave up waiting for the settings.
pub(super) const UNLOADED_NOTICE: &str =
    "Couldn't load your settings in time; changes this session won't be saved.";

/// `state.json`: the core reading state plus the window placement.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct StateFile {
    #[serde(flatten)]
    pub reading: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPlacement>,
}

/// Settings and state as loaded at boot, before the app exists.
pub struct Profile {
    pub settings: Settings,
    pub state: StateFile,
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
        Self {
            settings: Settings::default(),
            state: StateFile::default(),
            notice: Some(UNLOADED_NOTICE.to_owned()),
            wsl_distro: None,
            persist: false,
        }
    }
}

/// Reads `settings.json` and `state.json` from `config_dir`. A corrupt file is backed up and
/// replaced with defaults, and a notice says so.
pub fn load_profile(config_dir: &Path, wsl_distro: Option<String>) -> Profile {
    let mut notices = Vec::new();
    let mut settings: Settings = loaded(
        load_json_or_default(&config_dir.join(SETTINGS_FILE)),
        "settings",
        &mut notices,
    );
    settings.clamp();
    let state = loaded(
        load_json_or_default(&config_dir.join(STATE_FILE)),
        "reading positions and recent files",
        &mut notices,
    );
    Profile {
        settings,
        state,
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
}
