//! Comparing and naming paths as Windows does: case-insensitively, either separator.

use std::path::{Path, PathBuf};

use lectern_core::library::path_key;

pub(super) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// Whether `path` is `root` or below it, compared as Windows compares paths.
pub(super) fn is_under(path: &Path, root: &Path) -> bool {
    let (path, root) = (path_key(path), path_key(root));
    path == root
        || path
            .strip_prefix(root.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A root's display name: its folder name, or the whole path for a drive or share root.
pub(super) fn root_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || path_string(root),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// `path` as a root: absolute, without trailing separators (a drive root keeps its `\`).
pub(super) fn normalize_root(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    let mut text = trimmed.trim_end_matches(['\\', '/']).to_owned();
    if text.len() == 2 && text.ends_with(':') {
        text.push('\\');
    }
    let root = PathBuf::from(text);
    if !root.is_absolute() {
        return Err(format!("{trimmed} isn't a full folder path"));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_are_normalised_and_must_be_absolute() {
        assert_eq!(
            normalize_root(r" S:\Notes\My Vault\dev\ ").unwrap(),
            PathBuf::from(r"S:\Notes\My Vault\dev")
        );
        assert_eq!(normalize_root("C:").unwrap(), PathBuf::from(r"C:\"));
        assert_eq!(normalize_root(r"C:\\").unwrap(), PathBuf::from(r"C:\"));
        assert_eq!(
            normalize_root(r"\\nonexistent\share\").unwrap(),
            PathBuf::from(r"\\nonexistent\share")
        );
        assert!(normalize_root(r"notes\dev").is_err());
    }

    #[test]
    fn paths_compare_as_on_windows() {
        assert!(is_under(Path::new(r"S:\Dev\a\b.md"), Path::new(r"s:\dev")));
        assert!(is_under(Path::new(r"S:\Dev"), Path::new(r"S:/Dev/")));
        assert!(is_under(Path::new(r"C:\a.md"), Path::new(r"C:\")));
        assert!(!is_under(
            Path::new(r"S:\Devices\a.md"),
            Path::new(r"S:\Dev")
        ));
    }
}
