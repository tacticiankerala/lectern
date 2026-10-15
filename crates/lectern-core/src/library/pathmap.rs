//! Mapping paths written in notes to files on this machine, and local files to asset and page
//! URLs.

use std::ops::Range;
use std::path::{Path, PathBuf};

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::resolve::{library_suffix_match, moved_file_match};
use super::LibraryIndex;
use crate::ipc::{ConvertedPaths, PathForm, PathForms};

/// The most text [`PathMapper::convert_paths`] is asked to convert at once, in bytes.
pub const CONVERT_LIMIT: usize = 64 * 1024;

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

    /// The path a WSL shell knows a local file by, the reverse of [`PathMapper::map`]:
    /// 1. the longest user mapping from a Linux folder whose Windows side covers whole folders of
    ///    `path` (any letter case, either separator) gives that Linux folder;
    /// 2. `\\wsl.localhost\<distro>\…` and `\\wsl$\<distro>\…` give `/…`;
    /// 3. `X:\…` gives `/mnt/x/…`.
    ///
    /// Any other path, such as a share on another machine, has none.
    pub fn to_wsl(&self, path: &Path) -> Option<String> {
        let raw = path.to_string_lossy();
        self.user_unmapped(&raw)
            .or_else(|| wsl_share_path(&raw))
            .or_else(|| mnt_path(&raw))
    }

    /// `raw` under the longest mapping target that ends at a folder boundary, on the Linux side.
    /// Mappings from a Windows path (`C:\Old`, `\\server\share`) have no Linux side.
    fn user_unmapped(&self, raw: &str) -> Option<String> {
        self.mappings
            .iter()
            .filter(|(source, _)| source.starts_with('/') && !source.starts_with("//"))
            .filter_map(|(linux, target)| {
                let target = target.to_string_lossy();
                let target = target.trim_end_matches(['/', '\\']);
                if target.is_empty() {
                    return None;
                }
                let rest = strip_prefix_like_windows(raw, target)?;
                let rest = match rest.strip_prefix(['/', '\\']) {
                    Some(rest) => rest,
                    None if rest.is_empty() => rest,
                    None => return None,
                };
                Some((target.len(), linux, rest))
            })
            .max_by_key(|&(len, ..)| len)
            .map(|(_, linux, rest)| linux_join(linux, rest))
    }

    /// Both forms of the text `raw`, written as a path (a selection, a link's destination, or a
    /// broken link's text), for pasting into a terminal. Surrounding whitespace, quotes and
    /// backticks, and a trailing `:line`, `#Lline` or `#anchor`, are dropped first. Only strings
    /// and the in-memory index are consulted, never the disk, so it answers at once even for a
    /// path on a share that stalls.
    ///
    /// For an absolute path, the Windows form is where [`PathMapper::map`] puts it, verified or
    /// not, so it may be the file a moved note's link finds; the WSL form is
    /// [`PathMapper::to_wsl`] of that. A relative path (see `is_relative`) only changes its
    /// separators: `\` in the Windows form, `/` in the WSL form. It is never resolved, since
    /// paths in notes are often relative to somewhere else, such as a repository. Anything else,
    /// such as a bare file name or a URL, has neither form.
    pub fn forms(&self, raw: &str, index: Option<&LibraryIndex>) -> PathForms {
        self.bare_forms(bare_path(raw), index)
    }

    /// [`PathMapper::forms`] of a path already stripped by `bare_path`.
    fn bare_forms(&self, path: &str, index: Option<&LibraryIndex>) -> PathForms {
        if is_relative(path) {
            return PathForms {
                windows: Some(path.replace('/', "\\")),
                wsl: Some(path.replace('\\', "/")),
            };
        }
        let windows = match self.map(path, index) {
            Mapped::Verified(path) | Mapped::Unverified(path) => Some(path),
            Mapped::Unresolved => None,
        };
        self.both_forms(windows)
    }

    /// `text` with each of its path tokens (see [`path_tokens`]) in `form`, by the rules of
    /// [`PathMapper::forms`]; every other byte stays as it is, and so do the quotes, backticks
    /// and `:line` or `#anchor` around a path. A path with no such form stays as written and
    /// counts as unconverted.
    pub fn convert_paths(
        &self,
        text: &str,
        form: PathForm,
        index: Option<&LibraryIndex>,
    ) -> ConvertedPaths {
        let mut out = String::with_capacity(text.len());
        let (mut converted, mut unconverted) = (0, 0);
        let mut done = 0;
        for token in path_tokens(text) {
            out.push_str(&text[done..token.start]);
            let raw = &text[token.clone()];
            let span = bare_span(raw);
            let forms = self.bare_forms(&raw[span.clone()], index);
            let path = match form {
                PathForm::Windows => forms.windows,
                PathForm::Wsl => forms.wsl,
            };
            match path {
                Some(path) => {
                    out.push_str(&raw[..span.start]);
                    out.push_str(&path);
                    out.push_str(&raw[span.end..]);
                    converted += 1;
                }
                None => {
                    out.push_str(raw);
                    unconverted += 1;
                }
            }
            done = token.end;
        }
        out.push_str(&text[done..]);
        ConvertedPaths {
            text: out,
            converted,
            unconverted,
        }
    }

    /// Both forms of `path`, an exact path on this machine (the open file, a tree row, a resolved
    /// link's target), for pasting into a terminal. Nothing is trimmed or dropped, since a `#` or
    /// `:12` there is part of a name, and no index is consulted, so the file is never swapped for
    /// another. The Windows form is `path` itself when it is a Windows path, else where
    /// [`PathMapper::map`] puts it without an index; the WSL form is [`PathMapper::to_wsl`] of that.
    pub fn literal_forms(&self, path: &str) -> PathForms {
        let windows = if is_windows_absolute(path) {
            Some(PathBuf::from(path))
        } else {
            match self.map(path, None) {
                Mapped::Verified(path) | Mapped::Unverified(path) => Some(path),
                Mapped::Unresolved => None,
            }
        };
        self.both_forms(windows)
    }

    /// `windows` and its WSL form, as strings.
    fn both_forms(&self, windows: Option<PathBuf>) -> PathForms {
        PathForms {
            wsl: windows.as_deref().and_then(|path| self.to_wsl(path)),
            windows: windows.map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

/// `raw` without surrounding whitespace, quotes or backticks, and without a trailing `#anchor`,
/// `:line`, `:line:col` or `#Lline`. A `#` is an anchor only after a file name with an extension
/// (`plan.md#next-steps`), so `Meeting #3.md` and the folder in `C:\C#\x.cs` keep theirs.
fn bare_path(raw: &str) -> &str {
    &raw[bare_span(raw)]
}

/// Where in `raw` the path `bare_path` leaves is.
fn bare_span(raw: &str) -> Range<usize> {
    let quoted =
        |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’');
    let path = raw.trim_start_matches(quoted);
    let start = raw.len() - path.len();
    let path = path.trim_end_matches(quoted);
    let path = match path.rsplit_once('#') {
        Some((file, anchor)) if !anchor.contains(['/', '\\']) && has_extension(file) => file,
        _ => path,
    };
    // What `split_line_suffix` leaves is the start of `path`.
    start..start + split_line_suffix(path).path.len()
}

/// A relative path: not absolute, not starting at a drive's root (`\x`), holding a separator,
/// and without a `:` before its first one, which would make it a URL (`https://…`), a host
/// (`host:/srv`) or a drive's current folder (`C:x\y`).
fn is_relative(path: &str) -> bool {
    !is_absolute(path)
        && !path.starts_with('\\')
        && path
            .find(['/', '\\'])
            .is_some_and(|separator| !path[..separator].contains(':'))
}

/// The paths in `text`, as byte ranges, for converting them where they are (see
/// [`PathMapper::convert_paths`]). A token is a word (a run of anything but whitespace), or the
/// inside of a `"…"` or `'…'` run that opens a word and closes on the same line, which may hold
/// spaces. Of a flag's word (`--name=value`, `-n=value`) only the value counts. A token is a path
/// when it holds `/` or `\` and isn't a URL (`scheme://…` or `mailto:…`).
///
/// Linear in `text`: every scan stops where the next one starts (a quoted run at its closing quote
/// or line break, a flag's name at its `=`), so nothing is read twice, however long a line or a
/// word is.
pub fn path_tokens(text: &str) -> Vec<Range<usize>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find(|c: char| !c.is_whitespace()) {
        let word = at + offset;
        let start = flag_value(text, word).unwrap_or(word);
        // A quoted run may end before its word does, or run on past spaces; either way, what
        // follows its closing quote is read afresh.
        let token = match quoted_run(text, start) {
            Some(inside) => {
                at = inside.end + 1;
                inside
            }
            None => {
                at = text[start..]
                    .find(char::is_whitespace)
                    .map_or(text.len(), |n| start + n);
                start..at
            }
        };
        if is_path_token(&text[token.clone()]) {
            tokens.push(token);
        }
    }
    tokens
}

/// Where the value starts in a flag's word at `word` (`--name=value`, `-n=value`): just after its
/// first `=`, when the name before it holds no separator or quote.
fn flag_value(text: &str, word: usize) -> Option<usize> {
    let rest = text.get(word..).filter(|rest| rest.starts_with('-'))?;
    let end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
    let (name, after) = rest.split_at(end);
    let flag = after.starts_with('=') && !name.contains(['/', '\\', '"', '\'']);
    flag.then_some(word + end + 1)
}

/// The inside of the quoted run that opens at `start` in `text`: from a `"` or `'` to the next
/// one like it on the same line. `None` when there is no quote at `start`, or a line break or the
/// end comes first. The scan stops at whichever of those it meets first.
fn quoted_run(text: &str, start: usize) -> Option<Range<usize>> {
    let quote = text[start..]
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\''))?;
    let inside = start + 1;
    let stop = text[inside..].find([quote, '\n', '\r'])?;
    text[inside + stop..]
        .starts_with(quote)
        .then(|| inside..inside + stop)
}

