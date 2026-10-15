mod common;

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::ipc::{ConvertedPaths, PathForm, PathForms};
use lectern_core::library::pathmap::{
    page_path, page_token, page_url, path_tokens, split_line_suffix, LineRef, Mapped, PathMapper,
    PAGE_ORIGIN,
};

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
fn forms_of_a_relative_path_flip_its_separators() {
    assert_eq!(
        ubuntu().forms("app/services/x/writing_type.rb", None),
        path_forms(
            Some(r"app\services\x\writing_type.rb"),
            Some("app/services/x/writing_type.rb")
        )
    );
    assert_eq!(
        ubuntu().forms(r"`spec\a\b_spec.rb:12`", None),
        path_forms(Some(r"spec\a\b_spec.rb"), Some("spec/a/b_spec.rb"))
    );
    assert_eq!(
        ubuntu().forms("../artifacts/report.html#summary", None),
        path_forms(
            Some(r"..\artifacts\report.html"),
            Some("../artifacts/report.html")
        )
    );
    assert_eq!(
        ubuntu().forms(r"docs/My Notes\plan.md", None),
        path_forms(
            Some(r"docs\My Notes\plan.md"),
            Some("docs/My Notes/plan.md")
        )
    );
}

#[test]
fn forms_of_a_relative_path_are_never_resolved() {
    let (_tmp, root) = common::vault_copy();
    let ix = common::index_of(&root);
    // The index holds this file, but the path stays relative to wherever it was written.
    assert_eq!(
        ubuntu().forms("work/alpha/README.md", Some(&ix)),
        path_forms(Some(r"work\alpha\README.md"), Some("work/alpha/README.md"))
    );
}

#[test]
fn forms_of_a_bare_name_or_a_url_are_none() {
    for raw in [
        "notes.md",
        "`plan.md:12`",
        "",
        "https://example.com/a/b",
        "mailto:me@example.com",
        "host:/srv/notes/plan.md",
        r"C:notes\plan.md",
        r"\notes\plan.md",
    ] {
        assert_eq!(ubuntu().forms(raw, None), path_forms(None, None), "{raw:?}");
    }
}

fn tokens(text: &str) -> Vec<&str> {
    path_tokens(text).into_iter().map(|r| &text[r]).collect()
}

