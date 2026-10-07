//! Workspaces. Each Lectern window shows one, with its own name, libraries, theme, layout, last
//! note, recent notes and place on screen. They live in `workspaces.json`, while `settings.json`
//! keeps the settings every window shares and mirrors the first workspace's libraries and layout,
//! so an older Lectern still opens it. Also here: which settings a window sees, where a settings
//! change goes, and which window an Explorer or command-line open goes to.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ipc::{RecentEntry, Settings, SettingsPatch, ThemeId, ThemeMode};
use crate::library::{key_under, path_key};
use crate::store::{load_json_or_default, Loaded};

/// The workspaces file, beside `settings.json`.
pub const WORKSPACES_FILE: &str = "workspaces.json";

const VERSION: u32 = 1;
/// The name of the workspace an older Lectern's profile becomes.
const MIGRATED_NAME: &str = "Main";
const MAX_NAME_CHARS: usize = 60;

const LAST_WORKSPACE: &str = "Lectern needs at least one workspace.";
const STILL_OPEN: &str = "Close its window first.";
const UNKNOWN_WORKSPACE: &str = "That workspace no longer exists.";
const BLANK_NAME: &str = "A workspace needs a name.";

/// A window's normal size and position in physical pixels, and whether it was maximised. The
/// same shape as `state.json`'s `window`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// Which sidebars and comments a window shows, and how wide the sidebars are: the five layout
/// fields of `Settings`, whose defaults these are.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Layout {
    pub library_visible: bool,
    pub outline_visible: bool,
    pub library_width: u16,
    pub outline_width: u16,
    pub comments_visible: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self::of(&Settings::default())
    }
}

impl Layout {
    fn of(s: &Settings) -> Self {
        Self {
            library_visible: s.library_visible,
            outline_visible: s.outline_visible,
            library_width: s.library_width,
            outline_width: s.outline_width,
            comments_visible: s.comments_visible,
        }
    }

    fn write_into(&self, s: &mut Settings) {
        s.library_visible = self.library_visible;
        s.outline_visible = self.outline_visible;
        s.library_width = self.library_width;
        s.outline_width = self.outline_width;
        s.comments_visible = self.comments_visible;
    }
}

/// A workspace's own theme, shown instead of the shared one. Missing fields take the `Settings`
/// defaults.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceTheme {
    pub mode: ThemeMode,
    pub light: ThemeId,
    pub dark: ThemeId,
}

impl Default for WorkspaceTheme {
    fn default() -> Self {
        let s = Settings::default();
        Self {
            mode: s.theme_mode,
            light: s.light_theme,
            dark: s.dark_theme,
        }
    }
}

/// One workspace in `workspaces.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    /// "w1", "w2", …, never reused.
    pub id: String,
    pub name: String,
    /// The library roots, in the order they were added.
    pub roots: Vec<String>,
    /// `None` shows the shared theme ("Same as other windows").
    pub theme: Option<WorkspaceTheme>,
    pub layout: Layout,
    pub last_doc: Option<String>,
    /// Newest first.
    pub recent: Vec<RecentEntry>,
    pub placement: Option<WindowPlacement>,
    /// Shown in a window. The next launch reopens every workspace that was open.
    pub open: bool,
}

/// `workspaces.json`. Missing fields take their defaults and unknown fields are ignored, so files
/// from older and newer versions both load.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspaces {
    pub version: u32,
    /// The number in the next new workspace's id.
    pub next_id: u64,
    /// In creation order. The first is mirrored into `settings.json`.
    pub items: Vec<Workspace>,
    /// Every workspace id, the most recently focused first.
    pub focus: Vec<String>,
}

impl Default for Workspaces {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            items: Vec::new(),
            focus: Vec::new(),
        }
    }
}

impl Workspaces {
    /// An older Lectern's profile as one open workspace, "Main": the libraries and layout of
    /// `settings`, with the last document, recent files and window placement of `state.json`.
    pub fn migrate(
        settings: &Settings,
        last_doc: Option<String>,
        recent: Vec<RecentEntry>,
        placement: Option<WindowPlacement>,
    ) -> Self {
        let id = id_of(1);
        Self {
            version: VERSION,
            next_id: 2,
            items: vec![Workspace {
                id: id.clone(),
                name: MIGRATED_NAME.to_owned(),
                roots: settings.library_roots.clone(),
                theme: None,
                layout: Layout::of(settings),
                last_doc,
                recent,
                placement,
                open: true,
            }],
            focus: vec![id],
        }
    }

