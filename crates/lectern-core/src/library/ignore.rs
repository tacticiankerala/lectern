//! Names the library walk skips: OS and NAS litter, VCS and tool folders.

/// Files skipped wherever they appear.
const IGNORED_FILES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

/// Directories skipped, with everything below them. Any directory starting with `.` is skipped too.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "#recycle",
    "#snapshot",
    "@eaDir",
    "$RECYCLE.BIN",
    "System Volume Information",
];

/// Whether the walk skips an entry with this file name. Names compare case-insensitively, as
/// Windows does (NTFS names the recycle bin `$Recycle.Bin`).
pub fn is_ignored(name: &str, is_dir: bool) -> bool {
    if name.starts_with("._") {
        return true;
    }
    let listed = |names: &[&str]| names.iter().any(|n| n.eq_ignore_ascii_case(name));
    if is_dir {
        name.starts_with('.') || listed(IGNORED_DIRS)
    } else {
        listed(IGNORED_FILES)
    }
}
