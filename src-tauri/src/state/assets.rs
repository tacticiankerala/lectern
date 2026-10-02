//! Lectern's image protocol, `lxasset` (`http://lxasset.localhost/<encoded path>` on Windows),
//! which serves the local images rendered documents show.
//!
//! Every check is a pure string check made before anything touches the file system: the path
//! must be an absolute local path, on a trusted network host when it is a UNC path, an image by
//! its extension, and (once `.` and `..` are applied) inside a library root or the folder of a
//! document opened this session. Only then is the file read, capped at 20 MiB. Reaching an SMB
//! host hands it the user's Windows credentials, so a note must never make this touch one.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use lectern_core::library::path_key;
use percent_encoding::percent_decode_str;

use super::trust::Trust;
use crate::shell;

/// The largest image served.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Folders whose images may be served: the library roots, and the folders of documents opened
/// this session. Held as path keys, checked by prefix.
#[derive(Debug, Default)]
pub struct AssetScope {
    roots: Vec<String>,
    opened: HashSet<String>,
}

impl AssetScope {
    pub fn set_roots<P: AsRef<Path>>(&mut self, roots: &[P]) {
        self.roots = roots.iter().map(|root| path_key(root.as_ref())).collect();
    }

    /// Adds the folder of a document that has just opened.
    pub fn add_folder(&mut self, dir: &Path) {
        self.opened.insert(path_key(dir));
    }

