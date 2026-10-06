mod common;

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use lectern_core::library::ignore::is_ignored;
use lectern_core::library::scan::{
    probe_root, probe_with, read_heads, scan_root, ScanOptions, ADHOC_MAX_FILES, ROOT_MAX_FILES,
};
use lectern_core::library::snapshot::{load_snapshot, save_snapshot, snapshot_path};
use lectern_core::library::{FileEntry, LibraryIndex, RootIndex};

fn scan(root: &Path) -> RootIndex {
    scan_root(root, &ScanOptions::default()).unwrap()
}

fn rels(r: &RootIndex) -> Vec<&str> {
    r.files.iter().map(|f| f.rel.as_str()).collect()
}

fn entry<'a>(r: &'a RootIndex, rel: &str) -> &'a FileEntry {
    r.files
        .iter()
        .find(|f| f.rel == rel)
        .unwrap_or_else(|| panic!("{rel} not scanned; got {:?}", rels(r)))
}

#[test]
fn ignores_junk() {
    for (n, d) in [
        ("._README.md", false),
        (".DS_Store", false),
        ("Thumbs.db", false),
        ("desktop.ini", false),
        (".git", true),
        ("node_modules", true),
        ("#recycle", true),
        ("#snapshot", true),
        ("@eaDir", true),
        (".obsidian", true),
    ] {
        assert!(is_ignored(n, d), "{n}");
    }
    assert!(!is_ignored("README.md", false));
}

#[test]
fn ignore_rules_cover_windows_names_case_insensitively() {
    for (n, d) in [
        ("$RECYCLE.BIN", true),
        ("$Recycle.Bin", true),
        ("System Volume Information", true),
        ("Desktop.ini", false),
        ("thumbs.db", false),
        ("._anything", true),
    ] {
        assert!(is_ignored(n, d), "{n}");
    }
}

#[test]
fn ignore_rules_keep_hidden_files_and_ordinary_dirs() {
    assert!(!is_ignored(".gitkeep", false));
    assert!(!is_ignored("notes", true));
    assert!(!is_ignored("work", true));
    // Directory-only names are not ignored as files.
    assert!(!is_ignored("node_modules", false));
}

#[test]
fn ignore_rules_skip_lectern_temp_files() {
    assert!(is_ignored(".plan.review.md.lectern.tmp", false));
    assert!(is_ignored(".Plan.Review.md.LECTERN.TMP", false));
    assert!(!is_ignored("plan.lectern.tmp.md", false));
    assert!(!is_ignored("plan.tmp", false));
    assert!(!is_ignored("plan.review.md", false));
}

#[test]
fn scan_finds_md_and_other_files() {
    let (_tmp, vault) = common::vault_copy();
    let r = scan(&vault);
    assert_eq!(r.root, vault);
    assert!(entry(&r, "work/alpha/README.md").is_md);
    assert!(!entry(&r, "friends/img/logo.png").is_md);
    assert!(entry(&r, "notes/résumé notes.md").is_md);
    let all = rels(&r);
    assert!(
        !all.iter().any(|rel| rel.contains("._README.md")),
        "{all:?}"
    );
    assert!(!all.iter().any(|rel| rel.contains(".DS_Store")), "{all:?}");
    assert!(!all.contains(&"node_modules/x.md"), "{all:?}");
    // The walk reads no file contents.
    assert!(r
        .files
        .iter()
        .all(|f| f.fm_name.is_none() && f.fm_status.is_none()));
    assert!(r.scanned_at_ms > 0);
}

#[test]
fn scan_records_size_and_mtime() {
    let (_tmp, vault) = common::vault_copy();
    let r = scan(&vault);
    let f = entry(&r, "work/alpha/README.md");
    let meta = fs::metadata(vault.join("work/alpha/README.md")).unwrap();
    assert_eq!(f.size, meta.len());
    assert!(f.mtime_ms > 0);
}

