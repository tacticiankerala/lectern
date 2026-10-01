//! The IPC contract: JSON shapes of the tagged enums, and the TypeScript bindings ts-rs writes to
//! `ui/src/generated/` while `cargo test` runs.

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::ipc::{
    DocPayload, EditorPref, FollowResult, Measure, OpenError, OpenErrorKind, OpenResult,
    RecentEntry, RootState, SettingsPatch, StartupPayload,
};
use serde_json::json;
use ts_rs::{Config, TS};

fn generated_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/generated")
}

fn decl<T: TS>() -> String {
    T::decl(&Config::from_env())
}

#[test]
fn export_config_comes_from_cargo_config() {
    let dir = std::env::var("TS_RS_EXPORT_DIR")
        .expect("TS_RS_EXPORT_DIR unset: run cargo inside the repo so .cargo/config.toml applies");
    assert_eq!(
        fs::canonicalize(dir).unwrap(),
        fs::canonicalize(generated_dir()).unwrap()
    );
    assert_eq!(std::env::var("TS_RS_LARGE_INT").as_deref(), Ok("number"));
}

#[test]
fn ts_bindings_exported() {
    let dir = generated_dir();
    for name in [
        "Settings",
        "SettingsPatch",
        "Measure",
        "EditorPref",
        "OpenResult",
        "DocPayload",
        "StartupPayload",
        "LibraryPayload",
        "RootState",
        "FollowResult",
        "TreeNode",
        "OutlineItem",
        "Frontmatter",
        "PropValue",
    ] {
        assert!(
            dir.join(format!("{name}.ts")).is_file(),
            "{name}.ts missing from {}",
            dir.display()
        );
    }
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("bigint"),
            "{} uses bigint, which JSON cannot carry",
            path.display()
        );
    }
}

#[test]
fn ts_unions_follow_the_serde_tags() {
    assert_eq!(decl::<Measure>(), r#"type Measure = "full" | number;"#);
    assert_eq!(
        decl::<EditorPref>(),
        r#"type EditorPref = { "mode": "auto" } | { "mode": "custom", command: string, };"#
    );
    assert_eq!(
        decl::<OpenResult>(),
        r#"type OpenResult = { "status": "ok", doc: DocPayload, } | { "status": "err", error: OpenError, };"#
    );
    assert_eq!(
        decl::<RootState>(),
        r#"type RootState = { "state": "scanning" } | { "state": "ready" } | { "state": "unavailable", reason: string, };"#
    );
    assert_eq!(
        decl::<FollowResult>(),
        r#"type FollowResult = { "action": "openDoc", path: string, anchor: string | null, line: number | null, } | { "action": "opened" } | { "action": "notFound", message: string, };"#
    );
}

#[test]
fn ts_numbers_are_numbers_and_patches_are_partial() {
    assert!(decl::<DocPayload>().contains("mtimeMs: number,"));
    assert!(decl::<RecentEntry>().contains("openedMs: number,"));
    assert!(decl::<SettingsPatch>().contains("fontSize?: number | null,"));
}

#[test]
fn open_result_is_tagged_by_status() {
    let r = OpenResult::Err {
        error: OpenError {
            kind: OpenErrorKind::NotFound,
            message: "gone".to_owned(),
            path: "C:\\n\\a.md".to_owned(),
        },
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"status": "err", "error": {"kind": "notFound", "message": "gone", "path": "C:\\n\\a.md"}})
    );
}

#[test]
fn root_state_is_tagged_by_state() {
    assert_eq!(
        serde_json::to_value(RootState::Scanning).unwrap(),
        json!({"state": "scanning"})
    );
    assert_eq!(
        serde_json::to_value(RootState::Unavailable {
            reason: "timed out".to_owned()
        })
        .unwrap(),
        json!({"state": "unavailable", "reason": "timed out"})
    );
}

#[test]
fn follow_result_is_tagged_by_action() {
    assert_eq!(
        serde_json::to_value(FollowResult::OpenDoc {
            path: "C:\\n\\b.md".to_owned(),
            anchor: Some("intro".to_owned()),
            line: None,
        })
        .unwrap(),
        json!({"action": "openDoc", "path": "C:\\n\\b.md", "anchor": "intro", "line": null})
    );
    assert_eq!(
        serde_json::to_value(FollowResult::Opened).unwrap(),
        json!({"action": "opened"})
    );
}

#[test]
fn startup_payload_carries_a_notice() {
    let j = serde_json::to_value(StartupPayload {
        settings: Default::default(),
        library: lectern_core::ipc::LibraryPayload { roots: vec![] },
        recent: vec![],
        initial: None,
        version: "0.1.0".to_owned(),
        portable: false,
        startup_notice: Some("Settings were reset".to_owned()),
    })
    .unwrap();
    assert_eq!(j["startupNotice"], "Settings were reset");
    assert!(j["initial"].is_null());
    assert_eq!(j["settings"]["measure"], 72);
}
