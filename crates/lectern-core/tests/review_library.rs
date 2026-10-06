//! Review sidecars in the library: hidden from every surface that lists notes, whatever the
//! settings, and counted on the note they belong to. Each test builds its own root in a temporary
//! folder; sidecars never go in `fixtures/vault`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::native_join;
use lectern_core::library::resolve::{resolve_wikilink, WikiResolution};
use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::snapshot::{load_snapshot, save_snapshot, snapshot_path};
use lectern_core::library::tree::{build_tree, TreeNode};
use lectern_core::library::{FileEntry, LibraryIndex, RootIndex};
use lectern_core::search::ContentCache;

const TIDE: &str = "# Tide tables\n\nThe spring tide peaks at noon on the north quay.\n";

/// The sidecar of `tide.md`: two comments, the second resolved, so one is open.
const TIDE_REVIEW: &str = "---
lectern-review: 1
note: tide.md
name: tide-review
---
# Review: tide.md

## C1 · open · L3 · Tide tables
<!-- anchor prefix=\"The \" suffix=\" peaks at noon\" fp=\"fnv1a64:0123456789abcdef\" n=1 created=\"2026-10-01T09:00:00Z\" -->
> spring tide

**You:** Which almanac gives the spring tide?

## C2 · resolved · L3 · Tide tables
<!-- anchor prefix=\"peaks at \" suffix=\" on the north\" fp=\"fnv1a64:0123456789abcdef\" n=1 created=\"2026-10-01T09:05:00Z\" -->
> noon

**You:** Local time or harbour time?
";

/// An ordinary note that happens to end in `.review.md`.
const CODE_REVIEW: &str = "# Code review\n\nCheck the spring tide parser before the release.\n";

/// Sidecar frontmatter naming a note that isn't there.
const ORPHAN_REVIEW: &str = "---
lectern-review: 1
note: missing.md
---
# Review: missing.md
";

const TEMP: &str = "plans/.tide.review.md.lectern.tmp";

fn write(root: &Path, rel: &str, text: &str) {
    let path = native_join(root, rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A root holding `files`, walked but with no heads read yet: what the app shows first for a root
/// it has no snapshot of.
fn walked(files: &[(&str, &str)]) -> (tempfile::TempDir, RootIndex) {
    let tmp = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    let root = scan_root(tmp.path(), &ScanOptions::default()).unwrap();
    (tmp, root)
}

/// A root holding `files`, scanned and with its heads read, as the app builds it.
fn root_of(files: &[(&str, &str)]) -> (tempfile::TempDir, RootIndex) {
    let (tmp, mut root) = walked(files);
    read_heads(&mut root);
    (tmp, root)
}

/// The plans folder of the review comments feature: a note, its sidecar, an ordinary note named
/// like a sidecar, an orphaned sidecar and a save's temporary file.
fn plans() -> (tempfile::TempDir, RootIndex) {
    root_of(&[
        ("plans/tide.md", TIDE),
        ("plans/tide.review.md", TIDE_REVIEW),
        ("plans/code.review.md", CODE_REVIEW),
        ("plans/orphan.review.md", ORPHAN_REVIEW),
        (TEMP, TIDE_REVIEW),
    ])
}

fn names(node: &TreeNode) -> Vec<&str> {
    node.children.iter().map(|c| c.name.as_str()).collect()
}

fn child<'a>(node: &'a TreeNode, name: &str) -> &'a TreeNode {
    node.children
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} under {}: {:?}", node.name, names(node)))
}

fn md_rels(root: &RootIndex) -> Vec<&str> {
    root.md_files().map(|f| f.rel.as_str()).collect()
}

fn found(path: PathBuf) -> WikiResolution {
    WikiResolution::Found { path, anchor: None }
}