#[test]
fn scan_prunes_ignored_dirs_and_keeps_hidden_files() {
    let (_tmp, vault) = common::vault_copy();
    for dir in [".obsidian", ".git", "#recycle", "work/@eaDir"] {
        fs::create_dir_all(vault.join(dir)).unwrap();
        fs::write(vault.join(dir).join("note.md"), "# hidden\n").unwrap();
    }
    fs::write(vault.join("work/.gitkeep"), "").unwrap();
    let r = scan(&vault);
    let all = rels(&r);
    for rel in [
        ".obsidian/note.md",
        ".git/note.md",
        "#recycle/note.md",
        "work/@eaDir/note.md",
    ] {
        assert!(!all.contains(&rel), "{rel} should be ignored: {all:?}");
    }
    assert!(all.contains(&"work/.gitkeep"), "{all:?}");
}

#[test]
fn scan_treats_every_markdown_extension_case_insensitively() {
    let (_tmp, vault) = common::vault_copy();
    for (name, text) in [
        ("LOUD.MD", "# loud\n"),
        ("long.markdown", "# long\n"),
        ("old.mdown", "# old\n"),
        ("SHORT.MKD", "# short\n"),
        ("plain.txt", "plain\n"),
    ] {
        fs::write(vault.join(name), text).unwrap();
    }
    let r = scan(&vault);
    for md in ["LOUD.MD", "long.markdown", "old.mdown", "SHORT.MKD"] {
        assert!(entry(&r, md).is_md, "{md}");
    }
    assert!(!entry(&r, "plain.txt").is_md);
}

#[test]
fn scan_errs_on_missing_or_file_root() {
    let (_tmp, vault) = common::vault_copy();
    let kind = |root: &Path, max_files| {
        scan_root(root, &ScanOptions { max_files })
            .unwrap_err()
            .kind()
    };
    // The walk's own read of the root reports a missing root, even with no room for files.
    assert_eq!(
        kind(&vault.join("missing"), 20_000),
        io::ErrorKind::NotFound
    );
    assert_eq!(kind(&vault.join("missing"), 0), io::ErrorKind::NotFound);
    assert_eq!(
        kind(&vault.join("README.md"), 20_000),
        io::ErrorKind::NotADirectory
    );
}

#[test]
fn scan_errs_when_the_root_vanishes_before_the_walk() {
    // A share that disconnects after the root was added looks like a root that is gone by the
    // time the walk starts; it must fail, not come back as an empty library.
    let (_tmp, vault) = common::vault_copy();
    let scanned = scan(&vault);
    assert!(!scanned.files.is_empty());
    fs::remove_dir_all(&vault).unwrap();
    assert_eq!(
        scan_root(&vault, &ScanOptions::default())
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[cfg(unix)]
#[test]
fn scan_errs_on_a_root_that_is_a_broken_link() {
    let (tmp, vault) = common::vault_copy();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&vault, &link).unwrap();
    assert!(!scan(&link).files.is_empty());
    fs::remove_dir_all(&vault).unwrap();
    assert!(scan_root(&link, &ScanOptions::default()).is_err());
}

#[cfg(unix)]
#[test]
fn scan_errs_on_an_unlistable_root_but_skips_unlistable_folders() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, vault) = common::vault_copy();
    let locked = vault.join("work/alpha");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let below = scan_root(&vault, &ScanOptions::default());
    let at_root = scan_root(&locked, &ScanOptions::default());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let r = below.unwrap();
    assert!(r.get("work/alpha/README.md").is_none());
    assert!(r.get("archive/beta/README.md").is_some());
    assert_eq!(at_root.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn ad_hoc_roots_are_capped_at_20_000_files_and_library_roots_at_200_000() {
    assert_eq!(ADHOC_MAX_FILES, 20_000);
    assert_eq!(ROOT_MAX_FILES, 200_000);
    assert_eq!(ScanOptions::for_root(true).max_files, ADHOC_MAX_FILES);
    assert_eq!(ScanOptions::for_root(false).max_files, ROOT_MAX_FILES);
    assert_eq!(ScanOptions::default().max_files, ROOT_MAX_FILES);
}

/// A code-heavy folder past the ad-hoc cap: as an ad-hoc root it stops at 20,000 files and says
/// so; as a library root every file is indexed.
#[test]
fn a_library_root_indexes_past_the_ad_hoc_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let total = ADHOC_MAX_FILES + 1;
    for dir in 0..10 {
        fs::create_dir(root.join(format!("src{dir}"))).unwrap();
    }
    for n in 0..total {
        fs::File::create(root.join(format!("src{}/f{n}.rs", n % 10))).unwrap();
    }
    let adhoc = scan_root(root, &ScanOptions::for_root(true)).unwrap();
    assert_eq!(adhoc.files.len(), ADHOC_MAX_FILES);
    assert!(adhoc.truncated);
    let library = scan_root(root, &ScanOptions::for_root(false)).unwrap();
    assert_eq!(library.files.len(), total);
    assert!(!library.truncated);
}