    /// Repairs a file that may have been edited by hand: drops repeated ids (keeping the first),
    /// trims names, names the blank ones and caps them at 60 characters, lists every id in
    /// `focus` exactly once, drops repeated roots (compared as Windows compares paths), and moves
    /// `next_id` above every numbered id. It leaves an empty `items` empty; the loader migrates
    /// that instead.
    pub fn normalize(&mut self) {
        let mut ids = HashSet::new();
        self.items.retain(|w| ids.insert(w.id.clone()));
        for w in &mut self.items {
            w.name = clean_name(&w.name).unwrap_or_default();
            let mut keys = HashSet::new();
            w.roots
                .retain(|root| keys.insert(path_key(Path::new(root))));
        }
        for i in 0..self.items.len() {
            if self.items[i].name.is_empty() {
                self.items[i].name = self.suggest_name();
            }
        }
        let mut listed = HashSet::new();
        self.focus
            .retain(|id| ids.contains(id) && listed.insert(id.clone()));
        for w in &self.items {
            if !listed.contains(&w.id) {
                self.focus.push(w.id.clone());
            }
        }
        self.next_id = self.next_id.max(self.above_every_id());
    }

    /// Adds a closed workspace with no libraries, the shared theme and the default layout, as the
    /// least recently focused. A blank `name` takes `suggest_name()`. Returns its id.
    pub fn create(&mut self, name: &str) -> String {
        let name = clean_name(name).unwrap_or_else(|| self.suggest_name());
        let number = self.next_id.max(self.above_every_id());
        self.next_id = number.saturating_add(1);
        let id = id_of(number);
        self.items.push(Workspace {
            id: id.clone(),
            name,
            ..Workspace::default()
        });
        self.focus.push(id.clone());
        id
    }

    /// Renames `id`, trimmed and capped as `normalize` does. The error is for the user.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let ws = self.get_mut(id).ok_or(UNKNOWN_WORKSPACE)?;
        ws.name = clean_name(name).ok_or(BLANK_NAME)?;
        Ok(())
    }

    /// Forgets `id`, unless it is the last workspace or open in a window. The error is for the
    /// user.
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let at = self
            .items
            .iter()
            .position(|w| w.id == id)
            .ok_or(UNKNOWN_WORKSPACE)?;
        if self.items.len() == 1 {
            return Err(LAST_WORKSPACE.to_owned());
        }
        if self.items[at].open {
            return Err(STILL_OPEN.to_owned());
        }
        self.items.remove(at);
        self.focus.retain(|f| f != id);
        Ok(())
    }

    /// Makes `id` the most recently focused workspace. An unknown id changes nothing.
    pub fn touch_focus(&mut self, id: &str) {
        if self.get(id).is_some() {
            self.focus.retain(|f| f != id);
            self.focus.insert(0, id.to_owned());
        }
    }

    /// "Workspace N" for the smallest N from 2 that no workspace's name uses, ignoring case.
    pub fn suggest_name(&self) -> String {
        let taken: HashSet<String> = self
            .items
            .iter()
            .map(|w| w.name.trim().to_lowercase())
            .collect();
        let mut n = 2;
        loop {
            let name = format!("Workspace {n}");
            if !taken.contains(&name.to_lowercase()) {
                return name;
            }
            n += 1;
        }
    }

    /// Copies the first workspace's libraries and layout into `settings`, where an older Lectern
    /// reads them.
    pub fn mirror_into(&self, settings: &mut Settings) {
        if let Some(first) = self.items.first() {
            settings.library_roots.clone_from(&first.roots);
            first.layout.write_into(settings);
        }
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.items.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.items.iter_mut().find(|w| w.id == id)
    }

    /// A number above every numbered id ("w7" is 7), and at least 1.
    fn above_every_id(&self) -> u64 {
        self.items
            .iter()
            .filter_map(|w| w.id.strip_prefix('w')?.parse::<u64>().ok())
            .max()
            .map_or(1, |n| n.saturating_add(1))
    }
}

fn id_of(number: u64) -> String {
    format!("w{number}")
}

/// `name` trimmed and cut to 60 characters, or `None` when that leaves nothing.
fn clean_name(name: &str) -> Option<String> {
    let capped: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = capped.trim_end();
    (!name.is_empty()).then(|| name.to_owned())
}