    /// Whether the (normalised) `path` is inside an allowed folder.
    fn allows(&self, path: &str) -> bool {
        let key = path_key(Path::new(path));
        self.roots.iter().chain(&self.opened).any(|folder| {
            key == *folder
                || key
                    .strip_prefix(folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not an absolute local path, or one Windows would read differently from how it is written.
    NotLocal,
    UntrustedHost,
    NotAnImage,
    OutsideLibrary,
}

/// A file the protocol may serve.
#[derive(Debug, PartialEq, Eq)]
pub struct AssetFile {
    pub path: PathBuf,
    pub content_type: &'static str,
}

/// Decides whether `raw` (a percent-decoded request path) may be served, without touching the
/// file system.
pub fn decide(raw: &str, trust: &Trust, scope: &AssetScope) -> Result<AssetFile, Refusal> {
    let path = shell::local_path(raw).ok_or(Refusal::NotLocal)?;
    if !trust.allows(path) {
        return Err(Refusal::UntrustedHost);
    }
    let path = normalize(path).ok_or(Refusal::NotLocal)?;
    let content_type = content_type(&path).ok_or(Refusal::NotAnImage)?;
    if !scope.allows(&path) {
        return Err(Refusal::OutsideLibrary);
    }
    Ok(AssetFile {
        path: PathBuf::from(path),
        content_type,
    })
}

/// `path` with `.` and `..` applied and `\` separators: `X:\…` or `\\server\share\…`. `None`
/// when `..` climbs above the drive or share, or when a segment is one Windows would read as
/// something else: a name ending in `.` or a space (`.. ` is `..` to Windows), or a reserved
/// device name (`CON`, `NUL`, `COM1`, …) that would open a device instead of a file.
fn normalize(path: &str) -> Option<String> {
    let (prefix, rest) = if path.as_bytes().get(1) == Some(&b':') {
        (path[..2].to_owned(), &path[2..])
    } else {
        let mut parts = path.trim_start_matches(['\\', '/']).splitn(3, ['\\', '/']);
        let (server, share) = (parts.next()?, parts.next()?);
        (format!(r"\\{server}\{share}"), parts.next().unwrap_or(""))
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split(['\\', '/']) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            name if name.ends_with(['.', ' ']) || is_device_name(name) => return None,
            name => segments.push(name),
        }
    }
    Some(format!("{prefix}\\{}", segments.join("\\")))
}

/// Whether a file name (extensions and all) names a DOS device.
fn is_device_name(name: &str) -> bool {
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

/// The `Content-Type` of an image, by extension; `None` for anything else.
fn content_type(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    })
}

/// What the protocol answers.
#[derive(Debug, PartialEq, Eq)]
pub struct AssetResponse {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub body: Vec<u8>,
}

impl AssetResponse {
    /// As an HTTP response. Images are sniff-proof and sandboxed.
    pub fn into_http(self) -> tauri::http::Response<Vec<u8>> {
        let mut response = tauri::http::Response::builder()
            .status(self.status)
            .header("X-Content-Type-Options", "nosniff")
            .header(
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            )
            .header("Cache-Control", "no-cache");
        if let Some(content_type) = self.content_type {
            response = response.header("Content-Type", content_type);
        }
        response.body(self.body).unwrap_or_else(|_| {
            tauri::http::Response::builder()
                .status(500)
                .body(Vec::new())
                .unwrap_or_default()
        })
    }

    /// A plain refusal, for when there is no app state to ask.
    pub fn refused() -> Self {
        Self::empty(403)
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: None,
            body: Vec::new(),
        }
    }
}

/// Answers a request for `request_path` (the URL path, still percent-encoded, with or without its
/// leading `/`). `decide` judges the decoded path; `read` reads the file once it has been
/// allowed, and never before. 403 when refused or too large, 404 when missing.
pub fn serve(
    request_path: &str,
    decide: impl FnOnce(&str) -> Result<AssetFile, Refusal>,
    read: impl FnOnce(&Path) -> io::Result<Vec<u8>>,
) -> AssetResponse {
    let encoded = request_path.strip_prefix('/').unwrap_or(request_path);
    // A path that isn't valid UTF-8 once decoded is refused: decoding it lossily could alias
    // another file.
    let Ok(raw) = percent_decode_str(encoded).decode_utf8() else {
        log::debug!("lxasset refused a path that isn't UTF-8: {encoded}");
        return AssetResponse::empty(403);
    };
    let file = match decide(&raw) {
        Ok(file) => file,
        Err(refusal) => {
            log::debug!("lxasset refused {raw}: {refusal:?}");
            return AssetResponse::empty(403);
        }
    };
    match read(&file.path) {
        Ok(body) => AssetResponse {
            status: 200,
            content_type: Some(file.content_type),
            body,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => AssetResponse::empty(404),
        Err(e) => {
            log::debug!("lxasset couldn't serve {}: {e}", file.path.display());
            AssetResponse::empty(403)
        }
    }
}

/// Reads a regular file of at most 20 MiB.
pub fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not a file"));
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::other("larger than 20 MiB"));
    }
    let mut body = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_IMAGE_BYTES {
        return Err(io::Error::other("larger than 20 MiB"));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lectern_core::ipc::Settings;

    fn trust() -> Trust {
        Trust::new(&Settings {
            library_roots: vec![
                r"S:\Notes\My Vault\dev".to_owned(),
                r"\\nas\share\notes".to_owned(),
            ],
            ..Settings::default()
        })
    }

    fn scope() -> AssetScope {
        let mut scope = AssetScope::default();
        scope.set_roots(&[r"S:\Notes\My Vault\dev", r"\\nas\share\notes"]);
        scope.add_folder(Path::new(r"C:\Users\me\Downloads\trip"));
        scope
    }

    fn check(raw: &str) -> Result<AssetFile, Refusal> {
        decide(raw, &trust(), &scope())
    }

    #[test]
    fn images_inside_the_library_or_an_opened_folder_are_served() {
        assert_eq!(
            check(r"S:\Notes\My Vault\dev\work\img\logo.PNG"),
            Ok(AssetFile {
                path: PathBuf::from(r"S:\Notes\My Vault\dev\work\img\logo.PNG"),
                content_type: "image/png",
            })
        );
        assert_eq!(
            check(r"s:/notes/my vault/dev/a.svg").map(|f| f.content_type),
            Ok("image/svg+xml")
        );
        assert!(check(r"\\NAS\Share\notes\pics\a.jpg").is_ok());
        assert!(check(r"C:\Users\me\Downloads\trip\photos\day1.webp").is_ok());
        // `.` and a `..` that stays inside are fine.
        assert_eq!(
            check(r"S:\Notes\My Vault\dev\work\.\notes\..\img\a.gif").map(|f| f.path),
            Ok(PathBuf::from(r"S:\Notes\My Vault\dev\work\img\a.gif"))
        );
    }

    #[test]
    fn anything_outside_the_allowed_folders_is_refused() {
        assert_eq!(
            check(r"C:\Windows\Web\Screen\img100.jpg"),
            Err(Refusal::OutsideLibrary)
        );
        assert_eq!(
            check(r"S:\Notes\My Vault\devices\a.png"),
            Err(Refusal::OutsideLibrary)
        );
        assert_eq!(
            check(r"C:\Users\me\Downloads\other.png"),
            Err(Refusal::OutsideLibrary)
        );
        // `..` is applied before the prefix check.
        assert_eq!(
            check(r"S:\Notes\My Vault\dev\..\other\x.png"),
            Err(Refusal::OutsideLibrary)
        );
        assert_eq!(check(r"S:\..\..\x.png"), Err(Refusal::NotLocal));
    }

    #[test]
    fn untrusted_hosts_relative_paths_and_schemes_are_refused() {
        assert_eq!(
            check(r"\\attacker.invalid\s\x.png"),
            Err(Refusal::UntrustedHost)
        );
        assert_eq!(check(r"\\?\UNC\attacker\s\x.png"), Err(Refusal::NotLocal));
        for raw in [
            "img/logo.png",
            r"..\x.png",
            "file:///C:/x.png",
            "lxasset:x.png",
            "x.png",
            "",
        ] {
            assert_eq!(check(raw), Err(Refusal::NotLocal), "{raw}");
        }
    }

    #[test]
    fn names_windows_would_read_differently_are_refused() {
        for raw in [
            r"S:\Notes\My Vault\dev\.. \..\other\x.png",
            r"S:\Notes\My Vault\dev\work.\x.png",
            r"S:\Notes\My Vault\dev\CON.png",
            r"S:\Notes\My Vault\dev\img\com1.jpg",
            r"S:\Notes\My Vault\dev\lpt¹.png",
            r"S:\Notes\My Vault\dev\x.png:stream",
        ] {
            assert_eq!(check(raw), Err(Refusal::NotLocal), "{raw}");
        }
        assert!(check(r"S:\Notes\My Vault\dev\console.png").is_ok());
        assert!(check(r"S:\Notes\My Vault\dev\com10.png").is_ok());
    }

    #[test]
    fn only_images_are_served() {
        for raw in [
            r"S:\Notes\My Vault\dev\notes.md",
            r"S:\Notes\My Vault\dev\page.html",
            r"S:\Notes\My Vault\dev\run.exe",
            r"S:\Notes\My Vault\dev\README",
        ] {
            assert_eq!(check(raw), Err(Refusal::NotAnImage), "{raw}");
        }
    }

    fn no_reads(_: &Path) -> io::Result<Vec<u8>> {
        panic!("the file system was touched")
    }

    #[test]
    fn refusals_never_touch_the_file_system() {
        for path in [
            "/%5C%5Cattacker.invalid%5Cs%5Cx.png",
            "/C%3A%5CWindows%5CWeb%5Cimg.jpg",
            "/S%3A%5CNotes%5CMy%20Vault%5Cdev%5C..%5Cother%5Cx.png",
            "/S%3A%5CNotes%5CMy%20Vault%5Cdev%5Cnotes.md",
            "/img%2Flogo.png",
            "/%FF%FE",
        ] {
            let response = serve(path, check, no_reads);
            assert_eq!(response, AssetResponse::empty(403), "{path}");
        }
    }

    #[test]
    fn an_allowed_image_is_read_and_typed() {
        let response = serve(
            "/S%3A%5CNotes%5CMy%20Vault%5Cdev%5Cimg%5Clogo.png",
            check,
            |path| {
                assert_eq!(path, Path::new(r"S:\Notes\My Vault\dev\img\logo.png"));
                Ok(vec![1, 2, 3])
            },
        );
        assert_eq!(
            response,
            AssetResponse {
                status: 200,
                content_type: Some("image/png"),
                body: vec![1, 2, 3],
            }
        );
        let missing = serve("S%3A%5CNotes%5CMy%20Vault%5Cdev%5Cgone.png", check, |_| {
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert_eq!(missing, AssetResponse::empty(404));
    }
}