#[test]
fn max_files_cap() {
    let (_tmp, vault) = common::vault_copy();
    let r = scan_root(&vault, &ScanOptions { max_files: 3 }).unwrap();
    assert_eq!(r.files.len(), 3);
    assert!(r.truncated);
    assert!(!scan(&vault).truncated);
}

#[test]
fn a_truncated_index_stays_truncated_in_its_snapshot() {
    let (tmp, vault) = common::vault_copy();
    let dir = tmp.path().join("snapshots");
    let r = scan_root(&vault, &ScanOptions { max_files: 3 }).unwrap();
    save_snapshot(&dir, &r).unwrap();
    assert!(load_snapshot(&dir, &vault).unwrap().truncated);
    // A snapshot from before the flag existed loads as complete.
    let older = serde_json::json!({ "root": vault, "files": [], "scanned_at_ms": 1 });
    fs::write(snapshot_path(&dir, &vault), older.to_string()).unwrap();
    assert!(!load_snapshot(&dir, &vault).unwrap().truncated);
}

#[test]
fn scan_stops_at_the_cap_before_later_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("a.md"), "# a\n").unwrap();
    for dir in ["b", "c/deeper"] {
        fs::create_dir_all(root.join(dir)).unwrap();
        fs::write(root.join(dir).join("note.md"), "# n\n").unwrap();
    }
    let r = scan_root(root, &ScanOptions { max_files: 1 }).unwrap();
    assert_eq!(rels(&r), ["a.md"]);
    let r = scan_root(root, &ScanOptions { max_files: 0 }).unwrap();
    assert!(r.files.is_empty());
}

#[test]
fn heads_collect_name_and_status() {
    let (_tmp, vault) = common::vault_copy();
    let mut r = scan(&vault);
    read_heads(&mut r);
    assert_eq!(
        entry(&r, "memory/feedback_incremental_prs.md")
            .fm_name
            .as_deref(),
        Some("feedback-incremental-prs")
    );
    assert_eq!(
        entry(&r, "work/alpha/README.md").fm_status.as_deref(),
        Some("blocked")
    );
    // A trailing YAML comment is not part of the value.
    assert_eq!(entry(&r, "README.md").fm_status.as_deref(), Some("active"));
    assert_eq!(entry(&r, "prompts/writer.md").fm_status, None);
}

