//! Helpers shared by the library tests.

use std::fs;
use std::path::{Path, PathBuf};

/// A copy of `fixtures/vault` in a fresh temporary directory, so no test writes to the repo.
/// Returns the guard (the copy is deleted when it drops) and the path of the copied vault.
pub fn vault_copy() -> (tempfile::TempDir, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vault");
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("vault");
    copy_dir(&src, &dst);
    (tmp, dst)
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
