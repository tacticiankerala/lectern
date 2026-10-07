//! Workspaces: migrating a v0.2.1 profile, `workspaces.json`, which settings a window sees and
//! where a change goes, and which window an Explorer or command-line open goes to.

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::ipc::{
    EditorPref, Measure, PathMapping, Settings, SettingsPatch, ThemeId, ThemeMode,
};
use lectern_core::store::State;
use lectern_core::workspace::{
    apply_patch, effective_settings, load_workspaces, route_open, Layout, OpenWindow, PatchEffect,
    Route, WindowPlacement, Workspace, WorkspaceTheme, Workspaces, WORKSPACES_FILE,
};
use serde::Deserialize;
use serde_json::{json, Value};

fn v021_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/workspace/v0.2.1")
        .join(name)
}

/// `state.json` as the app reads it: core's reading state beside the window placement.
#[derive(Deserialize)]
struct StateFile {
    #[serde(flatten)]
    reading: State,
    window: Option<WindowPlacement>,
}

/// The invented v0.2.1 profile: `settings.json` and `state.json`.
fn v021() -> (Settings, StateFile) {
    let settings = serde_json::from_slice(&fs::read(v021_file("settings.json")).unwrap()).unwrap();
    let state = serde_json::from_slice(&fs::read(v021_file("state.json")).unwrap()).unwrap();
    (settings, state)
}

fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn ids(ws: &Workspaces) -> Vec<&str> {
    ws.items.iter().map(|w| w.id.as_str()).collect()
}

/// A fresh profile: "Main" (w1), open, with no libraries.
fn main_only() -> Workspaces {
    Workspaces::migrate(&Settings::default(), None, Vec::new(), None)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&s| s.to_owned()).collect()
}

fn window(label: &str, workspace: Option<&str>, roots: &[&str]) -> OpenWindow {
    OpenWindow {
        label: label.to_owned(),
        workspace: workspace.map(str::to_owned),
        roots: strings(roots),
    }
}

fn closed(id: &str, roots: &[&str]) -> (String, Vec<String>) {
    (id.to_owned(), strings(roots))
}

fn to(label: &str) -> Route {
    Route::Window(label.to_owned())
}

// Migration and loading.

#[test]
fn a_v021_profile_becomes_one_workspace_named_main() {
    let (mut settings, state) = v021();
    let config = tempfile::tempdir().unwrap();
    let (ws, notice) = load_workspaces(
        config.path(),
        &settings,
        state.reading.last_doc.clone(),
        state.reading.recent.clone(),
        state.window,
    );
    assert!(notice.is_none());
    assert_eq!(ws.version, 1);
    assert_eq!(ws.next_id, 2);
    assert_eq!(ws.focus, ["w1"]);
    assert_eq!(ids(&ws), ["w1"]);
    let main = &ws.items[0];
    assert_eq!(main.name, "Main");
    assert!(main.open);
    assert_eq!(main.roots, [r"S:\Notes\My Vault", r"\\nas\share\garden"]);
    assert!(main.theme.is_none());
    assert_eq!(
        main.layout,
        Layout {
            library_visible: true,
            outline_visible: false,
            library_width: 312,
            outline_width: 260,
            comments_visible: false,
        }
    );
    assert_eq!(
        main.last_doc.as_deref(),
        Some(r"S:\Notes\My Vault\projects\plan.md")
    );
    let recent: Vec<_> = main
        .recent
        .iter()
        .map(|e| (e.path.as_str(), e.title.as_str(), e.opened_ms))
        .collect();
    assert_eq!(
        recent,
        [
            (
                r"S:\Notes\My Vault\projects\plan.md",
                "Plan",
                1_791_100_000_000
            ),
            (
                r"\\nas\share\garden\tomatoes.md",
                "Tomatoes",
                1_791_000_000_000
            ),
            (
                r"C:\Users\me\Downloads\notes.md",
                "Notes",
                1_790_900_000_000
            ),
        ]
    );
    assert_eq!(
        main.placement,
        Some(WindowPlacement {
            x: 120,
            y: 80,
            width: 1280,
            height: 860,
            maximized: true,
        })
    );
    // Loading writes nothing; the app's first save does.
    assert!(!config.path().join(WORKSPACES_FILE).exists());
    // `migrate` is what the loader ran.
    assert_eq!(
        json_of(&ws),
        json_of(&Workspaces::migrate(
            &settings,
            state.reading.last_doc,
            state.reading.recent,
            state.window,
        ))
    );

    // Mirroring Main back into the settings changes nothing.
    let before = json_of(&settings);
    ws.mirror_into(&mut settings);
    assert_eq!(json_of(&settings), before);
}