/// What a window shows besides the shared settings: its workspace, or, for a blank window, the
/// layout it keeps for itself, which lasts as long as the window and is never saved.
#[derive(Clone, Copy, Debug)]
pub enum Shown<'a> {
    Workspace(&'a Workspace),
    Blank(&'a Layout),
}

/// `Shown`, for a change.
#[derive(Debug)]
pub enum ShownMut<'a> {
    Workspace(&'a mut Workspace),
    Blank(&'a mut Layout),
}

/// The settings a window shows: the shared ones, with its workspace's libraries and layout and,
/// when it has one, the workspace's own theme. A blank window has no libraries, and its own
/// layout.
pub fn effective_settings(shared: &Settings, shown: Shown<'_>) -> Settings {
    let mut s = shared.clone();
    match shown {
        Shown::Workspace(ws) => {
            s.library_roots.clone_from(&ws.roots);
            ws.layout.write_into(&mut s);
            if let Some(theme) = &ws.theme {
                s.theme_mode = theme.mode.clone();
                s.light_theme = theme.light.clone();
                s.dark_theme = theme.dark.clone();
            }
        }
        Shown::Blank(layout) => {
            s.library_roots.clear();
            layout.write_into(&mut s);
        }
    }
    s
}

/// Where a settings change went. Each is true when the patch set at least one field there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchEffect {
    pub shared_changed: bool,
    pub workspace_changed: bool,
    /// A blank window's own layout.
    pub blank_changed: bool,
}

/// Applies a change made in the window that shows `shown`, sending each field to exactly one
/// place:
/// - the theme fields to the workspace's own theme when it has one, else to the shared settings
/// - the libraries and the layout to the workspace; a blank window keeps the layout for itself and
///   has no libraries, so those are dropped
/// - everything else to the shared settings, which are then clamped
pub fn apply_patch(
    shared: &mut Settings,
    shown: ShownMut<'_>,
    patch: SettingsPatch,
) -> PatchEffect {
    // Named in full, so a new field fails to compile until it is routed.
    let SettingsPatch {
        mut theme_mode,
        mut light_theme,
        mut dark_theme,
        body_font,
        code_font,
        font_size,
        line_height,
        measure,
        code_wrap,
        library_visible,
        outline_visible,
        library_width,
        outline_width,
        library_roots,
        path_mappings,
        editor,
        auto_update,
        show_status_badges,
        sidebar_font_size,
        review_comments,
        comments_visible,
    } = patch;
    let mut effect = PatchEffect::default();
    let (layout, changed) = match shown {
        ShownMut::Workspace(ws) => {
            let changed = &mut effect.workspace_changed;
            if let Some(theme) = &mut ws.theme {
                *changed |= set(&mut theme.mode, theme_mode.take());
                *changed |= set(&mut theme.light, light_theme.take());
                *changed |= set(&mut theme.dark, dark_theme.take());
            }
            *changed |= set(&mut ws.roots, library_roots);
            (&mut ws.layout, changed)
        }
        ShownMut::Blank(layout) => (layout, &mut effect.blank_changed),
    };
    *changed |= set(&mut layout.library_visible, library_visible);
    *changed |= set(&mut layout.outline_visible, outline_visible);
    *changed |= set(&mut layout.library_width, library_width);
    *changed |= set(&mut layout.outline_width, outline_width);
    *changed |= set(&mut layout.comments_visible, comments_visible);
    let changed = &mut effect.shared_changed;
    *changed |= set(&mut shared.theme_mode, theme_mode);
    *changed |= set(&mut shared.light_theme, light_theme);
    *changed |= set(&mut shared.dark_theme, dark_theme);
    *changed |= set(&mut shared.body_font, body_font);
    *changed |= set(&mut shared.code_font, code_font);
    *changed |= set(&mut shared.font_size, font_size);
    *changed |= set(&mut shared.line_height, line_height);
    *changed |= set(&mut shared.measure, measure);
    *changed |= set(&mut shared.code_wrap, code_wrap);
    *changed |= set(&mut shared.path_mappings, path_mappings);
    *changed |= set(&mut shared.editor, editor);
    *changed |= set(&mut shared.auto_update, auto_update);
    *changed |= set(&mut shared.show_status_badges, show_status_badges);
    *changed |= set(&mut shared.sidebar_font_size, sidebar_font_size);
    *changed |= set(&mut shared.review_comments, review_comments);
    shared.clamp();
    effect
}

/// Sets `field` to `value` when there is one; true when it did.
fn set<T>(field: &mut T, value: Option<T>) -> bool {
    let Some(value) = value else {
        return false;
    };
    *field = value;
    true
}

/// An open window, as `route_open` sees it.
#[derive(Clone, Debug)]
pub struct OpenWindow {
    pub label: String,
    /// `None` for a blank window.
    pub workspace: Option<String>,
    pub roots: Vec<String>,
}

/// Where an open from Explorer or the command line goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// To the window with this label.
    Window(String),
    /// To this closed workspace, reopened in a new window.
    Reopen(String),
}

