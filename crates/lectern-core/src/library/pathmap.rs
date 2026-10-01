//! Mapping paths written in notes to files on this machine, and local files to asset URLs.

use std::path::{Path, PathBuf};

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::resolve::{library_suffix_match, moved_file_match};
use super::LibraryIndex;

/// Maps absolute paths found in notes (Linux, WSL, other machines) to local files.
#[derive(Debug, Clone, Default)]
pub struct PathMapper {
    /// User prefix mappings from settings, such as `/home/me/shared` → `S:\Shared`.
    pub mappings: Vec<(String, PathBuf)>,
    /// The default WSL distribution, for the `\\wsl.localhost\<distro>\…` fallback.
    pub wsl_distro: Option<String>,
}

/// Where an absolute path from a note points on this machine.
#[derive(Debug, PartialEq)]
pub enum Mapped {
    /// A file the library index holds.
    Verified(PathBuf),
    /// A local path that may or may not exist; checked when it's followed.
    Unverified(PathBuf),
    Unresolved,
}

impl PathMapper {
    /// Maps the absolute path `raw`, in order:
    /// 1. the longest user prefix mapping that covers whole folders (a Windows prefix matches in
    ///    any letter case and with either separator; a Linux one matches exactly);
    /// 2. `/mnt/<x>/…` → `X:\…`;
    /// 3. a library suffix match: a component equal to a root's folder name, and the rest a file
    ///    in that root;
    /// 4. moved-file recovery: the last 4, 3 or 2 components match exactly one indexed file;
    /// 5. `\\wsl.localhost\<distro>\…` for Linux paths, when WSL is present.
    ///
    /// A Windows path (`C:\…`, `\\server\…`) that no user mapping covers is a candidate as
    /// written, like the result of steps 1–2. Steps 3 and 4 need the index and give `Verified`;
    /// the others give `Unverified` unless the index holds the path. A candidate from steps 1–2
    /// that the index doesn't hold still gives way to steps 3–4, so a link written before
    /// `work/x` moved to `archive/x` finds the file behind a user mapping too.
    pub fn map(&self, raw: &str, index: Option<&LibraryIndex>) -> Mapped {
        if !is_absolute(raw) {
            return Mapped::Unresolved;
        }
        let candidate = self
            .user_mapped(raw)
            .or_else(|| mnt_drive(raw))
            .or_else(|| is_windows_absolute(raw).then(|| PathBuf::from(raw)));
        if let Some(index) = index {
            if let Some(candidate) = candidate.as_ref().filter(|c| index.contains(c)) {
                return Mapped::Verified(candidate.clone());
            }
            let parts: Vec<&str> = components(raw).collect();
            if let Some(found) =
                library_suffix_match(index, &parts).or_else(|| moved_file_match(index, &parts))
            {
                return Mapped::Verified(found);
            }
        }
        if let Some(candidate) = candidate {
            return Mapped::Unverified(candidate);
        }
        match self.wsl_path(raw) {
            Some(path) if index.is_some_and(|ix| ix.contains(&path)) => Mapped::Verified(path),
            Some(path) => Mapped::Unverified(path),
            None => Mapped::Unresolved,
        }
    }

    /// `raw` under the longest mapped prefix that ends at a folder boundary.
    fn user_mapped(&self, raw: &str) -> Option<PathBuf> {
        self.mappings
            .iter()
            .filter_map(|(prefix, target)| {
                let prefix = prefix.trim_end_matches(['/', '\\']);
                let rest = if is_windows_prefix(prefix) {
                    strip_prefix_like_windows(raw, prefix)?
                } else {
                    raw.strip_prefix(prefix)?
                };
                let rest = match rest.strip_prefix(['/', '\\']) {
                    Some(rest) => rest,
                    None if rest.is_empty() => rest,
                    None => return None,
                };
                Some((prefix.len(), target, rest))
            })
            .max_by_key(|&(len, ..)| len)
            .map(|(_, target, rest)| windows_join(&target.to_string_lossy(), rest))
    }

    fn wsl_path(&self, raw: &str) -> Option<PathBuf> {
        let distro = self.wsl_distro.as_deref()?;
        if !raw.starts_with('/') {
            return None;
        }
        Some(windows_join(&format!(r"\\wsl.localhost\{distro}"), raw))
    }
}

