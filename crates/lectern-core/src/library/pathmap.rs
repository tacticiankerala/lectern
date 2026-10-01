//! Mapping paths written in notes to files on this machine, and local files to asset URLs.

use std::path::{Path, PathBuf};

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// Maps absolute paths found in notes (Linux, WSL, other machines) to local files.
#[derive(Debug, Clone, Default)]
pub struct PathMapper {
    /// User prefix mappings from settings, such as `/home/me/shared` → `S:\Shared`.
    pub mappings: Vec<(String, PathBuf)>,
    /// The default WSL distribution, for the `\\wsl.localhost\<distro>\…` fallback.
    pub wsl_distro: Option<String>,
}

/// Everything `encodeURIComponent` escapes: all but ASCII alphanumerics and `-_.!~*'()`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// The asset-protocol URL for a local file, matching Tauri's `convertFileSrc`: `asset_base`
/// followed by the whole path passed through `encodeURIComponent`.
pub fn asset_url(asset_base: &str, path: &Path) -> String {
    format!(
        "{asset_base}{}",
        utf8_percent_encode(&path.to_string_lossy(), URI_COMPONENT)
    )
}
