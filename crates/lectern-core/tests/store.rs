use std::fs;
use std::path::Path;

use lectern_core::ipc::{
    EditorPref, Measure, PathMapping, RecentEntry, SavedPosition, Settings, SettingsPatch, ThemeId,
    ThemeMode,
};
use lectern_core::store::{load_json_or_default, write_json_atomic, Loaded, State};

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn pos(fraction: f64) -> SavedPosition {
    SavedPosition {
        heading_id: None,
        offset: 0.0,
        line: None,
        fraction,
    }
}

fn recent(path: &str) -> RecentEntry {
    RecentEntry {
        path: path.to_owned(),
        title: path.to_owned(),
        opened_ms: 0,
    }
}

fn recent_paths(s: &State) -> Vec<&str> {
    s.recent.iter().map(|e| e.path.as_str()).collect()
}

#[test]
fn settings_defaults_match_spec() {
    let s = Settings::default();
    assert!(matches!(s.theme_mode, ThemeMode::System));
    assert!(matches!(s.light_theme, ThemeId::Paper));
    assert!(matches!(s.dark_theme, ThemeId::Graphite));
    assert_eq!(s.body_font, "Segoe UI Variable Text");
    assert_eq!(s.code_font, "JetBrains Mono");
    assert_eq!(s.font_size, 18);
    assert_eq!(s.line_height, 1.65);
    assert!(matches!(s.measure, Measure::Chars(100)));
    assert!(!s.code_wrap);
    assert!(s.library_visible);
    assert!(s.outline_visible);
    assert_eq!(s.library_width, 280);
    assert_eq!(s.outline_width, 240);
    assert!(s.library_roots.is_empty());
    assert!(s.path_mappings.is_empty());
    assert!(matches!(s.editor, EditorPref::Auto));
    assert!(s.auto_update);
    assert!(s.show_status_badges);
    assert_eq!(s.sidebar_font_size, 13);
}

#[test]
fn settings_clamped() {
    let mut s = Settings::default();
    s.apply(SettingsPatch {
        font_size: Some(99),
        line_height: Some(0.5),
        ..Default::default()
    });
    assert_eq!(s.font_size, 32);
    assert_eq!(s.line_height, 1.3);

    s.apply(SettingsPatch {
        font_size: Some(3),
        line_height: Some(9.0),
        measure: Some(Measure::Chars(10)),
        ..Default::default()
    });
    assert_eq!(s.font_size, 12);
    assert_eq!(s.line_height, 2.0);
    assert!(matches!(s.measure, Measure::Chars(60)));

    s.apply(SettingsPatch {
        measure: Some(Measure::Chars(500)),
        ..Default::default()
    });
    assert!(matches!(s.measure, Measure::Chars(160)));
}

#[test]
fn sidebar_font_size_range_is_11_to_20() {
    for (saved, clamped) in [
        (0, 11),
        (10, 11),
        (11, 11),
        (16, 16),
        (20, 20),
        (21, 20),
        (255, 20),
    ] {
        let mut s = Settings {
            sidebar_font_size: saved,
            ..Settings::default()
        };
        s.clamp();
        assert_eq!(s.sidebar_font_size, clamped, "{saved} clamps to {clamped}");
    }
    let mut s = Settings::default();
    s.apply(SettingsPatch {
        sidebar_font_size: Some(99),
        ..Default::default()
    });
    assert_eq!(s.sidebar_font_size, 20);
}

#[test]
fn measure_range_is_60_to_160() {
    for (saved, clamped) in [
        (50, 60),
        (59, 60),
        (60, 60),
        (72, 72),
        (160, 160),
        (161, 160),
    ] {
        let mut s = Settings {
            measure: Measure::Chars(saved),
            ..Settings::default()
        };
        s.clamp();
        assert!(
            matches!(s.measure, Measure::Chars(c) if c == clamped),
            "{saved} clamps to {clamped}, got {:?}",
            s.measure
        );
    }
}

#[test]
fn clamp_resets_a_non_finite_line_height() {
    let mut s = Settings {
        line_height: f64::NAN,
        ..Settings::default()
    };
    s.clamp();
    assert_eq!(s.line_height, 1.65);
}