#[test]
fn heads_accept_crlf_and_bom_but_skip_unterminated_and_late_frontmatter() {
    let (_tmp, vault) = common::vault_copy();
    fs::write(
        vault.join("crlf.md"),
        "---\r\nname: crlf-note\r\n---\r\n# C\r\n",
    )
    .unwrap();
    fs::write(
        vault.join("bom.md"),
        "\u{feff}---\nstatus: done\n---\n# B\n",
    )
    .unwrap();
    let long_value = "x".repeat(5000);
    fs::write(
        vault.join("unterminated.md"),
        format!("---\nname: too-long\nsummary: {long_value}\n---\n"),
    )
    .unwrap();
    fs::write(vault.join("late.md"), "\n---\nname: late\n---\n").unwrap();
    fs::write(vault.join("empty.md"), "---\n---\n# E\n").unwrap();
    fs::write(vault.join("notes.txt"), "---\nname: not-markdown\n---\n").unwrap();
    let mut r = scan(&vault);
    read_heads(&mut r);
    assert_eq!(entry(&r, "crlf.md").fm_name.as_deref(), Some("crlf-note"));
    assert_eq!(entry(&r, "bom.md").fm_status.as_deref(), Some("done"));
    assert_eq!(entry(&r, "unterminated.md").fm_name, None);
    assert_eq!(entry(&r, "late.md").fm_name, None);
    assert_eq!(entry(&r, "empty.md").fm_name, None);
    assert_eq!(entry(&r, "notes.txt").fm_name, None);
}

#[test]
fn heads_read_a_sidecar_note_and_its_open_comments() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let comment = |id: u32, status: &str| {
        format!("## C{id} · {status} · L1 · Notes\n> quoted\n\n**You:** A remark.\n\n")
    };
    let head = "---\nlectern-review: 1\nnote: plan.md\n---\n# Review: plan.md\n\n";
    let small = format!("{head}{}{}", comment(1, "open"), comment(2, "dismissed"));
    // Longer than a head, so it is read in full; the open comment comes after the first 4 KiB.
    let padding = format!("Notes on the plan.\n{}\n", "x".repeat(5000));
    let long = format!(
        "{head}{padding}{}{}",
        comment(1, "resolved"),
        comment(2, "question")
    );
    let huge = format!(
        "{head}{}{}",
        "y".repeat(2 * 1024 * 1024),
        comment(1, "open")
    );
    for (name, text) in [
        ("small.review.md", small.as_str()),
        ("long.review.md", long.as_str()),
        ("huge.review.md", huge.as_str()),
        ("plain.review.md", "# Not a sidecar\n"),
        ("plan.md", head),
    ] {
        fs::write(root.join(name), text).unwrap();
    }
    let mut r = scan(root);
    read_heads(&mut r);
    let fields = |rel| {
        let f = entry(&r, rel);
        (f.review_of.as_deref(), f.review_open)
    };
    assert_eq!(fields("small.review.md"), (Some("plan.md"), Some(1)));
    assert_eq!(fields("long.review.md"), (Some("plan.md"), Some(1)));
    // Over 2 MB, so the comments aren't counted.
    assert_eq!(fields("huge.review.md"), (Some("plan.md"), None));
    assert_eq!(fields("plain.review.md"), (None, None));
    // Only names that end in `.review.md` are read as sidecars.
    assert_eq!(fields("plan.md"), (None, None));
}

#[test]
fn heads_skip_files_that_vanished_since_the_scan() {
    let (_tmp, vault) = common::vault_copy();
    let mut r = scan(&vault);
    fs::remove_file(vault.join("work/alpha/README.md")).unwrap();
    read_heads(&mut r);
    assert_eq!(entry(&r, "work/alpha/README.md").fm_status, None);
    assert_eq!(
        entry(&r, "archive/beta/README.md").fm_status.as_deref(),
        Some("done")
    );
}

#[test]
fn root_lookups_are_case_insensitive() {
    let (_tmp, vault) = common::vault_copy();
    let mut r = scan(&vault);
    read_heads(&mut r);
    assert_eq!(
        r.get("WORK/Alpha/readme.md").unwrap().rel,
        "work/alpha/README.md"
    );
    assert_eq!(
        r.get("work\\alpha\\README.md").unwrap().rel,
        "work/alpha/README.md"
    );
    assert!(r.get("work/alpha/missing.md").is_none());
    let by_stem: Vec<_> = r
        .with_stem("Feedback_Incremental_PRs")
        .map(|f| &f.rel)
        .collect();
    assert_eq!(by_stem, ["memory/feedback_incremental_prs.md"]);
    let by_name: Vec<_> = r
        .with_fm_name("FEEDBACK-incremental-prs")
        .map(|f| &f.rel)
        .collect();
    assert_eq!(by_name, ["memory/feedback_incremental_prs.md"]);
    // Stems are indexed for Markdown files only.
    assert_eq!(r.with_stem("logo").count(), 0);
}