#[test]
fn a_workspaces_file_with_no_items_migrates_quietly() {
    let (settings, state) = v021();
    let config = tempfile::tempdir().unwrap();
    fs::write(config.path().join(WORKSPACES_FILE), r#"{"items": []}"#).unwrap();
    let (ws, notice) = load_workspaces(
        config.path(),
        &settings,
        state.reading.last_doc,
        state.reading.recent,
        state.window,
    );
    assert!(notice.is_none());
    assert_eq!(ids(&ws), ["w1"]);
    assert_eq!(ws.items[0].name, "Main");
    assert_eq!(ws.items[0].roots, settings.library_roots);
    assert_eq!(ws.items[0].recent.len(), 3);
}

#[test]
fn a_corrupt_workspaces_file_is_kept_and_rebuilt_from_the_libraries() {
    let (settings, state) = v021();
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join(WORKSPACES_FILE);
    fs::write(&path, "{not json").unwrap();
    let (ws, notice) = load_workspaces(
        config.path(),
        &settings,
        state.reading.last_doc,
        state.reading.recent,
        state.window,
    );
    assert_eq!(ids(&ws), ["w1"]);
    assert_eq!(ws.items[0].name, "Main");
    assert_eq!(ws.items[0].roots, settings.library_roots);
    assert!(!path.exists(), "the corrupt file is moved aside");
    let backups: Vec<PathBuf> = fs::read_dir(config.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("workspaces.corrupt-")
        })
        .collect();
    let [backup] = backups.as_slice() else {
        panic!("expected one backup, found {backups:?}");
    };
    assert_eq!(fs::read_to_string(backup).unwrap(), "{not json");
    assert_eq!(
        notice,
        Some(format!(
            "Lectern couldn't read its workspaces, so they were rebuilt from your libraries. \
             The old file was kept as {}.",
            backup.display()
        ))
    );
}

#[test]
fn a_saved_file_loads_normalized() {
    let config = tempfile::tempdir().unwrap();
    let saved = json!({
        "version": 1,
        "nextId": 3,
        "items": [
            {"id": "w1", "name": "Work", "roots": [r"S:\Notes\My Vault", "s:/notes/my vault/"], "open": true},
            {"id": "w2", "name": " Personal ", "roots": [r"C:\Users\me\projects"]}
        ],
        "focus": ["w2"]
    });
    fs::write(config.path().join(WORKSPACES_FILE), saved.to_string()).unwrap();
    let mirror = Settings {
        library_roots: strings(&[r"\\nas\share"]),
        ..Settings::default()
    };
    let (ws, notice) = load_workspaces(config.path(), &mirror, None, Vec::new(), None);
    assert!(notice.is_none());
    assert_eq!(ids(&ws), ["w1", "w2"]);
    assert_eq!(ws.items[0].name, "Work");
    assert_eq!(ws.items[0].roots, [r"S:\Notes\My Vault"]);
    assert_eq!(ws.items[1].name, "Personal");
    assert_eq!(ws.focus, ["w2", "w1"]);
    assert_eq!(ws.next_id, 3);
}

// The file.