/// Whether a token is a path: it holds a separator and isn't a URL.
fn is_path_token(token: &str) -> bool {
    token.contains(['/', '\\'])
        && !token.contains("://")
        && !token.to_ascii_lowercase().contains("mailto:")
}

/// Whether the last component of `path` is a file name with an extension, such as `plan.md`.
fn has_extension(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// `\\wsl.localhost\Ubuntu\home\me` (or `\\wsl$\…`, with either separator) → `/home/me`.
fn wsl_share_path(raw: &str) -> Option<String> {
    let mut parts = raw
        .strip_prefix(r"\\")
        .or_else(|| raw.strip_prefix("//"))?
        .split(['/', '\\']);
    let host = parts.next()?;
    if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    parts.next().filter(|distro| !distro.is_empty())?;
    let rest: Vec<&str> = parts.filter(|part| !part.is_empty()).collect();
    Some(format!("/{}", rest.join("/")))
}

/// `C:\Users\me` → `/mnt/c/Users/me`.
fn mnt_path(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive.then(|| {
        let letter = char::from(b[0].to_ascii_lowercase());
        linux_join(&format!("/mnt/{letter}"), &raw[3..])
    })
}

/// `base` and the `/`- or `\`-separated `rest`, joined with `/`: the result is a Linux path.
fn linux_join(base: &str, rest: &str) -> String {
    let mut path = base.trim_end_matches('/').to_owned();
    for part in components(rest) {
        path.push('/');
        path.push_str(part);
    }
    if path.is_empty() {
        path.push('/');
    }
    path
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

/// The network host a UNC path names, lowercased: `server` for `\\server\share\…` (either
/// separator) and for the verbatim `\\?\UNC\server\…`. Other verbatim and device paths
/// (`\\?\C:\…`, `\\.\pipe\…`) give `?` or `.`, which no one trusts. `None` for anything
/// that isn't a UNC path, such as `C:\…` or `/home/…`.
pub fn unc_host(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if b.len() < 3 || !matches!(b[0], b'\\' | b'/') || !matches!(b[1], b'\\' | b'/') {
        return None;
    }
    let mut parts = path[2..].split(['\\', '/']);
    let first = parts.next().filter(|host| !host.is_empty())?;
    let verbatim_unc = (first == "?" || first == ".")
        && parts.next().is_some_and(|p| p.eq_ignore_ascii_case("UNC"));
    if verbatim_unc {
        return parts.next().map(str::to_lowercase);
    }
    Some(first.to_lowercase())
}

/// Whether reaching `path` is allowed: always for a local path, and for a UNC path only when its
/// host is one of `trusted` (compared case-insensitively). Windows hands any SMB host the user's
/// credentials, so a note must never make Lectern touch a host the user hasn't chosen.
pub fn unc_host_trusted(path: &str, trusted: &[String]) -> bool {
    match unc_host(path) {
        None => true,
        Some(host) => trusted.iter().any(|t| t.eq_ignore_ascii_case(&host)),
    }
}

/// The asset-protocol URL for a local file, matching Tauri's `convertFileSrc`: `asset_base`
/// followed by the whole path passed through `encodeURIComponent`.
pub fn asset_url(asset_base: &str, path: &Path) -> String {
    format!(
        "{asset_base}{}",
        utf8_percent_encode(&path.to_string_lossy(), URI_COMPONENT)
    )
}

/// The origin of Lectern's page protocol (`lxpage`), which serves HTML pages, PDFs and what a page
/// loads into a frame.
pub const PAGE_ORIGIN: &str = "http://lxpage.localhost";

/// RFC 3986's unreserved characters are the only ones a page URL keeps as they are.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The page-protocol URL of the page at `page`, opened as the open `token`:
/// `http://lxpage.localhost/o/<token>/<file name>`, the file name percent-encoded. The page's
/// relative links resolve under `/o/<token>/`, which names the page's folder (`page_path`), so a
/// page reaches its own folder and the folders below it, and nothing else. `None` for a file name
/// `page_path` would refuse (see `page_segment`), so `page_path` always gives the page back.
pub fn page_url(token: &str, page: &Path) -> Option<String> {
    let name = page_name(page)?;
    Some(format!(
        "{PAGE_ORIGIN}/o/{token}/{}",
        utf8_percent_encode(name, UNRESERVED)
    ))
}

/// The file name of the page at `page` (either separator), as an open's URL names it
/// (`page_url`); `None` for one `page_path` would refuse (see `page_segment`).
pub fn page_name(page: &Path) -> Option<&str> {
    let name = page.to_str()?.rsplit(['\\', '/']).next()?;
    page_segment(name).then_some(name)
}

/// The open a page-protocol URL path names, and the rest of the path below it, still encoded:
/// `/o/<token>/<rest>`, its query and fragment dropped. `None` for any other path, and for an empty
/// token or rest.
pub fn page_token(url_path: &str) -> Option<(&str, &str)> {
    let end = url_path.find(['?', '#']).unwrap_or(url_path.len());
    let (token, rest) = url_path[..end].strip_prefix("/o/")?.split_once('/')?;
    (!token.is_empty() && !rest.is_empty()).then_some((token, rest))
}

/// The local path the URL path `rest`, below an open's `/o/<token>/` (`page_token`), names in the
/// open's folder `folder`: `folder\a\b c.html` for `a/b%20c.html`. Each segment is percent-decoded
/// once and then checked (`page_segment`), so an encoded `..%2F` or `%3A%5C` is refused rather than
/// climbing or switching drive. `None` for an empty segment and for one that isn't UTF-8 once
/// decoded.
pub fn page_path(folder: &str, rest: &str) -> Option<String> {
    let mut out = folder.trim_end_matches(['\\', '/']).to_owned();
    for segment in rest.split('/') {
        let decoded = percent_decode_str(segment).decode_utf8().ok()?;
        if !page_segment(&decoded) {
            return None;
        }
        out.push('\\');
        out.push_str(&decoded);
    }
    Some(out)
}

/// Whether a decoded segment names one file or folder as written: not empty, `.` or `..`; without
/// a separator, `:`, a control character (NUL among them) or one of `<>"|?*`; not ending in `.` or
/// a space, which Windows drops (`.. ` is `..` to it); and not a device's name.
fn page_segment(segment: &str) -> bool {
    !matches!(segment, "" | "." | "..")
        && !segment.ends_with(['.', ' '])
        && !is_device_name(segment)
        && !segment.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
}

/// Whether a file name (extensions and all) names a DOS device (`CON`, `NUL`, `COM1`, …), which
/// Windows opens instead of a file of that name.
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.chars().count() == 4
        && upper
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')))
}