#[test]
fn md_files_and_abs() {
    let (_tmp, vault) = common::vault_copy();
    let r = scan(&vault);
    assert!(r.md_files().all(|f| f.is_md));
    assert!(r.md_files().any(|f| f.rel == "work/alpha/README.md"));
    assert!(!r.md_files().any(|f| f.rel == "friends/img/logo.png"));
    assert_eq!(
        r.abs("work/alpha/README.md"),
        vault.join("work").join("alpha").join("README.md")
    );
}

#[test]
fn library_root_for_picks_the_longest_root_case_insensitively() {
    let (_tmp, vault) = common::vault_copy();
    let mut lib = LibraryIndex::default();
    lib.upsert_root(scan(&vault));
    lib.upsert_root(scan(&vault.join("work")));
    let in_work = vault.join("work/alpha/README.md");
    assert_eq!(lib.root_for(&in_work).unwrap().root, vault.join("work"));
    let shouted = Path::new(&in_work.to_string_lossy().to_uppercase()).to_path_buf();
    assert_eq!(lib.root_for(&shouted).unwrap().root, vault.join("work"));
    assert_eq!(lib.root_for(&vault.join("README.md")).unwrap().root, vault);
    assert_eq!(lib.root_for(&vault).unwrap().root, vault);
    // A sibling whose name merely starts with the root's name is outside it.
    let sibling = vault.with_file_name("vault-other/README.md");
    assert!(lib.root_for(&sibling).is_none());
}

#[test]
fn library_contains_indexed_files_only() {
    let (_tmp, vault) = common::vault_copy();
    let mut lib = LibraryIndex::default();
    lib.upsert_root(scan(&vault));
    assert!(lib.contains(&vault.join("work/alpha/README.md")));
    assert!(lib.contains(&vault.join("WORK/ALPHA/readme.md")));
    assert!(!lib.contains(&vault.join("work/alpha/missing.md")));
    assert!(!lib.contains(&vault.join("node_modules/x.md")));
    assert!(!lib.contains(Path::new("/elsewhere/README.md")));
}

#[test]
fn library_contains_checks_every_root_that_holds_the_path() {
    let (_tmp, vault) = common::vault_copy();
    let mut lib = LibraryIndex::default();
    lib.upsert_root(scan(&vault));
    let work = vault.join("work");
    let capped = scan_root(&work, &ScanOptions { max_files: 1 }).unwrap();
    assert_eq!(rels(&capped), ["alpha/README.md"]);
    lib.upsert_root(capped);
    let plan = vault.join("work/alpha/plans/2026-01-01-big-plan.md");
    // The capped inner root is still the one `root_for` picks…
    assert_eq!(lib.root_for(&plan).unwrap().root, work);
    // …but the parent root has the file.
    assert!(lib.contains(&plan));
    assert!(lib.contains(&vault.join("work/alpha/README.md")));
    assert!(!lib.contains(&vault.join("work/alpha/plans/missing.md")));
}

#[test]
fn upsert_root_replaces_the_same_root() {
    let (_tmp, vault) = common::vault_copy();
    let mut lib = LibraryIndex::default();
    lib.upsert_root(scan_root(&vault, &ScanOptions { max_files: 1 }).unwrap());
    let shouted = Path::new(&vault.to_string_lossy().to_uppercase()).to_path_buf();
    let mut again = scan(&vault);
    again.root = shouted;
    lib.upsert_root(again);
    assert_eq!(lib.roots.len(), 1);
    assert!(lib.roots[0].files.len() > 1);
    lib.upsert_root(scan(&vault.join("work")));
    assert_eq!(lib.roots.len(), 2);
}