#[test]
fn workspaces_json_is_camel_case() {
    let settings = Settings {
        library_roots: strings(&[r"S:\Notes\My Vault"]),
        ..Settings::default()
    };
    let placement = WindowPlacement {
        x: -8,
        y: 0,
        width: 1280,
        height: 860,
        maximized: false,
    };
    let ws = Workspaces::migrate(
        &settings,
        Some(r"S:\Notes\My Vault\a.md".to_owned()),
        Vec::new(),
        Some(placement),
    );
    let j = json_of(&ws);
    assert_eq!(j["version"], 1);
    assert_eq!(j["nextId"], 2);
    assert_eq!(j["focus"], json!(["w1"]));
    let main = &j["items"][0];
    assert_eq!(main["id"], "w1");
    assert_eq!(main["roots"], json!([r"S:\Notes\My Vault"]));
    assert_eq!(main["lastDoc"], r"S:\Notes\My Vault\a.md");
    assert!(main["theme"].is_null());
    assert_eq!(
        main["layout"],
        json!({"libraryVisible": true, "outlineVisible": true, "libraryWidth": 280, "outlineWidth": 240, "commentsVisible": true})
    );
    // The same shape as `state.json`'s `window`.
    assert_eq!(
        main["placement"],
        json!({"x": -8, "y": 0, "width": 1280, "height": 860, "maximized": false})
    );
    assert_eq!(main["open"], true);
    let theme = WorkspaceTheme {
        mode: ThemeMode::Dark,
        light: ThemeId::Sepia,
        dark: ThemeId::Nord,
    };
    assert_eq!(
        json_of(&theme),
        json!({"mode": "dark", "light": "sepia", "dark": "nord"})
    );
}

#[test]
fn workspaces_json_ignores_unknown_fields_and_defaults_missing_ones() {
    let ws: Workspaces = serde_json::from_value(json!({
        "items": [{"id": "w1", "name": "Work", "fromTheFuture": 1}],
        "alsoFromTheFuture": true
    }))
    .unwrap();
    assert_eq!(ws.version, 1);
    assert_eq!(ws.next_id, 1);
    let work = &ws.items[0];
    assert_eq!(work.layout, Layout::default());
    assert!(work.roots.is_empty() && work.recent.is_empty());
    assert!(work.theme.is_none() && work.last_doc.is_none() && work.placement.is_none());
    assert!(!work.open);
    let theme: WorkspaceTheme = serde_json::from_value(json!({"light": "latte"})).unwrap();
    assert!(matches!(theme.mode, ThemeMode::System));
    assert!(matches!(theme.light, ThemeId::Latte));
    assert!(matches!(theme.dark, ThemeId::Graphite));
}

#[test]
fn the_default_layout_is_the_default_settings_layout() {
    let s = Settings::default();
    assert_eq!(
        Layout::default(),
        Layout {
            library_visible: s.library_visible,
            outline_visible: s.outline_visible,
            library_width: s.library_width,
            outline_width: s.outline_width,
            comments_visible: s.comments_visible,
        }
    );
}

// Operations.

#[test]
fn normalize_repairs_a_hand_edited_file() {
    let mut ws: Workspaces = serde_json::from_value(json!({
        "version": 1,
        "nextId": 2,
        "items": [
            {"id": "w1", "name": "  Work  ", "roots": [r"S:\Notes\My Vault", "s:/notes/my vault/", r"\\nas\share"]},
            {"id": "w1", "name": "Copy"},
            {"id": "w7", "name": "   "},
            {"id": "w3", "name": "ö".repeat(70)},
            {"id": "garden", "name": ""}
        ],
        "focus": ["w9", "w3", "w3", "w1"]
    }))
    .unwrap();
    ws.normalize();
    assert_eq!(ids(&ws), ["w1", "w7", "w3", "garden"]);
    let names: Vec<&str> = ws.items.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Work",
            "Workspace 2",
            "ö".repeat(60).as_str(),
            "Workspace 3"
        ]
    );
    assert_eq!(ws.items[0].roots, [r"S:\Notes\My Vault", r"\\nas\share"]);
    assert_eq!(ws.focus, ["w3", "w1", "w7", "garden"]);
    assert_eq!(ws.next_id, 8);

    // A next id already above every id stays, so a deleted workspace's id is never reused.
    let mut ws = main_only();
    ws.next_id = 12;
    ws.normalize();
    assert_eq!(ws.next_id, 12);
}

#[test]
fn the_suggested_name_is_the_smallest_free_number() {
    let mut ws = main_only();
    assert_eq!(ws.suggest_name(), "Workspace 2");
    assert_eq!(ws.create("Workspace 2"), "w2");
    assert_eq!(ws.suggest_name(), "Workspace 3");
    ws.rename("w2", "Personal").unwrap();
    assert_eq!(ws.suggest_name(), "Workspace 2");
    // Names compare case-insensitively.
    ws.rename("w1", "WORKSPACE 2").unwrap();
    assert_eq!(ws.suggest_name(), "Workspace 3");
}

