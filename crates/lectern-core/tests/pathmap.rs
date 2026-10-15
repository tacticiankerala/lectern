mod common;

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::ipc::PathForms;
use lectern_core::library::pathmap::{split_line_suffix, LineRef, Mapped, PathMapper};

fn mapper(mappings: &[(&str, &str)]) -> PathMapper {
    PathMapper {
        mappings: mappings
            .iter()
            .map(|(from, to)| ((*from).to_owned(), PathBuf::from(to)))
            .collect(),
        wsl_distro: None,
    }
}

fn unverified(path: &str) -> Mapped {
    Mapped::Unverified(PathBuf::from(path))
}

#[test]
fn line_suffixes() {
    assert_eq!(
        split_line_suffix("app/k.rb:17"),
        LineRef {
            path: "app/k.rb".into(),
            line: Some(17),
            col: None
        }
    );
    assert_eq!(split_line_suffix("a.md:3:9").col, Some(9));
    assert_eq!(split_line_suffix("a.md#L12").line, Some(12));
    assert_eq!(split_line_suffix(r"C:\x\y.md").path, r"C:\x\y.md");
}

#[test]
fn line_suffix_details() {
    assert_eq!(
        split_line_suffix("a.md:3:9"),
        LineRef {
            path: "a.md".into(),
            line: Some(3),
            col: Some(9)
        }
    );
    assert_eq!(
        split_line_suffix(r"C:\x\y.rb:12"),
        LineRef {
            path: r"C:\x\y.rb".into(),
            line: Some(12),
            col: None
        }
    );
    assert_eq!(
        split_line_suffix("/home/dev/app/k.rb:17:5"),
        LineRef {
            path: "/home/dev/app/k.rb".into(),
            line: Some(17),
            col: Some(5)
        }
    );
    for plain in ["notes.md", "a.md:", "a.md:x", "a.md#L", "a.md#intro", "C:"] {
        assert_eq!(
            split_line_suffix(plain),
            LineRef {
                path: plain.into(),
                line: None,
                col: None
            },
            "{plain}"
        );
    }
}

#[test]
fn map_user_mapping() {
    let m = mapper(&[("/home/dev/shared", r"S:\Notes\My Vault")]);
    assert_eq!(
        m.map("/home/dev/shared/x/y.md", None),
        unverified(r"S:\Notes\My Vault\x\y.md")
    );
}

