//! Root indexes persisted as JSON, so the tree can appear before the first walk finishes.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{path_key, RootIndex};

/// Tells apart the temporary files of saves running at the same time.
static SAVE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The snapshot file for `root` in `dir`: `index-<hash>.json`, where the hash is 64-bit FNV-1a
/// over the root's comparison key, so `S:\Dev` and `s:\dev\` share a snapshot.
pub fn snapshot_path(dir: &Path, root: &Path) -> PathBuf {
    dir.join(format!(
        "index-{:016x}.json",
        fnv1a(path_key(root).as_bytes())
    ))
}

/// Writes `r`'s snapshot, creating `dir` if needed. The JSON goes to a temporary file in `dir`
/// that then replaces the snapshot, so a reader never sees a half-written file.
pub fn save_snapshot(dir: &Path, r: &RootIndex) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let target = snapshot_path(dir, &r.root);
    let mut tmp_name = target.file_name().unwrap_or_default().to_owned();
    tmp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        SAVE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);
    let written = write_json(&tmp, r).and_then(|()| fs::rename(&tmp, &target));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn write_json(path: &Path, r: &RootIndex) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut out = BufWriter::new(file);
    serde_json::to_writer(&mut out, r)?;
    out.flush()?;
    out.get_ref().sync_all()
}

/// The saved snapshot of `root`, finalized. `None` when it is missing, unreadable, corrupt, or
/// records a different root.
pub fn load_snapshot(dir: &Path, root: &Path) -> Option<RootIndex> {
    // Reading the whole file first parses much faster than `from_reader`.
    let bytes = fs::read(snapshot_path(dir, root)).ok()?;
    let mut r: RootIndex = serde_json::from_slice(&bytes).ok()?;
    if path_key(&r.root) != path_key(root) {
        return None;
    }
    r.finalize();
    Some(r)
}

/// 64-bit FNV-1a: stable across runs and platforms, unlike `std`'s hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::fnv1a;

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
