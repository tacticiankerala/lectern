mod common;

use std::cmp::Ordering;
use std::fs;
use std::path::Path;

use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::tree::{build_tree, natural_cmp, TreeNode};

fn tree_of(vault: &Path) -> TreeNode {
    let mut r = scan_root(vault, &ScanOptions::default()).unwrap();
    read_heads(&mut r);
    build_tree(&r)
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

fn abs(vault: &Path, rel: &str) -> String {
    rel.split('/')
        .fold(vault.to_path_buf(), |p, c| p.join(c))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn tree_md_only_folders_first_natural() {
    let (_tmp, vault) = common::vault_copy();
    let tree = tree_of(&vault);
    assert!(tree.is_dir);
    assert_eq!(tree.name, "vault");
    assert_eq!(tree.path, vault.to_string_lossy());
    assert_eq!(
        names(&tree),
        [
            "archive",
            "friends",
            "memory",
            "notes",
            "prompts",
            "stress",
            "work",
            "README.md"
        ]
    );
    assert_eq!(names(child(&tree, "friends")), ["readme-style.md"]);
    let alpha = child(child(&tree, "work"), "alpha");
    assert!(alpha.is_dir);
    assert_eq!(alpha.path, abs(&vault, "work/alpha"));
    assert_eq!(names(alpha), ["notes", "plans", "README.md"]);
    assert_eq!(alpha.readme, Some(abs(&vault, "work/alpha/README.md")));
    assert_eq!(alpha.status.as_deref(), Some("blocked"));
    let readme = child(alpha, "README.md");
    assert!(!readme.is_dir && readme.children.is_empty());
    assert_eq!(readme.path, abs(&vault, "work/alpha/README.md"));
}

#[test]
fn tree_root_and_folders_carry_readme_status() {
    let (_tmp, vault) = common::vault_copy();
    let tree = tree_of(&vault);
    assert_eq!(tree.readme, Some(abs(&vault, "README.md")));
    assert_eq!(tree.status.as_deref(), Some("active"));
    let beta = child(child(&tree, "archive"), "beta");
    assert_eq!(beta.status.as_deref(), Some("done"));
    // readme-style.md is not a README.
    let friends = child(&tree, "friends");
    assert_eq!(friends.readme, None);
    assert_eq!(friends.status, None);
    // Files carry neither.
    assert_eq!(child(&tree, "README.md").readme, None);
}

#[test]
fn tree_drops_folders_without_markdown_below() {
    let (_tmp, vault) = common::vault_copy();
    fs::create_dir_all(vault.join("assets/deep/er")).unwrap();
    fs::write(vault.join("assets/deep/er/pic.png"), b"png").unwrap();
    fs::create_dir_all(vault.join("empty")).unwrap();
    fs::create_dir_all(vault.join("nested/only/here")).unwrap();
    fs::write(vault.join("nested/only/here/note.md"), "# n\n").unwrap();
    let tree = tree_of(&vault);
    let top = names(&tree);
    assert!(
        !top.contains(&"assets") && !top.contains(&"empty"),
        "{top:?}"
    );
    assert!(!top.contains(&"node_modules"), "{top:?}");
    let here = child(child(child(&tree, "nested"), "only"), "here");
    assert_eq!(names(here), ["note.md"]);
}

#[test]
fn tree_serializes_camel_case() {
    let (_tmp, vault) = common::vault_copy();
    let json = serde_json::to_value(tree_of(&vault)).unwrap();
    assert_eq!(json["isDir"], true);
    assert!(json["children"].is_array());
    assert_eq!(json["status"], "active");
}

#[test]
fn natural_sort() {
    let mut v = vec!["a10", "a2", "B1", "a1"];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(v, ["a1", "a2", "a10", "B1"]);
}

#[test]
fn natural_sort_handles_spaces_dates_and_long_numbers() {
    let mut v = vec!["file 10.md", "file 9.md", "File 1.md"];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(v, ["File 1.md", "file 9.md", "file 10.md"]);

    let mut d = vec!["2026-01-10-x.md", "2026-01-02-y.md", "2025-12-31-z.md"];
    d.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(d, ["2025-12-31-z.md", "2026-01-02-y.md", "2026-01-10-x.md"]);

    assert_eq!(
        natural_cmp("x99999999999999999999999", "x100000000000000000000000"),
        Ordering::Less
    );
}

#[test]
fn natural_sort_is_a_total_order() {
    // Equal ignoring case or leading zeros still compares unequal, so sorting is deterministic.
    assert_ne!(natural_cmp("a", "A"), Ordering::Equal);
    assert_ne!(natural_cmp("a01", "a1"), Ordering::Equal);
    assert_eq!(natural_cmp("a1", "a1"), Ordering::Equal);
    assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    assert_eq!(natural_cmp("ab", "a"), Ordering::Greater);
}