#[test]
fn a_new_workspace_is_closed_empty_and_least_recently_focused() {
    let mut ws = main_only();
    assert_eq!(ws.create("  Garden  "), "w2");
    let garden = ws.get("w2").unwrap();
    assert_eq!(garden.name, "Garden");
    assert!(!garden.open);
    assert!(garden.roots.is_empty() && garden.recent.is_empty());
    assert!(garden.theme.is_none() && garden.last_doc.is_none() && garden.placement.is_none());
    assert_eq!(garden.layout, Layout::default());
    assert_eq!(ws.focus, ["w1", "w2"]);
    assert_eq!(ws.next_id, 3);
    // A blank name takes the suggestion.
    assert_eq!(ws.create("   "), "w3");
    assert_eq!(ws.get("w3").unwrap().name, "Workspace 2");
    // Ids are never reused.
    ws.delete("w3").unwrap();
    assert_eq!(ws.create("Personal"), "w4");
    assert!(ws.get("w3").is_none());
}

#[test]
fn rename_trims_and_caps_and_refuses_a_blank_name() {
    let mut ws = main_only();
    ws.rename("w1", "  Work  ").unwrap();
    assert_eq!(ws.get("w1").unwrap().name, "Work");
    ws.rename("w1", &"a".repeat(61)).unwrap();
    assert_eq!(ws.get("w1").unwrap().name, "a".repeat(60));
    assert_eq!(
        ws.rename("w1", "   "),
        Err("A workspace needs a name.".to_owned())
    );
    assert_eq!(ws.get("w1").unwrap().name, "a".repeat(60));
    assert_eq!(
        ws.rename("w9", "Work"),
        Err("That workspace no longer exists.".to_owned())
    );
}

#[test]
fn delete_refuses_an_open_workspace_and_the_last_one() {
    let mut ws = main_only();
    ws.create("Workspace 2");
    assert_eq!(ws.delete("w1"), Err("Close its window first.".to_owned()));
    ws.delete("w2").unwrap();
    assert_eq!(ids(&ws), ["w1"]);
    assert_eq!(ws.focus, ["w1"]);
    assert_eq!(
        ws.delete("w1"),
        Err("Lectern needs at least one workspace.".to_owned())
    );
    ws.get_mut("w1").unwrap().open = false;
    assert_eq!(
        ws.delete("w1"),
        Err("Lectern needs at least one workspace.".to_owned())
    );
    assert_eq!(
        ws.delete("w9"),
        Err("That workspace no longer exists.".to_owned())
    );
}

#[test]
fn touch_focus_puts_a_workspace_first() {
    let mut ws = main_only();
    ws.create("Work");
    ws.create("Personal");
    assert_eq!(ws.focus, ["w1", "w2", "w3"]);
    ws.touch_focus("w3");
    assert_eq!(ws.focus, ["w3", "w1", "w2"]);
    ws.touch_focus("w9");
    assert_eq!(ws.focus, ["w3", "w1", "w2"]);
}

#[test]
fn the_first_workspace_is_mirrored_into_the_settings() {
    let mut ws = main_only();
    let id = ws.create("Personal");
    ws.get_mut(&id).unwrap().roots = strings(&[r"C:\Users\me\projects"]);
    let main = ws.get_mut("w1").unwrap();
    main.roots = strings(&[r"S:\Notes\My Vault"]);
    main.layout = Layout {
        library_visible: false,
        outline_visible: false,
        library_width: 300,
        outline_width: 200,
        comments_visible: false,
    };
    let mut settings = Settings {
        font_size: 21,
        ..Settings::default()
    };
    ws.mirror_into(&mut settings);
    assert_eq!(settings.library_roots, [r"S:\Notes\My Vault"]);
    assert!(!settings.library_visible && !settings.outline_visible && !settings.comments_visible);
    assert_eq!((settings.library_width, settings.outline_width), (300, 200));
    assert_eq!(settings.font_size, 21);
}