#[test]
fn apply_sets_only_the_patched_fields() {
    let mut s = Settings::default();
    s.apply(SettingsPatch {
        theme_mode: Some(ThemeMode::Dark),
        dark_theme: Some(ThemeId::Nord),
        measure: Some(Measure::Full),
        editor: Some(EditorPref::Custom {
            command: "code -g {path}:{line}".to_owned(),
        }),
        library_roots: Some(vec!["S:\\Dev".to_owned()]),
        show_status_badges: Some(false),
        sidebar_font_size: Some(17),
        path_mappings: Some(vec![PathMapping {
            from: "/home/me/shared".to_owned(),
            to: "S:\\".to_owned(),
        }]),
        ..Default::default()
    });
    assert!(matches!(s.theme_mode, ThemeMode::Dark));
    assert!(matches!(s.dark_theme, ThemeId::Nord));
    assert!(matches!(s.measure, Measure::Full));
    assert!(
        matches!(&s.editor, EditorPref::Custom { command } if command == "code -g {path}:{line}")
    );
    assert_eq!(s.library_roots, ["S:\\Dev"]);
    assert_eq!(s.path_mappings[0].to, "S:\\");
    assert!(!s.show_status_badges);
    assert_eq!(s.sidebar_font_size, 17);
    // Untouched fields keep their values.
    assert!(matches!(s.light_theme, ThemeId::Paper));
    assert_eq!(s.font_size, 18);
    assert!(s.auto_update);
}

#[test]
fn settings_json_shape() {
    let j = serde_json::to_value(Settings::default()).unwrap();
    assert_eq!(j["themeMode"], "system");
    assert_eq!(j["lightTheme"], "paper");
    assert_eq!(j["darkTheme"], "graphite");
    assert_eq!(j["fontSize"], 18);
    assert_eq!(j["measure"], 100);
    assert_eq!(j["editor"]["mode"], "auto");
    assert_eq!(j["autoUpdate"], true);
    assert_eq!(j["showStatusBadges"], true);
    assert_eq!(j["sidebarFontSize"], 13);
    assert_eq!(j["libraryRoots"], serde_json::json!([]));
    // Exactly 1.65 both as a JSON value and as text (an f32 would widen to 1.649999976158142).
    assert_eq!(j["lineHeight"].as_f64(), Some(1.65));
    let text = serde_json::to_string(&Settings::default()).unwrap();
    assert!(text.contains(r#""lineHeight":1.65,"#), "{text}");
}

#[test]
fn measure_full_roundtrip() {
    let m: Measure = serde_json::from_str("\"full\"").unwrap();
    assert!(matches!(m, Measure::Full));
    assert_eq!(serde_json::to_string(&Measure::Full).unwrap(), "\"full\"");
    let m: Measure = serde_json::from_str("96").unwrap();
    assert!(matches!(m, Measure::Chars(96)));
    assert_eq!(serde_json::to_string(&Measure::Chars(96)).unwrap(), "96");
}

#[test]
fn custom_editor_json_shape() {
    let e = EditorPref::Custom {
        command: "notepad.exe".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"mode": "custom", "command": "notepad.exe"})
    );
}

#[test]
fn settings_missing_fields_default_and_unknown_fields_are_ignored() {
    let s: Settings =
        serde_json::from_str(r#"{"fontSize": 20, "measure": "full", "fromTheFuture": 1}"#).unwrap();
    assert_eq!(s.font_size, 20);
    assert!(matches!(s.measure, Measure::Full));
    assert_eq!(s.line_height, 1.65);
    assert!(matches!(s.dark_theme, ThemeId::Graphite));
    // A file written before the setting existed shows the badges, as before.
    assert!(s.show_status_badges);
    // And sets the sidebars in the size they always had.
    assert_eq!(s.sidebar_font_size, 13);
}

#[test]
fn settings_patch_accepts_a_partial_object() {
    let p: SettingsPatch = serde_json::from_str(
        r#"{"fontSize": 22, "lightTheme": "sepia", "codeWrap": null, "sidebarFontSize": 15}"#,
    )
    .unwrap();
    assert_eq!(p.font_size, Some(22));
    assert_eq!(p.sidebar_font_size, Some(15));
    assert!(matches!(p.light_theme, Some(ThemeId::Sepia)));
    assert!(p.code_wrap.is_none());
    assert!(p.theme_mode.is_none());
}

#[test]
fn missing_file_is_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let loaded: Loaded<Settings> = load_json_or_default(&tmp.path().join("settings.json"));
    assert!(matches!(loaded, Loaded::Fresh(s) if s.font_size == 18));
    assert!(names_in(tmp.path()).is_empty());
}

#[test]
fn saved_file_loads_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    let s = Settings {
        font_size: 24,
        ..Settings::default()
    };
    write_json_atomic(&path, &s).unwrap();
    let loaded: Loaded<Settings> = load_json_or_default(&path);
    assert!(matches!(loaded, Loaded::Ok(s) if s.font_size == 24));
}

