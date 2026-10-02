mod common;

use std::fs;
use std::path::PathBuf;

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