// Settings.

fn personal() -> Workspace {
    Workspace {
        id: "w2".to_owned(),
        name: "Personal".to_owned(),
        roots: strings(&[r"C:\Users\me\projects"]),
        theme: Some(WorkspaceTheme {
            mode: ThemeMode::Light,
            light: ThemeId::Sepia,
            dark: ThemeId::Mocha,
        }),
        layout: Layout {
            library_visible: false,
            outline_visible: true,
            library_width: 300,
            outline_width: 200,
            comments_visible: false,
        },
        ..Workspace::default()
    }
}

#[test]
fn a_window_sees_its_workspace_libraries_layout_and_theme() {
    let shared = Settings {
        font_size: 21,
        theme_mode: ThemeMode::Dark,
        library_roots: strings(&[r"S:\Notes\My Vault"]),
        ..Settings::default()
    };
    let s = effective_settings(&shared, Some(&personal()));
    assert_eq!(s.library_roots, [r"C:\Users\me\projects"]);
    assert!(!s.library_visible && s.outline_visible && !s.comments_visible);
    assert_eq!((s.library_width, s.outline_width), (300, 200));
    assert!(matches!(s.theme_mode, ThemeMode::Light));
    assert!(matches!(s.light_theme, ThemeId::Sepia));
    assert!(matches!(s.dark_theme, ThemeId::Mocha));
    assert_eq!(s.font_size, 21);

    // Without a theme of its own, the window shows the shared one.
    let follows = Workspace {
        theme: None,
        ..personal()
    };
    let s = effective_settings(&shared, Some(&follows));
    assert!(matches!(s.theme_mode, ThemeMode::Dark));
    assert!(matches!(s.light_theme, ThemeId::Paper));
    assert_eq!(s.library_roots, [r"C:\Users\me\projects"]);
}

#[test]
fn a_blank_window_sees_no_libraries_and_the_default_layout() {
    let shared = Settings {
        font_size: 21,
        theme_mode: ThemeMode::Dark,
        library_roots: strings(&[r"S:\Notes\My Vault"]),
        library_visible: false,
        library_width: 400,
        comments_visible: false,
        ..Settings::default()
    };
    let s = effective_settings(&shared, None);
    assert!(s.library_roots.is_empty());
    let d = Settings::default();
    assert_eq!(
        (
            s.library_visible,
            s.outline_visible,
            s.library_width,
            s.outline_width,
            s.comments_visible
        ),
        (
            d.library_visible,
            d.outline_visible,
            d.library_width,
            d.outline_width,
            d.comments_visible
        )
    );
    assert!(matches!(s.theme_mode, ThemeMode::Dark));
    assert_eq!(s.font_size, 21);
}

