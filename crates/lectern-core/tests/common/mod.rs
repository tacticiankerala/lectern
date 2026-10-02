//! Helpers shared by the library and render tests. Each test crate uses only some of them.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::library::pathmap::asset_url;
use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::LibraryIndex;

/// A copy of `fixtures/vault` in a fresh temporary directory, so no test writes to the repo.
/// Returns the guard (the copy is deleted when it drops) and the path of the copied vault.
pub fn vault_copy() -> (tempfile::TempDir, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vault");
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("vault");
    copy_dir(&src, &dst);
    (tmp, dst)
}

/// An index holding `root` alone, scanned with its frontmatter heads read.
pub fn index_of(root: &Path) -> LibraryIndex {
    let mut index = scan_root(root, &ScanOptions::default()).unwrap();
    read_heads(&mut index);
    LibraryIndex { roots: vec![index] }
}

/// Where a redacted vault path starts in a snapshot.
pub const VAULT_MARK: &str = "[vault]";

/// `text` with `vault`'s path, written plainly or encoded in an asset URL, replaced by
/// `[vault]`, and the rest of each such path (up to the end of its attribute value) written with
/// `/` and `%2F` rather than `\` and `%5C`. Snapshots then read the same on Windows and Linux.
pub fn redact_vault(text: &str, vault: &Path) -> String {
    let text = text
        .replace(&asset_url("", vault), VAULT_MARK)
        .replace(&*vault.to_string_lossy(), VAULT_MARK);
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find(VAULT_MARK) {
        let path_start = at + VAULT_MARK.len();
        let path_end = rest[path_start..]
            .find(['"', '<', '\n'])
            .map_or(rest.len(), |len| path_start + len);
        out.push_str(&rest[..path_start]);
        out.push_str(
            &rest[path_start..path_end]
                .replace('\\', "/")
                .replace("%5C", "%2F"),
        );
        rest = &rest[path_end..];
    }
    out.push_str(rest);
    out
}

/// `path` without the verbatim `\\?\` prefix that `canonicalize` (and so `insta::glob!`) adds to
/// a drive path on Windows. Lectern opens documents by their plain paths and treats a verbatim
/// path as untrusted, so a document rendered at one would have its local images blocked.
pub fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// `root` joined with the `/`-separated `rel` using the platform's separators, as the index
/// joins paths.
pub fn native_join(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}