#[test]
fn map_user_mapping_takes_the_longest_whole_folder_prefix() {
    let m = mapper(&[
        ("/home/dev", r"D:\Home"),
        ("/home/dev/shared/", r"S:\Shared\"),
    ]);
    assert_eq!(
        m.map("/home/dev/shared/a.md", None),
        unverified(r"S:\Shared\a.md")
    );
    assert_eq!(
        m.map("/home/dev/other.md", None),
        unverified(r"D:\Home\other.md")
    );
    assert_eq!(m.map("/home/devx/a.md", None), Mapped::Unresolved);
}

#[test]
fn map_windows_user_mapping_ignores_case_and_separators() {
    let m = mapper(&[(r"C:\Old", r"D:\New"), (r"\\nas\share\", r"S:\")]);
    for raw in [r"c:\old\a.md", "C:/Old/a.md", r"C:\OLD/a.md"] {
        assert_eq!(m.map(raw, None), unverified(r"D:\New\a.md"), "{raw}");
    }
    assert_eq!(m.map("//NAS/share/x.md", None), unverified(r"S:\x.md"));
    // Whole folders only: `C:\Older` is not under `C:\Old`.
    assert_eq!(m.map(r"c:\older\a.md", None), unverified(r"c:\older\a.md"));
}

#[test]
fn map_linux_user_mapping_is_case_sensitive() {
    let m = mapper(&[("/home/dev", r"D:\Home")]);
    assert_eq!(m.map("/home/dev/a.md", None), unverified(r"D:\Home\a.md"));
    assert_eq!(m.map("/HOME/dev/a.md", None), Mapped::Unresolved);
    assert_eq!(m.map(r"/home\dev/a.md", None), Mapped::Unresolved);
}

#[test]
fn map_mnt_drive() {
    let m = PathMapper::default();
    assert_eq!(
        m.map("/mnt/c/Users/a.txt", None),
        Mapped::Unverified(PathBuf::from(r"C:\Users\a.txt"))
    );
    assert_eq!(
        m.map("/mnt/s/Notes/My Vault/x.md", None),
        unverified(r"S:\Notes\My Vault\x.md")
    );
    assert_eq!(m.map("/mnt/wsl/x.md", None), Mapped::Unresolved);
}

#[test]
fn map_windows_paths_as_written() {
    let m = PathMapper::default();
    assert_eq!(
        m.map(r"C:\Users\a.txt", None),
        unverified(r"C:\Users\a.txt")
    );
    assert_eq!(
        m.map(r"\\nas\share\x.md", None),
        unverified(r"\\nas\share\x.md")
    );
}

#[test]
fn map_relative_paths_is_unresolved() {
    let m = PathMapper {
        wsl_distro: Some("Ubuntu".into()),
        ..PathMapper::default()
    };
    assert_eq!(m.map("notes/a.md", None), Mapped::Unresolved);
}

#[test]
fn map_library_suffix() {
    let (_tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    assert_eq!(
        PathMapper::default().map("/home/dev/shared/vault/work/alpha/README.md", Some(&ix)),
        Mapped::Verified(root.join("work/alpha/README.md"))
    );
}

#[test]
fn map_moved_file_recovery() {
    let (_tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    assert_eq!(
        PathMapper::default().map("/x/vault/work/beta/README.md", Some(&ix)),
        Mapped::Verified(root.join("archive/beta/README.md"))
    );
}

#[test]
fn map_moved_file_recovery_needs_a_unique_match() {
    let (_tmp, root) = common::vault_copy();
    for rel in ["a/shared/README.md", "b/shared/README.md"] {
        fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        fs::write(root.join(rel), "# Shared\n").unwrap();
    }
    let ix = common::index_of(&root);
    let m = PathMapper::default();
    assert_eq!(m.map("/x/shared/README.md", Some(&ix)), Mapped::Unresolved);
    // A third component makes it unique again.
    assert_eq!(
        m.map("/x/a/shared/README.md", Some(&ix)),
        Mapped::Verified(root.join("a/shared/README.md"))
    );
}

#[test]
fn map_recovers_a_moved_file_behind_a_user_mapping() {
    let (_tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    let m = mapper(&[("/home/dev/shared", r"S:\Notes\My Vault")]);
    assert_eq!(
        m.map("/home/dev/shared/vault/work/beta/README.md", Some(&ix)),
        Mapped::Verified(root.join("archive/beta/README.md"))
    );
}

#[test]
fn map_user_mapping_onto_an_indexed_file_is_verified() {
    let (_tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    let m = mapper(&[("/home/dev/shared", root.to_str().unwrap())]);
    // Mapped paths are Windows paths, joined with `\` even when the test runs on Linux.
    assert_eq!(
        m.map("/home/dev/shared/README.md", Some(&ix)),
        Mapped::Verified(PathBuf::from(format!("{}\\README.md", root.display())))
    );
}

#[test]
fn map_wsl_fallback() {
    let m = PathMapper {
        wsl_distro: Some("Ubuntu".into()),
        ..PathMapper::default()
    };
    assert_eq!(
        m.map("/home/dev/projects/app/k.rb", None),
        unverified(r"\\wsl.localhost\Ubuntu\home\dev\projects\app\k.rb")
    );
}

#[test]
fn map_without_wsl_is_unresolved() {
    assert_eq!(
        PathMapper::default().map("/home/dev/projects/app/k.rb", None),
        Mapped::Unresolved
    );
}

#[test]
fn unc_hosts_are_read_from_either_separator_and_verbatim_paths() {
    use lectern_core::library::pathmap::{unc_host, unc_host_trusted};
    assert_eq!(unc_host(r"\\NAS\Share\a.md").as_deref(), Some("nas"));
    assert_eq!(unc_host("//server/share/a.md").as_deref(), Some("server"));
    assert_eq!(
        unc_host(r"\\?\UNC\Server\share\a").as_deref(),
        Some("server")
    );
    assert_eq!(unc_host(r"\\?\C:\a").as_deref(), Some("?"));
    assert_eq!(unc_host(r"\\.\pipe\x").as_deref(), Some("."));
    assert_eq!(unc_host(r"C:\a.md"), None);
    assert_eq!(unc_host("/home/me/a.md"), None);
    assert_eq!(unc_host(r"\\"), None);
    let trusted = ["nas".to_owned(), "wsl.localhost".to_owned()];
    assert!(unc_host_trusted(r"\\NAS\Share\x.png", &trusted));
    assert!(unc_host_trusted(r"\\wsl.localhost\Ubuntu\a.md", &trusted));
    assert!(unc_host_trusted(r"S:\Notes\x.png", &trusted));
    assert!(!unc_host_trusted(r"\\attacker\s\x.png", &trusted));
    assert!(!unc_host_trusted(r"\\?\UNC\attacker\s\x.png", &trusted));
    assert!(!unc_host_trusted(r"\\.\pipe\x", &trusted));
}

#[test]
fn to_wsl_reverses_user_mappings_first() {
    let m = mapper(&[("/home/me/shared", r"S:\Shared")]);
    assert_eq!(
        m.to_wsl(Path::new(r"S:\Shared\notes\a.md")).as_deref(),
        Some("/home/me/shared/notes/a.md")
    );
    assert_eq!(
        m.to_wsl(Path::new("s:/shared/notes/a.md")).as_deref(),
        Some("/home/me/shared/notes/a.md")
    );
    // Whole folders only: `S:\SharedOld` is not under `S:\Shared`, so it falls back to `/mnt`.
    assert_eq!(
        m.to_wsl(Path::new(r"S:\SharedOld\a.md")).as_deref(),
        Some("/mnt/s/SharedOld/a.md")
    );
}

#[test]
fn to_wsl_takes_the_longest_mapped_folder() {
    let m = mapper(&[
        ("/home/me/all", r"S:\"),
        ("/home/me/shared/", r"S:\Shared\"),
    ]);
    assert_eq!(
        m.to_wsl(Path::new(r"S:\Shared\a.md")).as_deref(),
        Some("/home/me/shared/a.md")
    );
    assert_eq!(
        m.to_wsl(Path::new(r"S:\Other\a.md")).as_deref(),
        Some("/home/me/all/Other/a.md")
    );
}

#[test]
fn to_wsl_reverses_only_mappings_from_linux_paths() {
    // On a tie the last mapping would win, so the Linux one goes first.
    let m = mapper(&[
        ("/home/me/shared", r"S:\Shared"),
        (r"C:\Old", r"D:\New"),
        (r"\\nas\share", r"S:\Shared"),
        ("//nas/share", r"S:\Shared"),
    ]);
    assert_eq!(
        m.to_wsl(Path::new(r"D:\New\a.md")).as_deref(),
        Some("/mnt/d/New/a.md"),
        "a Windows source isn't a WSL path"
    );
    assert_eq!(
        m.to_wsl(Path::new(r"S:\Shared\a.md")).as_deref(),
        Some("/home/me/shared/a.md"),
        "the Linux source wins over UNC ones onto the same folder"
    );
}

#[test]
fn to_wsl_handles_wsl_unc_and_drives() {
    let m = mapper(&[]);
    assert_eq!(
        m.to_wsl(Path::new(r"\\wsl.localhost\Ubuntu\home\me\a.md"))
            .as_deref(),
        Some("/home/me/a.md")
    );
    assert_eq!(
        m.to_wsl(Path::new(r"\\wsl$\Ubuntu\home\me\a.md"))
            .as_deref(),
        Some("/home/me/a.md")
    );
    assert_eq!(
        m.to_wsl(Path::new(r"C:\Users\me\a.md")).as_deref(),
        Some("/mnt/c/Users/me/a.md")
    );
}

#[test]
fn to_wsl_gives_none_for_other_unc() {
    assert_eq!(mapper(&[]).to_wsl(Path::new(r"\\nas\share\a.md")), None);
}

fn ubuntu() -> PathMapper {
    PathMapper {
        wsl_distro: Some("Ubuntu".into()),
        ..PathMapper::default()
    }
}

fn path_forms(windows: Option<&str>, wsl: Option<&str>) -> PathForms {
    PathForms {
        windows: windows.map(str::to_owned),
        wsl: wsl.map(str::to_owned),
    }
}

#[test]
fn forms_of_a_drive_path_and_its_mount() {
    let plan = path_forms(
        Some(r"S:\Notes\My Vault\plan.md"),
        Some("/mnt/s/Notes/My Vault/plan.md"),
    );
    assert_eq!(ubuntu().forms(r"S:\Notes\My Vault\plan.md", None), plan);
    assert_eq!(ubuntu().forms("/mnt/s/Notes/My Vault/plan.md", None), plan);
}

#[test]
fn forms_of_a_wsl_share_path() {
    for raw in [
        r"\\wsl.localhost\Ubuntu\home\me\a.md",
        r"\\wsl$\Ubuntu\home\me\a.md",
    ] {
        assert_eq!(
            ubuntu().forms(raw, None),
            path_forms(Some(raw), Some("/home/me/a.md")),
            "{raw}"
        );
    }
}

#[test]
fn forms_of_a_linux_path_go_through_the_distro() {
    assert_eq!(
        ubuntu().forms("/home/me/a.md", None),
        path_forms(
            Some(r"\\wsl.localhost\Ubuntu\home\me\a.md"),
            Some("/home/me/a.md")
        )
    );
}

#[test]
fn forms_follow_a_user_mapping_both_ways() {
    let m = mapper(&[("/home/me/shared", r"\\nas\share")]);
    let x = path_forms(Some(r"\\nas\share\x.md"), Some("/home/me/shared/x.md"));
    assert_eq!(m.forms("/home/me/shared/x.md", None), x);
    assert_eq!(m.forms(r"\\nas\share\x.md", None), x);
}

#[test]
fn forms_of_an_unmapped_share_have_no_wsl_form() {
    assert_eq!(
        ubuntu().forms(r"\\nas\share\x.md", None),
        path_forms(Some(r"\\nas\share\x.md"), None)
    );
}

#[test]
fn forms_drop_line_suffixes_anchors_quotes_and_backticks() {
    let plan = path_forms(Some(r"S:\Notes\plan.md"), Some("/mnt/s/Notes/plan.md"));
    for raw in [
        r"`S:\Notes\plan.md:12`",
        r"S:\Notes\plan.md:12:5",
        r"S:\Notes\plan.md#L12",
        r"S:\Notes\plan.md#next-steps",
        r#"  "S:\Notes\plan.md"  "#,
        "'S:\\Notes\\plan.md'\n",
    ] {
        assert_eq!(ubuntu().forms(raw, None), plan, "{raw:?}");
    }
    // A `#` inside a folder name is part of the path.
    assert_eq!(
        ubuntu().forms(r"C:\src\C#\app.cs", None),
        path_forms(Some(r"C:\src\C#\app.cs"), Some("/mnt/c/src/C#/app.cs"))
    );
}

#[test]
fn forms_keep_a_hash_in_a_file_name() {
    // Only a fragment after a file name with an extension is an anchor.
    let meeting = path_forms(
        Some(r"S:\Notes\Meeting #3.md"),
        Some("/mnt/s/Notes/Meeting #3.md"),
    );
    assert_eq!(ubuntu().forms(r"S:\Notes\Meeting #3.md", None), meeting);
    assert_eq!(
        ubuntu().forms(r"S:\Notes\Meeting #3.md#agenda", None),
        meeting
    );
    assert_eq!(
        ubuntu().forms("/home/me/Issue #12 fix.md", None),
        path_forms(
            Some(r"\\wsl.localhost\Ubuntu\home\me\Issue #12 fix.md"),
            Some("/home/me/Issue #12 fix.md")
        )
    );
}

#[test]
fn forms_of_a_relative_path_are_none() {
    for raw in ["notes/plan.md", "`plan.md:12`", ""] {
        assert_eq!(ubuntu().forms(raw, None), path_forms(None, None), "{raw:?}");
    }
}

#[test]
fn forms_come_from_the_index_in_memory_never_the_disk() {
    let (tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    // The files are gone; the index still knows them, and nothing looks at the disk.
    drop(tmp);
    assert!(!root.exists());
    let forms = ubuntu().forms("/home/me/shared/vault/work/alpha/README.md", Some(&ix));
    assert_eq!(
        forms.windows,
        Some(
            common::native_join(&root, "work/alpha/README.md")
                .to_string_lossy()
                .into_owned()
        )
    );
}

#[test]
fn literal_forms_never_recover_a_moved_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Notes");
    fs::create_dir_all(root.join("project")).unwrap();
    fs::write(root.join("project").join("README.md"), "# Project\n").unwrap();
    let ix = common::index_of(&root);
    let archived = r"C:\Notes\.archive\project\README.md";
    // As text, a path the index doesn't hold finds where the file moved to...
    assert_eq!(
        ubuntu().forms(archived, Some(&ix)).windows,
        Some(
            common::native_join(&root, "project/README.md")
                .to_string_lossy()
                .into_owned()
        )
    );
    // ...but an exact path, such as the open file outside the index, is copied as it is.
    assert_eq!(
        ubuntu().literal_forms(archived),
        path_forms(
            Some(archived),
            Some("/mnt/c/Notes/.archive/project/README.md")
        )
    );
}

#[test]
fn literal_forms_keep_hashes_and_line_numbers_in_names() {
    for (path, wsl) in [
        (
            r"S:\Notes\plan.md#draft.md",
            "/mnt/s/Notes/plan.md#draft.md",
        ),
        (r"S:\Notes\v1.0#archive", "/mnt/s/Notes/v1.0#archive"),
        (
            r"S:\Notes\v1.0#archive\x.md",
            "/mnt/s/Notes/v1.0#archive/x.md",
        ),
    ] {
        assert_eq!(
            ubuntu().literal_forms(path),
            path_forms(Some(path), Some(wsl)),
            "{path}"
        );
    }
    assert_eq!(
        ubuntu().literal_forms("/home/me/logs/run:12"),
        path_forms(
            Some(r"\\wsl.localhost\Ubuntu\home\me\logs\run:12"),
            Some("/home/me/logs/run:12")
        )
    );
    // Text written as a path still loses its line anchor.
    assert_eq!(
        ubuntu().forms(r"S:\Notes\plan.md#L12", None),
        path_forms(Some(r"S:\Notes\plan.md"), Some("/mnt/s/Notes/plan.md"))
    );
}

#[test]
fn literal_forms_map_linux_and_mounted_paths_without_the_index() {
    let m = PathMapper {
        wsl_distro: Some("Ubuntu".into()),
        ..mapper(&[("/home/me/shared", r"\\nas\share")])
    };
    assert_eq!(
        m.literal_forms("/home/me/shared/x.md"),
        path_forms(Some(r"\\nas\share\x.md"), Some("/home/me/shared/x.md"))
    );
    assert_eq!(
        m.literal_forms("/mnt/s/Notes/plan.md"),
        path_forms(Some(r"S:\Notes\plan.md"), Some("/mnt/s/Notes/plan.md"))
    );
    assert_eq!(m.literal_forms("notes/plan.md"), path_forms(None, None));
}