#[test]
fn path_tokens_are_words_or_quoted_runs_holding_a_separator() {
    assert_eq!(
        tokens("bundle exec rubocop app/services/x spec/services/x"),
        ["app/services/x", "spec/services/x"]
    );
    assert_eq!(
        tokens(r#"cat "/mnt/s/My Vault/a.md" 'b c\d' "no path" it's"#),
        ["/mnt/s/My Vault/a.md", r"b c\d"]
    );
    assert_eq!(tokens("x\t`a/b:12`,\n  c\\d"), ["`a/b:12`,", r"c\d"]);
    // A quote closes on its own line, or it's an ordinary character.
    assert_eq!(tokens("\"a/b\nc/d\""), ["\"a/b", "c/d\""]);
    // What follows a closing quote is read afresh; any Unicode space separates words.
    assert_eq!(
        tokens("\"é/a\"b/c\u{a0}d/é\u{3000}'x/y"),
        ["é/a", "b/c", "d/é", "'x/y"]
    );
    assert!(tokens("plain words, notes.md and v1.2").is_empty());
}

#[test]
fn path_tokens_take_only_a_flags_value() {
    assert_eq!(
        tokens(r#"--config=app/x.yml -f=a\b --out="/mnt/s/My Vault/x" --flag"#),
        ["app/x.yml", r"a\b", "/mnt/s/My Vault/x"]
    );
    // Without `=`, the whole word is the candidate.
    assert_eq!(tokens("-Iinclude/x"), ["-Iinclude/x"]);
}

#[test]
fn path_tokens_skip_urls() {
    assert!(tokens("curl https://example.com/a/b file:///C:/x mailto:me@example.com/x").is_empty());
    assert_eq!(tokens("see (https://example.com/a) and a/b"), ["a/b"]);
}

/// Each quoted run is scanned to its closing quote, never on to the end of its line, and no word
/// is read twice: a long line of them takes linear time. Scanning to the line's end made this
/// 600 KB line take minutes.
#[test]
fn path_tokens_read_a_long_line_of_quoted_runs_in_linear_time() {
    let line = r#""a/b" "#.repeat(100_000);
    let started = std::time::Instant::now();
    let found = path_tokens(&line);
    let elapsed = started.elapsed();
    assert_eq!(found.len(), 100_000);
    assert_eq!(&line[found[99_999].clone()], "a/b");
    assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
    // So does a long word of runs back to back, and of flags with quoted values.
    for word in [r#""a/b""#.repeat(100_000), r#"-x="a/b""#.repeat(60_000)] {
        let started = std::time::Instant::now();
        assert!(!path_tokens(&word).is_empty());
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(1), "{elapsed:?}");
    }
}

/// A path with spaces is one path to `forms`, so a user mapping applies to all of it; converted
/// word by word, its first word alone can't reach the mapping.
#[test]
fn forms_map_a_whole_path_with_spaces_that_convert_paths_splits() {
    let m = PathMapper {
        wsl_distro: Some("Ubuntu".into()),
        ..mapper(&[("/home/me/vault", r"S:\Notes\My Vault")])
    };
    let plan = r"S:\Notes\My Vault\plan.md";
    assert_eq!(
        m.forms(plan, None).wsl.as_deref(),
        Some("/home/me/vault/plan.md")
    );
    assert_eq!(
        m.convert_paths(plan, PathForm::Wsl, None).text,
        "/mnt/s/Notes/My Vault/plan.md"
    );
}

fn converted(text: &str, converted: u32, unconverted: u32) -> ConvertedPaths {
    ConvertedPaths {
        text: text.to_owned(),
        converted,
        unconverted,
    }
}

#[test]
fn convert_paths_flips_the_relative_paths_in_a_command() {
    let command = "bundle exec rubocop app/services/x spec/services/x";
    assert_eq!(
        ubuntu().convert_paths(command, PathForm::Windows, None),
        converted(r"bundle exec rubocop app\services\x spec\services\x", 2, 0)
    );
    assert_eq!(
        ubuntu().convert_paths(command, PathForm::Wsl, None),
        converted(command, 2, 0)
    );
}

#[test]
fn convert_paths_maps_absolute_paths_both_ways() {
    assert_eq!(
        ubuntu().convert_paths("cat /mnt/s/Notes/plan.md", PathForm::Windows, None),
        converted(r"cat S:\Notes\plan.md", 1, 0)
    );
    assert_eq!(
        ubuntu().convert_paths(r"cat S:\Notes\plan.md", PathForm::Wsl, None),
        converted("cat /mnt/s/Notes/plan.md", 1, 0)
    );
    assert_eq!(
        ubuntu().convert_paths("bash /home/me/run.sh", PathForm::Windows, None),
        converted(r"bash \\wsl.localhost\Ubuntu\home\me\run.sh", 1, 0)
    );
}

#[test]
fn convert_paths_keeps_quotes_around_a_quoted_path() {
    assert_eq!(
        ubuntu().convert_paths(r#""/mnt/s/My Vault/a.md""#, PathForm::Windows, None),
        converted(r#""S:\My Vault\a.md""#, 1, 0)
    );
    assert_eq!(
        ubuntu().convert_paths(r"code 'S:\My Vault\a.md' -n", PathForm::Wsl, None),
        converted("code '/mnt/s/My Vault/a.md' -n", 1, 0)
    );
}

#[test]
fn convert_paths_converts_a_flags_value_only() {
    assert_eq!(
        ubuntu().convert_paths("--config=app/x.yml", PathForm::Windows, None),
        converted(r"--config=app\x.yml", 1, 0)
    );
    assert_eq!(
        ubuntu().convert_paths(
            r#"run -f=a/b.yml --out="/mnt/s/My Vault/x""#,
            PathForm::Windows,
            None
        ),
        converted(r#"run -f=a\b.yml --out="S:\My Vault\x""#, 2, 0)
    );
}

#[test]
fn convert_paths_leaves_urls_alone() {
    let text = "curl https://example.com/a/b";
    for form in [PathForm::Windows, PathForm::Wsl] {
        assert_eq!(
            ubuntu().convert_paths(text, form, None),
            converted(text, 0, 0)
        );
    }
}

#[test]
fn convert_paths_keeps_line_breaks_indentation_and_what_surrounds_a_path() {
    let block =
        "cd app/services\n    rspec spec/a_spec.rb:12\r\n\tcat `docs/x.md`, /mnt/c/x.md#top\n";
    assert_eq!(
        ubuntu().convert_paths(block, PathForm::Windows, None),
        converted(
            "cd app\\services\n    rspec spec\\a_spec.rb:12\r\n\tcat `docs\\x.md`, C:\\x.md#top\n",
            4,
            0
        )
    );
}

#[test]
fn convert_paths_counts_a_path_with_no_form_and_leaves_it_as_written() {
    assert_eq!(
        ubuntu().convert_paths(r"\\nas\share\x.md", PathForm::Wsl, None),
        converted(r"\\nas\share\x.md", 0, 1)
    );
    assert_eq!(
        ubuntu().convert_paths(r"cp \\nas\share\x.md S:\Notes\x.md", PathForm::Wsl, None),
        converted(r"cp \\nas\share\x.md /mnt/s/Notes/x.md", 1, 1)
    );
    // A user mapping gives the share one.
    assert_eq!(
        mapper(&[("/home/me/shared", r"\\nas\share")]).convert_paths(
            r"\\nas\share\x.md",
            PathForm::Wsl,
            None
        ),
        converted("/home/me/shared/x.md", 1, 0)
    );
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

/// An open's URL names its token and the page's file name alone: the page's folder is the
/// open's, never in the URL.
#[test]
fn an_opens_url_names_its_token_and_the_pages_file_name() {
    assert_eq!(
        page_url("t1", Path::new(r"X:\a\b c.html")).as_deref(),
        Some("http://lxpage.localhost/o/t1/b%20c.html")
    );
    assert_eq!(
        page_url("t1", Path::new(r"\\host\share\x.pdf")).as_deref(),
        Some("http://lxpage.localhost/o/t1/x.pdf")
    );
    // Only RFC 3986's unreserved characters stay as they are.
    assert_eq!(
        page_url(
            "t1",
            Path::new(r"S:\Notes\C#\a #1 (draft)~v2_x-y Café 日本 100%.html")
        )
        .as_deref(),
        Some(
            "http://lxpage.localhost/o/t1/\
             a%20%231%20%28draft%29~v2_x-y%20Caf%C3%A9%20%E6%97%A5%E6%9C%AC%20100%25.html"
        )
    );
    // Either separator.
    assert_eq!(
        page_url("t1", Path::new("S:/Notes/a.pdf")).as_deref(),
        Some("http://lxpage.localhost/o/t1/a.pdf")
    );
    // A name the open's path would refuse has no URL.
    for path in [
        "",
        r"X:\",
        r"X:\a\",
        r"X:\CON.html",
        r"X:\a\work.",
        r"X:\a\a.html ",
        r"X:\a\..",
        "S:/Notes/a.html:stream",
    ] {
        assert_eq!(page_url("t1", Path::new(path)), None, "{path:?}");
    }
}

/// A request on the page protocol names an open by its token, then the path below the open's
/// folder, still encoded; its query and fragment are dropped. The old forms name none.
#[test]
fn a_page_request_names_its_open_and_the_path_below_it() {
    assert_eq!(page_token("/o/t1/report.html"), Some(("t1", "report.html")));
    assert_eq!(
        page_token("/o/0123abcd/img/a%20b.png?v=2#page=1"),
        Some(("0123abcd", "img/a%20b.png"))
    );
    assert_eq!(page_token("/o/t1/a%3Fb.html"), Some(("t1", "a%3Fb.html")));
    for url in [
        "/d/s/Notes/a.html",
        "/u/nas/share/a.pdf",
        "/o",
        "/o/",
        "/o/t1",
        "/o/t1/",
        "/o//a.html",
        "o/t1/a.html",
        "/O/t1/a.html",
        "/__csp-report",
        "",
    ] {
        assert_eq!(page_token(url), None, "{url}");
    }
}

/// The path below an open is decoded a segment at a time, once, under the open's folder, and each
/// segment is checked after decoding: nothing climbs out, switches drive or names a device.
#[test]
fn segments_under_an_opens_folder_are_decoded_once_then_checked() {
    let folder = r"S:\Notes\My Vault\artifacts";
    assert_eq!(
        page_path(folder, "report.html").as_deref(),
        Some(r"S:\Notes\My Vault\artifacts\report.html")
    );
    assert_eq!(
        page_path(folder, "img/a%20b.png").as_deref(),
        Some(r"S:\Notes\My Vault\artifacts\img\a b.png")
    );
    assert_eq!(
        page_path(folder, "C%23/100%25/Caf%C3%A9%20%E6%97%A5%E6%9C%AC.css").as_deref(),
        Some(r"S:\Notes\My Vault\artifacts\C#\100%\Café 日本.css")
    );
    assert_eq!(page_path(r"X:\", "a.html").as_deref(), Some(r"X:\a.html"));
    assert_eq!(
        page_path(r"\\nas\share", "x.pdf").as_deref(),
        Some(r"\\nas\share\x.pdf")
    );
    for rest in [
        // Climbing, as written or encoded.
        "..%2F..%2Fsecret.txt",
        "../secret.txt",
        "%2E%2E/secret.txt",
        "./a.html",
        "%2e/a.html",
        "..%5Csecret.txt",
        "..%20/a.html",
        // Another drive or a stream, smuggled into a segment.
        "%3A%5CWindows%5Cwin.ini",
        "C%3A%5Ca.html",
        "a.html%3Astream",
        "a%2Fb.html",
        // NUL, control characters and the characters Windows forbids.
        "a%00b.html",
        "%00",
        "a%0Ab.html",
        "a%1Fb.html",
        "a%3Cb.html",
        "a%3Eb.html",
        "a%22b.html",
        "a%7Cb.html",
        "a%3Fb.html",
        "a%2Ab.html",
        // Devices, and names Windows reads without their trailing dot or space.
        "CON",
        "con.html",
        "img/COM1.png",
        "AUX.css",
        "lpt%C2%B9.png",
        "work./a.html",
        "a.html.",
        "a.html%20",
        // Empty segments, and nothing at all.
        "",
        "a//b.html",
        "a.html/",
        "/a.html",
        // Not UTF-8 once decoded.
        "%FF%FE.html",
    ] {
        assert_eq!(page_path(folder, rest), None, "{rest}");
    }
    // Names a device's only look like.
    assert!(page_path(folder, "console.html").is_some());
    assert!(page_path(folder, "com10.png").is_some());
}

/// An open's URL leads back to its page, under the page's folder.
#[test]
fn an_opens_url_leads_back_to_its_page() {
    for (folder, name) in [
        (r"X:\a", "b c.html"),
        (r"S:\Notes\C#\100%", "Café 日本 #1.html"),
        (r"S:\Notes", "50% off; [x] & {y} = 'z'!.html"),
        (r"\\nas\share\notes", "x.pdf"),
        (r"X:\", "a.pdf"),
    ] {
        let page = format!(r"{}\{name}", folder.trim_end_matches('\\'));
        let url = page_url("t1", Path::new(&page)).unwrap_or_else(|| panic!("{page}"));
        let path = url.strip_prefix(PAGE_ORIGIN).unwrap();
        let (token, rest) = page_token(path).unwrap();
        assert_eq!(token, "t1");
        assert_eq!(
            page_path(folder, rest).as_deref(),
            Some(page.as_str()),
            "{page}"
        );
    }
}
