//! The library sidebar tree: Markdown files and the folders that lead to them.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde::Serialize;

use super::{FileEntry, RootIndex};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub name: String,
    /// The absolute path.
    pub path: String,
    pub is_dir: bool,
    /// Folders first, then files, each in natural order. Empty for files.
    pub children: Vec<TreeNode>,
    /// The absolute path of this folder's `README.md`.
    pub readme: Option<String>,
    /// The frontmatter `status:` of that README.
    pub status: Option<String>,
}

/// Files and subfolders of one folder while the tree is assembled.
#[derive(Default)]
struct Folder<'a> {
    folders: HashMap<&'a str, Folder<'a>>,
    files: Vec<(&'a str, &'a FileEntry)>,
}

/// The tree of `root`'s Markdown files. A folder appears only when it holds Markdown somewhere
/// below it.
pub fn build_tree(root: &RootIndex) -> TreeNode {
    let mut top = Folder::default();
    for file in root.md_files() {
        let mut parts = file.rel.split('/');
        let Some(name) = parts.next_back() else {
            continue;
        };
        let folder = parts.fold(&mut top, |folder, part| {
            folder.folders.entry(part).or_default()
        });
        folder.files.push((name, file));
    }
    let name = root.root.file_name().unwrap_or(root.root.as_os_str());
    folder_node(
        root,
        name.to_string_lossy().into_owned(),
        String::new(),
        top,
    )
}

fn folder_node(root: &RootIndex, name: String, rel: String, folder: Folder) -> TreeNode {
    let readme = folder
        .files
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("README.md"))
        .map(|&(_, file)| file);
    let mut folders: Vec<TreeNode> = folder
        .folders
        .into_iter()
        .map(|(name, sub)| {
            let sub_rel = if rel.is_empty() {
                name.to_owned()
            } else {
                format!("{rel}/{name}")
            };
            folder_node(root, name.to_owned(), sub_rel, sub)
        })
        .collect();
    let mut files: Vec<TreeNode> = folder
        .files
        .iter()
        .map(|&(name, file)| TreeNode {
            name: name.to_owned(),
            path: abs_string(root, &file.rel),
            is_dir: false,
            children: Vec::new(),
            readme: None,
            status: None,
        })
        .collect();
    folders.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    files.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    folders.append(&mut files);
    TreeNode {
        name,
        path: abs_string(root, &rel),
        is_dir: true,
        children: folders,
        readme: readme.map(|file| abs_string(root, &file.rel)),
        status: readme.and_then(|file| file.fm_status.clone()),
    }
}

fn abs_string(root: &RootIndex, rel: &str) -> String {
    root.abs(rel).to_string_lossy().into_owned()
}

/// Case-insensitive natural order: runs of digits compare by value, so `a2` sorts before `a10`.
/// Names equal on those terms (`a`/`A`, `a01`/`a1`) fall back to plain order, so the order is
/// total and sorting is deterministic.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while let (Some(ca), Some(cb)) = (a[i..].chars().next(), b[j..].chars().next()) {
        let ord = if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let end_a = i + digit_run(&a[i..]);
            let end_b = j + digit_run(&b[j..]);
            let ord = cmp_numbers(&a[i..end_a], &b[j..end_b]);
            (i, j) = (end_a, end_b);
            ord
        } else {
            i += ca.len_utf8();
            j += cb.len_utf8();
            ca.to_lowercase().cmp(cb.to_lowercase())
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

fn digit_run(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_digit).count()
}

/// Compares two runs of ASCII digits by value, without overflow on long runs.
fn cmp_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}