#[test]
fn snapshot_roundtrip() {
    let (tmp, vault) = common::vault_copy();
    let dir = tmp.path().join("snapshots");
    let mut r = scan(&vault);
    read_heads(&mut r);
    save_snapshot(&dir, &r).unwrap();
    let loaded = load_snapshot(&dir, &vault).unwrap();
    assert_eq!(loaded.files.len(), r.files.len());
    assert_eq!(rels(&loaded), rels(&r));
    assert_eq!(loaded.scanned_at_ms, r.scanned_at_ms);
    // Loading finalizes, so lookups work straight away.
    assert_eq!(
        loaded
            .get("work/alpha/readme.md")
            .unwrap()
            .fm_status
            .as_deref(),
        Some("blocked")
    );
}

#[test]
fn snapshot_save_replaces_and_leaves_no_temp_files() {
    let (tmp, vault) = common::vault_copy();
    let dir = tmp.path().join("snapshots");
    save_snapshot(
        &dir,
        &scan_root(&vault, &ScanOptions { max_files: 1 }).unwrap(),
    )
    .unwrap();
    let full = scan(&vault);
    save_snapshot(&dir, &full).unwrap();
    assert_eq!(
        load_snapshot(&dir, &vault).unwrap().files.len(),
        full.files.len()
    );
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn snapshot_path_is_stable_and_case_insensitive() {
    let dir = Path::new("/data");
    let a = snapshot_path(dir, Path::new("/Shared/Dev"));
    assert_eq!(a, snapshot_path(dir, Path::new("/shared/dev/")));
    assert_eq!(a, snapshot_path(dir, Path::new("\\shared\\dev")));
    assert_ne!(a, snapshot_path(dir, Path::new("/shared/other")));
    assert_eq!(a.parent(), Some(dir));
    let name = a.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("index-") && name.ends_with(".json"),
        "{name}"
    );
}

#[test]
fn snapshot_load_tolerates_missing_corrupt_and_foreign_files() {
    let (tmp, vault) = common::vault_copy();
    let dir = tmp.path().join("snapshots");
    assert!(load_snapshot(&dir, &vault).is_none());
    fs::create_dir_all(&dir).unwrap();
    fs::write(snapshot_path(&dir, &vault), b"{ not json").unwrap();
    assert!(load_snapshot(&dir, &vault).is_none());
    // A snapshot whose recorded root differs (a hash collision) is not used.
    let work = vault.join("work");
    save_snapshot(&dir, &scan(&work)).unwrap();
    fs::rename(snapshot_path(&dir, &work), snapshot_path(&dir, &vault)).unwrap();
    assert!(load_snapshot(&dir, &vault).is_none());
}

#[test]
fn probe_times_out_on_slow_fs() {
    let t = Instant::now();
    let r = probe_with(
        || {
            std::thread::sleep(Duration::from_secs(10));
            Ok(())
        },
        Duration::from_millis(200),
    );
    assert!(r.is_err() && t.elapsed() < Duration::from_secs(1));
}

#[test]
fn probe_reports_lister_errors() {
    let r = probe_with(
        || Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
        Duration::from_secs(1),
    );
    assert!(r.unwrap_err().contains("denied"));
}

#[test]
fn probe_missing_root_errs() {
    assert!(probe_root(Path::new("/definitely/not/here"), Duration::from_secs(1)).is_err());
}

#[test]
fn probe_existing_root_ok() {
    let (_tmp, vault) = common::vault_copy();
    assert_eq!(probe_root(&vault, Duration::from_secs(5)), Ok(()));
    assert!(probe_root(&vault.join("README.md"), Duration::from_secs(5)).is_err());
}