/// Where an open of `path` from Explorer or the command line goes.
///
/// - A file goes to the most recently focused window whose libraries hold it, else to the most
///   recently focused closed workspace whose libraries hold it (reopened), else to the most
///   recently focused window.
/// - A folder goes to the most recently focused window that shows a workspace, else (every
///   window is blank) to the most recently focused window.
///
/// Paths compare as Windows compares them: case-insensitively, either separator. `focus` holds
/// window labels, the most recent first; a window missing from it counts as least recent.
/// `closed` holds the closed workspaces' ids and roots, the most recently focused first.
/// `windows` is never empty while Lectern runs; given none, the answer is a window with an empty
/// label.
pub fn route_open(
    path: &Path,
    is_dir: bool,
    windows: &[OpenWindow],
    closed: &[(String, Vec<String>)],
    focus: &[String],
) -> Route {
    let key = path_key(path);
    let holds = |roots: &[String]| {
        roots
            .iter()
            .any(|root| key_under(&key, &path_key(Path::new(root))).is_some())
    };
    let latest = |pick: &dyn Fn(&OpenWindow) -> bool| {
        windows.iter().filter(|w| pick(w)).min_by_key(|w| {
            focus
                .iter()
                .position(|l| *l == w.label)
                .unwrap_or(usize::MAX)
        })
    };
    if is_dir {
        if let Some(w) = latest(&|w| w.workspace.is_some()) {
            return Route::Window(w.label.clone());
        }
    } else {
        if let Some(w) = latest(&|w| holds(&w.roots)) {
            return Route::Window(w.label.clone());
        }
        if let Some((id, _)) = closed.iter().find(|(_, roots)| holds(roots)) {
            return Route::Reopen(id.clone());
        }
    }
    Route::Window(
        latest(&|_| true)
            .map(|w| w.label.clone())
            .unwrap_or_default(),
    )
}

/// What `load_workspaces` found.
#[derive(Debug)]
pub struct LoadedWorkspaces {
    pub workspaces: Workspaces,
    /// For the user, once: the file couldn't be read.
    pub notice: Option<String>,
    /// `workspaces.json` is there but could be neither read nor moved aside, so the workspaces
    /// were made from the other files instead. Saving them would replace the user's, so nothing
    /// is saved to it this session.
    pub read_only: bool,
}

/// Reads `workspaces.json` from `config_dir`, normalized. A missing file, or one with no
/// workspaces, becomes the migration of the given profile. A corrupt one is kept beside it as a
/// backup and also migrated, from the libraries and layout `settings.json` mirrors, with a notice
/// for the user. So is one that can be neither read nor moved aside, which is then left alone
/// (`read_only`).
pub fn load_workspaces(
    config_dir: &Path,
    settings: &Settings,
    last_doc: Option<String>,
    recent: Vec<RecentEntry>,
    placement: Option<WindowPlacement>,
) -> LoadedWorkspaces {
    let path = config_dir.join(WORKSPACES_FILE);
    let migrate = || Workspaces::migrate(settings, last_doc, recent, placement);
    let mut loaded = LoadedWorkspaces {
        workspaces: Workspaces::default(),
        notice: None,
        read_only: false,
    };
    loaded.workspaces = match load_json_or_default::<Workspaces>(&path) {
        Loaded::Ok(saved) if !saved.items.is_empty() => saved,
        Loaded::Ok(_) => migrate(),
        // There, or no telling: either way it mustn't be replaced.
        Loaded::Fresh(_) if path.try_exists().unwrap_or(true) => {
            log::warn!("workspaces could be neither read nor moved aside; saving none of them");
            loaded.read_only = true;
            loaded.notice = Some(
                "Lectern couldn't read its workspaces file, so it opened your libraries from the \
                 settings. Workspace changes won't be saved until you restart Lectern."
                    .to_owned(),
            );
            migrate()
        }
        Loaded::Fresh(_) => migrate(),
        Loaded::RecoveredFromCorrupt { backup, .. } => {
            log::warn!("workspaces were unreadable; kept as {}", backup.display());
            loaded.notice = Some(format!(
                "Lectern couldn't read its workspaces, so they were rebuilt from your libraries. \
                 The old file was kept as {}.",
                backup.display()
            ));
            migrate()
        }
    };
    loaded.workspaces.normalize();
    loaded
}
