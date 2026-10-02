//! Settings validation, reading state (positions and recent files), and the atomic JSON IO behind
//! `settings.json`, `state.json` and the library snapshots.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ipc::{
    EditorPref, Measure, RecentEntry, SavedPosition, Settings, SettingsPatch, ThemeId, ThemeMode,
};
use crate::library::path_key;

const FONT_SIZE: (u8, u8) = (12, 32);
const LINE_HEIGHT: (f64, f64) = (1.3, 2.0);
const MEASURE: (u16, u16) = (60, 160);
const DEFAULT_LINE_HEIGHT: f64 = 1.65;

/// Reading positions kept, least recently saved dropped first.
const MAX_POSITIONS: usize = 1000;
const MAX_RECENT: usize = 20;

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::System,
            light_theme: ThemeId::Paper,
            dark_theme: ThemeId::Graphite,
            body_font: "Segoe UI Variable Text".to_owned(),
            code_font: "JetBrains Mono".to_owned(),
            font_size: 18,
            line_height: DEFAULT_LINE_HEIGHT,
            measure: Measure::Chars(100),
            code_wrap: false,
            library_visible: true,
            outline_visible: true,
            library_width: 280,
            outline_width: 240,
            library_roots: Vec::new(),
            path_mappings: Vec::new(),
            editor: EditorPref::Auto,
            auto_update: true,
            show_status_badges: true,
        }
    }
}

impl Settings {
    /// Sets every field `p` holds, then clamps.
    pub fn apply(&mut self, p: SettingsPatch) {
        fn set<T>(field: &mut T, value: Option<T>) {
            if let Some(value) = value {
                *field = value;
            }
        }
        set(&mut self.theme_mode, p.theme_mode);
        set(&mut self.light_theme, p.light_theme);
        set(&mut self.dark_theme, p.dark_theme);
        set(&mut self.body_font, p.body_font);
        set(&mut self.code_font, p.code_font);
        set(&mut self.font_size, p.font_size);
        set(&mut self.line_height, p.line_height);
        set(&mut self.measure, p.measure);
        set(&mut self.code_wrap, p.code_wrap);
        set(&mut self.library_visible, p.library_visible);
        set(&mut self.outline_visible, p.outline_visible);
        set(&mut self.library_width, p.library_width);
        set(&mut self.outline_width, p.outline_width);
        set(&mut self.library_roots, p.library_roots);
        set(&mut self.path_mappings, p.path_mappings);
        set(&mut self.editor, p.editor);
        set(&mut self.auto_update, p.auto_update);
        set(&mut self.show_status_badges, p.show_status_badges);
        self.clamp();
    }

    /// Brings font size, line height and measure into range. Call after loading `settings.json`,
    /// which may have been edited by hand.
    pub fn clamp(&mut self) {
        self.font_size = self.font_size.clamp(FONT_SIZE.0, FONT_SIZE.1);
        self.line_height = if self.line_height.is_finite() {
            self.line_height.clamp(LINE_HEIGHT.0, LINE_HEIGHT.1)
        } else {
            DEFAULT_LINE_HEIGHT
        };
        if let Measure::Chars(chars) = &mut self.measure {
            *chars = (*chars).clamp(MEASURE.0, MEASURE.1);
        }
    }
}

/// `state.json`: reading positions, recent files and the last open document.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// `(path, position, saved at in Unix ms)`, oldest first.
    pub positions: Vec<(String, SavedPosition, i64)>,
    /// Newest first.
    pub recent: Vec<RecentEntry>,
    pub last_doc: Option<String>,
}

impl State {
    /// Saves the position in `path` as the newest, dropping the oldest beyond 1,000. Paths
    /// compare as on Windows: case-insensitive, either separator.
    pub fn set_position(&mut self, path: &str, p: SavedPosition, now_ms: i64) {
        let key = path_key(Path::new(path));
        self.positions
            .retain(|(saved, ..)| path_key(Path::new(saved)) != key);
        self.positions.push((path.to_owned(), p, now_ms));
        if self.positions.len() > MAX_POSITIONS {
            let excess = self.positions.len() - MAX_POSITIONS;
            self.positions.drain(..excess);
        }
    }

    pub fn position(&self, path: &str) -> Option<&SavedPosition> {
        let key = path_key(Path::new(path));
        self.positions
            .iter()
            .rev()
            .find(|(saved, ..)| path_key(Path::new(saved)) == key)
            .map(|(_, p, _)| p)
    }

    /// Puts `e` first, removing an earlier entry for the same path and keeping the newest 20.
    pub fn push_recent(&mut self, e: RecentEntry) {
        let key = path_key(Path::new(&e.path));
        self.recent
            .retain(|old| path_key(Path::new(&old.path)) != key);
        self.recent.insert(0, e);
        self.recent.truncate(MAX_RECENT);
    }

    /// Removes `path` from the recent files, compared as `push_recent` does; true when it was
    /// there.
    pub fn remove_recent(&mut self, path: &str) -> bool {
        let key = path_key(Path::new(path));
        let before = self.recent.len();
        self.recent
            .retain(|old| path_key(Path::new(&old.path)) != key);
        self.recent.len() != before
    }
}

/// Tells apart the temporary files of writes running at the same time.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `v` as JSON to `path`, creating its folder if needed. The JSON goes to a temporary
/// file in the same folder that then replaces `path`, so a reader never sees a half-written file.
pub fn write_json_atomic<T: Serialize>(path: &Path, v: &T) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);
    // `rename` replaces an existing target on Windows as well as on Unix.
    let written = write_new_json(&tmp, v).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn write_new_json<T: Serialize>(path: &Path, v: &T) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut out = BufWriter::new(file);
    serde_json::to_writer(&mut out, v)?;
    out.flush()?;
    out.get_ref().sync_all()
}

/// The outcome of `load_json_or_default`.
#[derive(Debug)]
pub enum Loaded<T> {
    /// There was no file; this is the default.
    Fresh(T),
    Ok(T),
    /// The file could not be read or parsed. It was moved to `backup` and this is the default.
    RecoveredFromCorrupt {
        value: T,
        backup: PathBuf,
    },
}

/// Reads JSON from `path`, or the default when the file is missing. A file that can't be read or
/// parsed is renamed to `<name>.corrupt-<unix ms>.json` beside it, so the next save doesn't
/// destroy it; if even that fails, the result is `Fresh`.
pub fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Loaded<T> {
    let parsed = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(drop),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Fresh(T::default()),
        Err(_) => Err(()),
    };
    match parsed {
        Ok(value) => Loaded::Ok(value),
        Err(()) => match back_up(path) {
            Ok(backup) => Loaded::RecoveredFromCorrupt {
                value: T::default(),
                backup,
            },
            Err(_) => Loaded::Fresh(T::default()),
        },
    }
}

fn back_up(path: &Path) -> io::Result<PathBuf> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let backup = path.with_file_name(format!("{stem}.corrupt-{ms}.json"));
    fs::rename(path, &backup)?;
    Ok(backup)
}