#[test]
fn a_theme_change_goes_to_a_workspace_with_its_own_theme() {
    let mut shared = Settings::default();
    let mut w2 = personal();
    let effect = apply_patch(
        &mut shared,
        Some(&mut w2),
        SettingsPatch {
            light_theme: Some(ThemeId::Latte),
            font_size: Some(22),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(
        effect,
        PatchEffect {
            shared_changed: true,
            workspace_changed: true
        }
    );
    let theme = w2.theme.as_ref().unwrap();
    assert!(matches!(theme.light, ThemeId::Latte));
    assert!(matches!(theme.mode, ThemeMode::Light));
    assert!(matches!(theme.dark, ThemeId::Mocha));
    assert_eq!(shared.font_size, 22);
    assert!(matches!(shared.theme_mode, ThemeMode::System));
    assert!(matches!(shared.light_theme, ThemeId::Paper));
    assert!(matches!(shared.dark_theme, ThemeId::Graphite));
}

#[test]
fn a_theme_change_goes_to_shared_when_the_workspace_follows_it() {
    let mut shared = Settings::default();
    let mut w2 = Workspace {
        theme: None,
        ..personal()
    };
    let before = json_of(&w2);
    let effect = apply_patch(
        &mut shared,
        Some(&mut w2),
        SettingsPatch {
            theme_mode: Some(ThemeMode::Dark),
            dark_theme: Some(ThemeId::Nord),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(
        effect,
        PatchEffect {
            shared_changed: true,
            workspace_changed: false
        }
    );
    assert!(matches!(shared.theme_mode, ThemeMode::Dark));
    assert!(matches!(shared.dark_theme, ThemeId::Nord));
    assert_eq!(json_of(&w2), before);
}

#[test]
fn libraries_and_layout_go_to_the_workspace() {
    let mut shared = Settings::default();
    let before = json_of(&shared);
    let mut w2 = personal();
    let effect = apply_patch(
        &mut shared,
        Some(&mut w2),
        SettingsPatch {
            library_roots: Some(strings(&[r"C:\Users\me\projects", r"\\nas\share"])),
            library_visible: Some(true),
            outline_visible: Some(false),
            library_width: Some(333),
            outline_width: Some(222),
            comments_visible: Some(true),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(
        effect,
        PatchEffect {
            shared_changed: false,
            workspace_changed: true
        }
    );
    assert_eq!(w2.roots, [r"C:\Users\me\projects", r"\\nas\share"]);
    assert_eq!(
        w2.layout,
        Layout {
            library_visible: true,
            outline_visible: false,
            library_width: 333,
            outline_width: 222,
            comments_visible: true,
        }
    );
    assert_eq!(json_of(&shared), before);
}

#[test]
fn a_layout_change_from_a_blank_window_changes_nothing() {
    let mut shared = Settings::default();
    let before = json_of(&shared);
    let effect = apply_patch(
        &mut shared,
        None,
        SettingsPatch {
            library_width: Some(400),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(effect, PatchEffect::default());
    assert_eq!(json_of(&shared), before);

    // A theme change from a blank window goes to shared.
    let effect = apply_patch(
        &mut shared,
        None,
        SettingsPatch {
            theme_mode: Some(ThemeMode::Dark),
            library_roots: Some(strings(&[r"S:\Notes\My Vault"])),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(
        effect,
        PatchEffect {
            shared_changed: true,
            workspace_changed: false
        }
    );
    assert!(matches!(shared.theme_mode, ThemeMode::Dark));
    assert!(shared.library_roots.is_empty());
}

#[test]
fn every_other_setting_goes_to_shared_and_is_clamped() {
    let mut shared = Settings::default();
    let mut w2 = personal();
    let before = json_of(&w2);
    let effect = apply_patch(
        &mut shared,
        Some(&mut w2),
        SettingsPatch {
            body_font: Some("Georgia".to_owned()),
            code_font: Some("Cascadia Code".to_owned()),
            font_size: Some(99),
            line_height: Some(9.0),
            measure: Some(Measure::Chars(10)),
            code_wrap: Some(true),
            path_mappings: Some(vec![PathMapping {
                from: "/home/me/shared".to_owned(),
                to: r"S:\Shared".to_owned(),
            }]),
            editor: Some(EditorPref::Custom {
                command: "notepad.exe {path}".to_owned(),
            }),
            auto_update: Some(false),
            show_status_badges: Some(false),
            sidebar_font_size: Some(2),
            review_comments: Some(false),
            ..SettingsPatch::default()
        },
    );
    assert_eq!(
        effect,
        PatchEffect {
            shared_changed: true,
            workspace_changed: false
        }
    );
    assert_eq!(json_of(&w2), before);
    assert_eq!(shared.body_font, "Georgia");
    assert_eq!(shared.code_font, "Cascadia Code");
    assert_eq!(shared.font_size, 32);
    assert_eq!(shared.line_height, 2.0);
    assert!(matches!(shared.measure, Measure::Chars(60)));
    assert!(shared.code_wrap);
    assert_eq!(shared.path_mappings[0].to, r"S:\Shared");
    assert!(matches!(shared.editor, EditorPref::Custom { .. }));
    assert!(!shared.auto_update);
    assert!(!shared.show_status_badges);
    assert_eq!(shared.sidebar_font_size, 11);
    assert!(!shared.review_comments);
}

// Open routing.

#[test]
fn a_file_goes_to_the_window_whose_libraries_hold_it() {
    let windows = [
        window("main", Some("w1"), &[r"S:\Notes\My Vault"]),
        window("win-1", Some("w2"), &[r"C:\Users\me\projects"]),
    ];
    let focus = strings(&["win-1", "main"]);
    assert_eq!(
        route_open(
            Path::new("s:/notes/my vault/plan.md"),
            false,
            &windows,
            &[],
            &focus
        ),
        to("main")
    );
    // A folder joins the most recently focused window's workspace.
    assert_eq!(
        route_open(Path::new(r"D:\new"), true, &windows, &[], &focus),
        to("win-1")
    );
}

#[test]
fn a_file_only_a_closed_workspace_holds_reopens_it() {
    let windows = [
        window("main", Some("w1"), &[r"S:\Notes\My Vault"]),
        window("win-2", None, &[]),
    ];
    let shut = [closed("w2", &[r"C:\Users\me\projects"])];
    let focus = strings(&["win-2", "main"]);
    assert_eq!(
        route_open(
            Path::new(r"C:\Users\me\projects\lectern\todo.md"),
            false,
            &windows,
            &shut,
            &focus
        ),
        Route::Reopen("w2".to_owned())
    );
    // Held by neither: the most recently focused window, blank or not.
    assert_eq!(
        route_open(
            Path::new(r"D:\elsewhere\a.md"),
            false,
            &windows,
            &shut,
            &focus
        ),
        to("win-2")
    );
    // A sibling folder that only starts with a root's name isn't under it.
    assert_eq!(
        route_open(
            Path::new(r"S:\Notes\My Vault2\a.md"),
            false,
            &windows,
            &shut,
            &focus
        ),
        to("win-2")
    );
}

#[test]
fn an_open_window_wins_over_a_closed_workspace_and_closed_ones_go_by_focus() {
    let windows = [window("main", Some("w1"), &[r"S:\Notes\My Vault"])];
    let shut = [
        closed("w3", &[r"S:\Notes"]),
        closed("w2", &[r"S:\Notes\Garden"]),
    ];
    let focus = strings(&["main"]);
    assert_eq!(
        route_open(
            Path::new(r"S:\Notes\My Vault\a.md"),
            false,
            &windows,
            &shut,
            &focus
        ),
        to("main")
    );
    // Both closed workspaces hold it; w3 was focused more recently.
    assert_eq!(
        route_open(
            Path::new(r"S:\Notes\Garden\b.md"),
            false,
            &windows,
            &shut,
            &focus
        ),
        Route::Reopen("w3".to_owned())
    );
}

#[test]
fn the_same_folder_in_two_windows_goes_to_the_more_recently_focused() {
    let windows = [
        window("main", Some("w1"), &[r"S:\Notes\My Vault"]),
        window("win-1", Some("w2"), &[r"s:\notes\my vault\"]),
        window("win-2", Some("w3"), &[r"C:\Users\me\projects"]),
    ];
    let file = Path::new(r"S:\Notes\My Vault\plan.md");
    assert_eq!(
        route_open(
            file,
            false,
            &windows,
            &[],
            &strings(&["win-2", "win-1", "main"])
        ),
        to("win-1")
    );
    assert_eq!(
        route_open(
            file,
            false,
            &windows,
            &[],
            &strings(&["main", "win-2", "win-1"])
        ),
        to("main")
    );
    // A window missing from the focus list counts as least recent.
    assert_eq!(
        route_open(file, false, &windows, &[], &strings(&["win-1"])),
        to("win-1")
    );
}

#[test]
fn a_folder_skips_blank_windows_unless_every_window_is_blank() {
    let windows = [
        window("main", Some("w1"), &[r"S:\Notes\My Vault"]),
        window("win-1", None, &[]),
    ];
    assert_eq!(
        route_open(
            Path::new(r"\\nas\share\garden"),
            true,
            &windows,
            &[],
            &strings(&["win-1", "main"])
        ),
        to("main")
    );
    let blank = [window("win-1", None, &[]), window("win-2", None, &[])];
    assert_eq!(
        route_open(
            Path::new(r"\\nas\share\garden"),
            true,
            &blank,
            &[closed("w1", &[r"\\nas\share"])],
            &strings(&["win-2", "win-1"])
        ),
        to("win-2")
    );
}