#[test]
fn corrupt_settings_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    fs::write(&path, "{nope").unwrap();
    let loaded: Loaded<Settings> = load_json_or_default(&path);
    let Loaded::RecoveredFromCorrupt { value, backup } = loaded else {
        panic!("expected RecoveredFromCorrupt, got {loaded:?}");
    };
    assert_eq!(value.font_size, 18);
    assert_eq!(fs::read_to_string(&backup).unwrap(), "{nope");
    assert_eq!(backup.parent(), Some(tmp.path()));
    let name = backup.file_name().unwrap().to_str().unwrap();
    let ts = name
        .strip_prefix("settings.corrupt-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .unwrap_or_else(|| panic!("unexpected backup name {name}"));
    assert!(ts.parse::<u64>().is_ok(), "{name}");
    assert!(!path.exists(), "the corrupt file is moved aside");
}

#[test]
fn atomic_write_leaves_no_tmp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("settings.json");
    write_json_atomic(&path, &Settings::default()).unwrap();
    let s = Settings {
        code_wrap: true,
        ..Settings::default()
    };
    write_json_atomic(&path, &s).unwrap();
    assert_eq!(names_in(&dir), ["settings.json"]);
    let back: Settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(back.code_wrap);
}

#[test]
fn positions_lru_capped() {
    let mut s = State::default();
    for i in 0..1001 {
        s.set_position(&format!("C:\\n\\{i}.md"), pos(0.5), i);
    }
    assert_eq!(s.positions.len(), 1000);
    assert!(s.position("C:\\n\\0.md").is_none(), "oldest evicted");
    assert!(s.position("C:\\n\\1.md").is_some());
    assert!(s.position("C:\\n\\1000.md").is_some());
}

#[test]
fn position_update_replaces_and_refreshes() {
    let mut s = State::default();
    s.set_position("C:\\n\\keep.md", pos(0.1), 0);
    for i in 1..1000 {
        s.set_position(&format!("C:\\n\\{i}.md"), pos(0.5), i);
    }
    // Saving again replaces the entry and makes it the newest, so the next insert evicts `1.md`.
    s.set_position("C:\\n\\keep.md", pos(0.9), 1000);
    s.set_position("C:\\n\\new.md", pos(0.5), 1001);
    assert_eq!(s.positions.len(), 1000);
    assert_eq!(s.position("C:\\n\\keep.md").unwrap().fraction, 0.9);
    assert!(s.position("C:\\n\\1.md").is_none());
}

#[test]
fn positions_match_paths_case_insensitively() {
    let mut s = State::default();
    s.set_position("S:\\Dev\\Plan.md", pos(0.25), 1);
    assert_eq!(s.position("s:/dev/plan.md").unwrap().fraction, 0.25);
    s.set_position("s:\\dev\\plan.md", pos(0.75), 2);
    assert_eq!(s.positions.len(), 1);
    assert_eq!(s.position("S:\\Dev\\Plan.md").unwrap().fraction, 0.75);
}

#[test]
fn recent_dedupes() {
    let mut s = State::default();
    s.push_recent(recent("a"));
    s.push_recent(recent("b"));
    s.push_recent(recent("a"));
    assert_eq!(recent_paths(&s), ["a", "b"]);
}

#[test]
fn recent_entry_is_removed_case_insensitively() {
    let mut s = State::default();
    s.push_recent(recent(r"C:\notes\a.md"));
    s.push_recent(recent(r"C:\notes\b.md"));
    assert!(s.remove_recent("c:/NOTES/a.md"));
    assert_eq!(recent_paths(&s), [r"C:\notes\b.md"]);
    assert!(!s.remove_recent(r"C:\notes\gone.md"));
    assert_eq!(recent_paths(&s), [r"C:\notes\b.md"]);
}

#[test]
fn recent_capped_newest_first() {
    let mut s = State::default();
    for i in 0..25 {
        s.push_recent(recent(&format!("C:\\n\\{i}.md")));
    }
    assert_eq!(s.recent.len(), 20);
    assert_eq!(s.recent[0].path, "C:\\n\\24.md");
    assert_eq!(s.recent[19].path, "C:\\n\\5.md");
    s.push_recent(recent("c:\\N\\10.md"));
    assert_eq!(s.recent.len(), 20, "same file, different case: deduped");
    assert_eq!(s.recent[0].path, "c:\\N\\10.md");
}

#[test]
fn state_roundtrips_through_json() {
    let mut s = State::default();
    s.set_position("C:\\n\\a.md", pos(0.5), 7);
    s.push_recent(recent("C:\\n\\a.md"));
    s.last_doc = Some("C:\\n\\a.md".to_owned());
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("state.json");
    write_json_atomic(&path, &s).unwrap();
    let Loaded::Ok(back) = load_json_or_default::<State>(&path) else {
        panic!("state did not load");
    };
    assert_eq!(back.position("C:\\n\\a.md").unwrap().fraction, 0.5);
    assert_eq!(recent_paths(&back), ["C:\\n\\a.md"]);
    assert_eq!(back.last_doc.as_deref(), Some("C:\\n\\a.md"));
}