/// A mapping prefix (trailing separators trimmed) that names a drive (`C:`, `C:\Old`) or a UNC
/// share (`\\server\share`).
fn is_windows_prefix(prefix: &str) -> bool {
    let b = prefix.as_bytes();
    let drive = b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || matches!(b[2], b'\\' | b'/'));
    drive || prefix.starts_with(r"\\")
}

/// `raw` without `prefix`, compared as Windows compares paths: letter case ignored, `\` and `/`
/// alike.
fn strip_prefix_like_windows<'r>(raw: &'r str, prefix: &str) -> Option<&'r str> {
    let is_separator = |c: char| c == '/' || c == '\\';
    let mut chars = raw.char_indices();
    for p in prefix.chars() {
        let (_, r) = chars.next()?;
        let same =
            r == p || (is_separator(r) && is_separator(p)) || r.to_lowercase().eq(p.to_lowercase());
        if !same {
            return None;
        }
    }
    let rest_start = chars.next().map_or(raw.len(), |(i, _)| i);
    Some(&raw[rest_start..])
}

/// `/mnt/c/Users/a.txt` → `C:\Users\a.txt`.
fn mnt_drive(raw: &str) -> Option<PathBuf> {
    let rest = raw.strip_prefix("/mnt/")?;
    let (drive, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let mut letters = drive.chars();
    match (letters.next(), letters.next()) {
        (Some(letter), None) if letter.is_ascii_alphabetic() => Some(windows_join(
            &format!("{}:", letter.to_ascii_uppercase()),
            rest,
        )),
        _ => None,
    }
}

/// `base` and the `/`- or `\`-separated `rest`, joined with `\` whatever platform this runs on:
/// the result is always a Windows path.
fn windows_join(base: &str, rest: &str) -> PathBuf {
    let mut path = base.trim_end_matches(['/', '\\']).to_owned();
    let mut parts = components(rest).peekable();
    // A bare drive needs its separator: `C:` alone means the drive's current folder.
    if parts.peek().is_none() && path.ends_with(':') {
        path.push('\\');
    }
    for part in parts {
        path.push('\\');
        path.push_str(part);
    }
    PathBuf::from(path)
}

/// The non-empty components of a path written with either separator.
fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\']).filter(|part| !part.is_empty())
}

/// A Linux path (`/…`), a drive path (`C:\…`, `C:/…`) or a UNC path (`\\server\…`).
pub(crate) fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || is_windows_absolute(path)
}

fn is_windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with(r"\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// A path with an optional line and column, as notes cite code: `a.rb:17`, `a.rb:17:5`,
/// `a.md#L17`.
#[derive(Debug, PartialEq)]
pub struct LineRef {
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

/// Splits a trailing `:line`, `:line:col` or `#Lline` off `s`. A drive letter's colon (`C:\x`)
/// is never a suffix.
pub fn split_line_suffix(s: &str) -> LineRef {
    if let Some((path, line)) = s
        .rsplit_once('#')
        .and_then(|(path, fragment)| Some((path, line_fragment(fragment)?)))
    {
        return LineRef {
            path: path.to_owned(),
            line: Some(line),
            col: None,
        };
    }
    let Some((rest, last)) = number_suffix(s) else {
        return LineRef {
            path: s.to_owned(),
            line: None,
            col: None,
        };
    };
    let (path, line, col) = match number_suffix(rest) {
        Some((path, line)) => (path, line, Some(last)),
        None => (rest, last, None),
    };
    LineRef {
        path: path.to_owned(),
        line: Some(line),
        col,
    }
}

/// The line in a GitHub-style `L17` fragment.
pub(crate) fn line_fragment(fragment: &str) -> Option<u32> {
    parse_digits(fragment.strip_prefix('L')?)
}

/// A trailing `:digits`, and what comes before it. The part before must be more than a drive
/// letter.
fn number_suffix(s: &str) -> Option<(&str, u32)> {
    let (head, digits) = s.rsplit_once(':')?;
    let n = parse_digits(digits)?;
    let is_drive = head.len() == 1 && head.bytes().all(|b| b.is_ascii_alphabetic());
    (!head.is_empty() && !is_drive).then_some((head, n))
}

fn parse_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
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