#[test]
fn sidecar_hidden_from_tree_quick_open_search_and_wikilinks() {
    let (tmp, root) = plans();
    let tree = build_tree(&root);
    assert_eq!(
        names(child(&tree, "plans")),
        ["code.review.md", "orphan.review.md", "tide.md"]
    );

    // Quick open lists `md_files`.
    assert_eq!(
        md_rels(&root),
        [
            "plans/code.review.md",
            "plans/orphan.review.md",
            "plans/tide.md"
        ]
    );

    let index = LibraryIndex { roots: vec![root] };
    let tide = native_join(tmp.path(), "plans/tide.md");
    assert_eq!(
        resolve_wikilink(&index, &tide, "tide.review"),
        WikiResolution::Broken
    );
    // Nor by its frontmatter `name:`.
    assert_eq!(
        resolve_wikilink(&index, &tide, "tide-review"),
        WikiResolution::Broken
    );
    // Ordinary notes named like sidecars stay linkable.
    assert_eq!(
        resolve_wikilink(&index, &tide, "code.review"),
        found(native_join(tmp.path(), "plans/code.review.md"))
    );
    assert_eq!(
        resolve_wikilink(&index, &tide, "orphan.review"),
        found(native_join(tmp.path(), "plans/orphan.review.md"))
    );

    // The quote in the sidecar matches too, but only the notes are searched.
    let hits = ContentCache::new().search(&index, "spring tide");
    let rels: Vec<&str> = hits.iter().map(|f| f.rel.as_str()).collect();
    assert!(rels.contains(&"plans/tide.md"), "{rels:?}");
    assert!(rels.contains(&"plans/code.review.md"), "{rels:?}");
    assert!(!rels.contains(&"plans/tide.review.md"), "{rels:?}");
}

#[test]
fn note_carries_its_open_comment_count() {
    let (_tmp, root) = plans();
    let tree = build_tree(&root);
    let plans = child(&tree, "plans");
    assert_eq!(child(plans, "tide.md").comments, Some(1));
    assert_eq!(child(plans, "code.review.md").comments, None);
    assert_eq!(child(plans, "orphan.review.md").comments, None);
    assert_eq!(plans.comments, None);
    assert_eq!(tree.comments, None);
    assert_eq!(root.comment_count("plans/tide.md"), Some(1));
    assert_eq!(root.comment_count("PLANS/Tide.md"), Some(1));
    assert_eq!(root.comment_count("plans/code.review.md"), None);
}

#[test]
fn temp_files_are_ignored() {
    let (_tmp, root) = plans();
    assert!(root.get(TEMP).is_none());
    let rels: Vec<&str> = root.files.iter().map(|f| f.rel.as_str()).collect();
    assert!(
        !rels.iter().any(|rel| rel.ends_with(".lectern.tmp")),
        "{rels:?}"
    );
}

#[test]
fn old_snapshots_still_load() {
    let entry: FileEntry = serde_json::from_value(serde_json::json!({
        "rel": "plans/tide.md",
        "is_md": true,
        "mtime_ms": 1,
        "size": 2,
        "fm_name": null,
        "fm_status": null,
    }))
    .unwrap();
    assert_eq!(entry.review_of, None);
    assert_eq!(entry.review_open, None);
    assert!(!entry.is_sidecar);
}

#[test]
fn a_loaded_snapshot_keeps_sidecars_hidden_and_counted() {
    let (tmp, root) = plans();
    let dir = tmp.path().join("snapshots");
    save_snapshot(&dir, &root).unwrap();
    let loaded = load_snapshot(&dir, tmp.path()).unwrap();
    assert!(root.heads_read);
    assert!(loaded.heads_read);
    assert_eq!(md_rels(&loaded), md_rels(&root));
    assert_eq!(loaded.comment_count("plans/tide.md"), Some(1));
}

/// A snapshot saved before sidecars existed has no heads for them, so they're told by their names
/// until the next scan reads the heads.
#[test]
fn an_older_snapshot_hides_sidecars_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("snapshots");
    let entry = |rel: &str| {
        serde_json::json!({
            "rel": rel, "is_md": true, "mtime_ms": 1, "size": 1, "fm_name": null, "fm_status": null,
        })
    };
    let older = serde_json::json!({
        "root": tmp.path(),
        "files": [entry("plans/tide.md"), entry("plans/tide.review.md"), entry("plans/code.review.md")],
        "scanned_at_ms": 1,
    });
    fs::create_dir_all(&dir).unwrap();
    fs::write(snapshot_path(&dir, tmp.path()), older.to_string()).unwrap();
    let loaded = load_snapshot(&dir, tmp.path()).unwrap();
    assert!(!loaded.heads_read);
    assert_eq!(md_rels(&loaded), ["plans/tide.md", "plans/code.review.md"]);
}

/// Before the heads are read, nothing is known of any frontmatter. A file named as the sidecar of
/// a Markdown note beside it is hidden in the meantime, so the tree, quick open, search and
/// wikilinks never offer a sidecar while a slow share is still being read.
#[test]
fn sidecars_are_hidden_by_name_until_the_heads_are_read() {
    // `scan_root` finalizes the index it builds.
    let (_tmp, root) = walked(&[
        ("plans/tide.md", TIDE),
        ("plans/tide.review.md", TIDE_REVIEW),
        ("plans/code.review.md", CODE_REVIEW),
        ("plans/orphan.review.md", ORPHAN_REVIEW),
    ]);
    assert!(!root.heads_read);
    let notes = [
        "plans/code.review.md",
        "plans/orphan.review.md",
        "plans/tide.md",
    ];
    assert_eq!(md_rels(&root), notes);
    assert_eq!(
        names(child(&build_tree(&root), "plans")),
        ["code.review.md", "orphan.review.md", "tide.md"]
    );
    assert_eq!(root.with_stem("tide.review").count(), 0);
    assert_eq!(root.with_stem("code.review").count(), 1);
}

#[test]
fn reading_the_heads_settles_which_files_are_sidecars() {
    let markdown_review = TIDE_REVIEW.replace("note: tide.md", "note: x.markdown");
    let (_tmp, mut root) = walked(&[
        ("plans/code.md", TIDE),
        ("plans/code.review.md", CODE_REVIEW),
        ("plans/tide.md", TIDE),
        ("plans/tide.review.md", TIDE_REVIEW),
        ("plans/x.markdown", TIDE),
        ("plans/x.markdown.review.md", &markdown_review),
    ]);
    // Each `.review.md` here is named as the sidecar of the note beside it, so for now all three
    // are hidden, the ordinary `code.review.md` too.
    assert_eq!(
        md_rels(&root),
        ["plans/code.md", "plans/tide.md", "plans/x.markdown"]
    );
    assert_eq!(root.comment_count("plans/tide.md"), None);

    read_heads(&mut root);
    assert!(root.heads_read);
    // `code.review.md` has no sidecar frontmatter, so it's a note after all.
    assert_eq!(
        md_rels(&root),
        [
            "plans/code.md",
            "plans/code.review.md",
            "plans/tide.md",
            "plans/x.markdown"
        ]
    );
    assert_eq!(root.comment_count("plans/tide.md"), Some(1));
    assert_eq!(root.comment_count("plans/x.markdown"), Some(1));
    assert_eq!(root.comment_count("plans/code.md"), None);
}

/// `x.markdown.md` and `x.markdown` both have the sidecar name `x.markdown.review.md`; its `note:`
/// says whose it is. Names compare ignoring case, as Windows does.
#[test]
fn a_name_clash_counts_only_for_the_named_note() {
    let sidecar = TIDE_REVIEW.replace("note: tide.md", "note: X.Markdown.MD");
    let (_tmp, root) = root_of(&[
        ("x.markdown.md", TIDE),
        ("x.markdown", TIDE),
        ("x.markdown.review.md", &sidecar),
    ]);
    assert_eq!(md_rels(&root), ["x.markdown", "x.markdown.md"]);
    assert_eq!(root.comment_count("x.markdown.md"), Some(1));
    assert_eq!(root.comment_count("x.markdown"), None);
}

/// A `note:` that reaches into another folder doesn't make a sidecar, even when that note exists.
#[test]
fn a_sidecar_sits_beside_its_note() {
    let sidecar = TIDE_REVIEW.replace("note: tide.md", "note: sub/tide.md");
    let (_tmp, root) = root_of(&[
        ("plans/sub/tide.md", TIDE),
        ("plans/tide.review.md", &sidecar),
    ]);
    assert_eq!(
        md_rels(&root),
        ["plans/sub/tide.md", "plans/tide.review.md"]
    );
    assert_eq!(root.comment_count("plans/sub/tide.md"), None);
}

/// Only Markdown files are notes, so a sidecar naming anything else is an ordinary note itself.
#[test]
fn a_sidecar_belongs_to_a_markdown_note() {
    let sidecar = TIDE_REVIEW.replace("note: tide.md", "note: photo.png");
    let (_tmp, root) = root_of(&[
        ("plans/photo.png", "not really a picture"),
        ("plans/photo.png.review.md", &sidecar),
    ]);
    assert_eq!(md_rels(&root), ["plans/photo.png.review.md"]);
    let tree = build_tree(&root);
    assert_eq!(names(child(&tree, "plans")), ["photo.png.review.md"]);
    assert_eq!(root.comment_count("plans/photo.png"), None);
}
