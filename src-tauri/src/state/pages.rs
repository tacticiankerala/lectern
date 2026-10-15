//! Lectern's page protocol, `lxpage` (`http://lxpage.localhost/d/<drive>/…` and
//! `/u/<host>/<share>/…`, from `pathmap::page_url`), which serves HTML pages and PDFs into a frame,
//! and the files a page loads beside it. Its URLs are hierarchical, so a page's relative links
//! resolve as they would on disk.
//!
//! The request path is decoded one segment at a time (`pathmap::page_path`), then put through the
//! image protocol's checks (`assets::admit`): absolute and local, on a trusted host, normalised, a
//! type served here, and inside a library root or the folder of a document opened this session.
//! Nothing touches the file system before the file is read.
//!
//! Every response but a PDF's carries a sandboxing CSP of its own, so a page or an SVG that ends up
//! in an unsandboxed frame (a link in a PDF can navigate the PDF's frame) is sandboxed all the same.
//! None sends `Access-Control-Allow-Origin`, so a page can't read the files beside it.

use std::io;
use std::path::{Path, PathBuf};

use lectern_core::library::pathmap::page_path;

use super::assets::{self, AssetScope, Refusal};
use super::trust::Trust;

/// The largest file served, but for a PDF. An HTML page over it doesn't open (`doc::check_size`).
pub(super) const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// The largest PDF served. One over it doesn't open (`doc::check_size`).
pub(super) const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

/// The policy of everything a page loads, and of every refusal: nothing runs or loads, only
/// inline styles apply (an SVG's), and it is sandboxed when opened on its own.
const SANDBOXED: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";
/// An HTML page's policy: sandboxed, with no script run and nothing loaded.
const PAGE_POLICY: &str = "default-src 'none'; sandbox";

/// How a type is served: its cap and its policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// An HTML page.
    Page,
    /// A PDF, which WebView2's own viewer shows, and only unsandboxed.
    Pdf,
    /// What a page loads: styles, scripts, data, images and fonts.
    Resource,
}

/// The types served, by extension: their `Content-Type`, and how they are served.
const TYPES: &[(&str, &str, Served)] = &[
    ("html", "text/html", Served::Page),
    ("htm", "text/html", Served::Page),
    ("css", "text/css", Served::Resource),
    ("js", "text/javascript", Served::Resource),
    ("mjs", "text/javascript", Served::Resource),
    ("json", "application/json", Served::Resource),
    ("png", "image/png", Served::Resource),
    ("jpg", "image/jpeg", Served::Resource),
    ("jpeg", "image/jpeg", Served::Resource),
    ("gif", "image/gif", Served::Resource),
    ("webp", "image/webp", Served::Resource),
    ("avif", "image/avif", Served::Resource),
    ("svg", "image/svg+xml", Served::Resource),
    ("ico", "image/x-icon", Served::Resource),
    ("bmp", "image/bmp", Served::Resource),
    ("woff", "font/woff", Served::Resource),
    ("woff2", "font/woff2", Served::Resource),
    ("ttf", "font/ttf", Served::Resource),
    ("otf", "font/otf", Served::Resource),
    ("pdf", "application/pdf", Served::Pdf),
];

/// What a served file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageType {
    pub content_type: &'static str,
    pub served: Served,
}

impl PageType {
    /// The largest file of this type served.
    fn cap(self) -> u64 {
        match self.served {
            Served::Pdf => MAX_PDF_BYTES,
            Served::Page | Served::Resource => MAX_BYTES,
        }
    }
}

/// The type of the file at `path`, by its extension in any letter case; `None` for one not served.
fn page_type(path: &str) -> Option<PageType> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    TYPES
        .iter()
        .find(|(extension, ..)| *extension == ext)
        .map(|&(_, content_type, served)| PageType {
            content_type,
            served,
        })
}

/// A file the protocol may serve.
#[derive(Debug, PartialEq, Eq)]
pub struct PageFile {
    pub path: PathBuf,
    pub kind: PageType,
}

/// Decides whether the file `url_path` names (the URL path, still percent-encoded) may be served,
/// without touching the file system.
pub fn decide(url_path: &str, trust: &Trust, scope: &AssetScope) -> Result<PageFile, Refusal> {
    let raw = page_path(url_path).ok_or(Refusal::NotLocal)?;
    let (path, kind) = assets::admit(&raw, trust, scope, page_type)?;
    Ok(PageFile {
        path: PathBuf::from(path),
        kind,
    })
}

/// What the protocol answers.
#[derive(Debug, PartialEq, Eq)]
pub struct PageResponse {
    pub status: u16,
    pub content_type: Option<&'static str>,
    /// The `Content-Security-Policy`: `None` only for a PDF.
    pub policy: Option<&'static str>,
    pub body: Vec<u8>,
}

impl PageResponse {
    /// A file, as its type is served.
    fn file(kind: PageType, body: Vec<u8>) -> Self {
        match kind.served {
            Served::Page => page(kind, body),
            Served::Pdf => Self {
                status: 200,
                content_type: Some(kind.content_type),
                policy: None,
                body,
            },
            Served::Resource => Self {
                status: 200,
                content_type: Some(kind.content_type),
                policy: Some(SANDBOXED),
                body,
            },
        }
    }

    /// As an HTTP response: sniff-proof, never cached, and never readable from another origin.
    pub fn into_http(self) -> tauri::http::Response<Vec<u8>> {
        let mut response = tauri::http::Response::builder()
            .status(self.status)
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-cache");
        if let Some(content_type) = self.content_type {
            response = response.header("Content-Type", content_type);
        }
        if let Some(policy) = self.policy {
            response = response.header("Content-Security-Policy", policy);
        }
        response.body(self.body).unwrap_or_else(|_| {
            tauri::http::Response::builder()
                .status(500)
                .header("Content-Security-Policy", SANDBOXED)
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
            policy: Some(SANDBOXED),
            body: Vec::new(),
        }
    }
}

/// An HTML page, as served: as written, sandboxed, with its scripts off.
fn page(kind: PageType, body: Vec<u8>) -> PageResponse {
    PageResponse {
        status: 200,
        content_type: Some(kind.content_type),
        policy: Some(PAGE_POLICY),
        body,
    }
}

/// Answers a request for `url_path` (the URL path, still percent-encoded). `decide` judges it;
/// `read` reads the file, up to the cap its type allows, once it has been allowed, and never
/// before. 403 when refused, 404 for a type not served or a missing file, 413 over the cap.
pub fn serve(
    url_path: &str,
    decide: impl FnOnce(&str) -> Result<PageFile, Refusal>,
    read: impl FnOnce(&Path, u64) -> io::Result<Vec<u8>>,
) -> PageResponse {
    let file = match decide(url_path) {
        Ok(file) => file,
        Err(refusal) => {
            log::debug!("lxpage refused {url_path}: {refusal:?}");
            let status = if refusal == Refusal::WrongType {
                404
            } else {
                403
            };
            return PageResponse::empty(status);
        }
    };
    match read(&file.path, file.kind.cap()) {
        Ok(body) => PageResponse::file(file.kind, body),
        Err(e) if e.kind() == io::ErrorKind::NotFound => PageResponse::empty(404),
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => PageResponse::empty(413),
        Err(e) => {
            log::debug!("lxpage couldn't serve {}: {e}", file.path.display());
            PageResponse::empty(403)
        }
    }
}

/// Reads a file for the protocol, up to `cap` bytes.
pub fn read(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    assets::read_at_most(path, cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::TempDir;

    fn trust() -> Trust {
        Trust::new(
            &[
                r"S:\Notes\My Vault".to_owned(),
                r"\\nas\share\notes".to_owned(),
            ],
            &[],
        )
    }

    fn scope() -> AssetScope {
        let mut scope = AssetScope::default();
        scope.set_roots(&[r"S:\Notes\My Vault", r"\\nas\share\notes"]);
        scope.add_folder(Path::new(r"C:\Users\me\Downloads\trip"));
        scope
    }

    fn check(url_path: &str) -> Result<PageFile, Refusal> {
        decide(url_path, &trust(), &scope())
    }

    fn no_reads(_: &Path, _: u64) -> io::Result<Vec<u8>> {
        panic!("the file system was touched")
    }

    /// `body`, whatever is asked for.
    fn reads(body: &'static [u8]) -> impl FnOnce(&Path, u64) -> io::Result<Vec<u8>> {
        move |_, _| Ok(body.to_vec())
    }

    fn header(response: &tauri::http::Response<Vec<u8>>, name: &str) -> Option<String> {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().unwrap().to_owned())
    }

    #[test]
    fn pages_pdfs_and_what_pages_load_are_decided_after_decoding() {
        assert_eq!(
            check("/d/s/Notes/My%20Vault/artifacts/report.HTML"),
            Ok(PageFile {
                path: PathBuf::from(r"S:\Notes\My Vault\artifacts\report.HTML"),
                kind: PageType {
                    content_type: "text/html",
                    served: Served::Page,
                },
            })
        );
        assert_eq!(
            check("/u/nas/share/notes/brief.pdf?v=2").map(|f| f.kind.served),
            Ok(Served::Pdf)
        );
        assert_eq!(
            check("/d/c/Users/me/Downloads/trip/img/a%20b.svg").map(|f| f.path),
            Ok(PathBuf::from(r"C:\Users\me\Downloads\trip\img\a b.svg"))
        );
        assert_eq!(
            check("/d/s/Notes/My%20Vault/C%23/100%25/Caf%C3%A9%20%E6%97%A5%E6%9C%AC.css")
                .map(|f| f.path),
            Ok(PathBuf::from(r"S:\Notes\My Vault\C#\100%\Café 日本.css"))
        );
    }

    #[test]
    fn every_check_of_the_image_protocol_holds_after_decoding() {
        // The URL itself: segments that decode to a climb, another drive or a forbidden name.
        for url in [
            "/d/s/Notes/My%20Vault/..%2F..%2Fsecret.txt",
            "/d/s/Notes/My%20Vault/%3A%5CWindows%5Cwin.ini",
            "/d/s/Notes/My%20Vault/a%00.html",
            "/x/s/Notes/a.html",
            "/u/nas",
        ] {
            assert_eq!(check(url), Err(Refusal::NotLocal), "{url}");
        }
        // A name Windows would read as another file, or as a device.
        for url in [
            "/d/s/Notes/My%20Vault/work./a.html",
            "/d/s/Notes/My%20Vault/..%20/a.html",
            "/d/s/Notes/My%20Vault/CON.html",
        ] {
            assert_eq!(check(url), Err(Refusal::NotLocal), "{url}");
        }
        assert_eq!(
            check("/u/attacker.invalid/s/x.html"),
            Err(Refusal::UntrustedHost)
        );
        for url in [
            "/d/s/Notes/My%20Vault/run.exe",
            "/d/s/Notes/My%20Vault/notes.md",
            "/d/s/Notes/My%20Vault/README",
        ] {
            assert_eq!(check(url), Err(Refusal::WrongType), "{url}");
        }
        for url in [
            "/d/c/Windows/Web/a.html",
            "/d/s/Notes/My%20Vaults/a.pdf",
            "/u/nas/share/other/a.pdf",
        ] {
            assert_eq!(check(url), Err(Refusal::OutsideLibrary), "{url}");
        }
    }

    #[test]
    fn refusals_never_touch_the_file_system() {
        for (url, status) in [
            ("/d/s/Notes/My%20Vault/..%2F..%2Fsecret.txt", 403),
            ("/d/s/Notes/..%2FMy%20Vault%2Fa.html", 403),
            ("/d/c/Windows/Web/a.html", 403),
            ("/u/attacker.invalid/s/x.pdf", 403),
            ("/d/s/Notes/My%20Vault/%FF.html", 403),
            ("/d/s/Notes/My%20Vault/run.exe", 404),
            ("/d/s/Notes/My%20Vault/notes.md", 404),
        ] {
            let response = serve(url, check, no_reads);
            assert_eq!(response, PageResponse::empty(status), "{url}");
        }
    }

    #[test]
    fn an_allowed_file_is_read_up_to_its_types_cap() {
        let response = serve(
            "/d/s/Notes/My%20Vault/artifacts/brief.pdf",
            check,
            |path, cap| {
                assert_eq!(path, Path::new(r"S:\Notes\My Vault\artifacts\brief.pdf"));
                assert_eq!(cap, 100 * 1024 * 1024);
                Ok(b"%PDF".to_vec())
            },
        );
        assert_eq!(
            response,
            PageResponse {
                status: 200,
                content_type: Some("application/pdf"),
                policy: None,
                body: b"%PDF".to_vec(),
            }
        );
        let page = serve("/d/s/Notes/My%20Vault/report.htm", check, |_, cap| {
            assert_eq!(cap, 20 * 1024 * 1024);
            Ok(b"<p>Hi</p>".to_vec())
        });
        assert_eq!(page.content_type, Some("text/html"));
        assert_eq!(page.body, b"<p>Hi</p>");
        let missing = serve("/d/s/Notes/My%20Vault/gone.css", check, |_, _| {
            Err(io::ErrorKind::NotFound.into())
        });
        assert_eq!(missing, PageResponse::empty(404));
        let large = serve("/d/s/Notes/My%20Vault/huge.pdf", check, |_, _| {
            Err(io::ErrorKind::FileTooLarge.into())
        });
        assert_eq!(large, PageResponse::empty(413));
        let denied = serve("/d/s/Notes/My%20Vault/locked.png", check, |_, _| {
            Err(io::ErrorKind::PermissionDenied.into())
        });
        assert_eq!(denied, PageResponse::empty(403));
    }

    #[test]
    fn a_file_over_its_cap_is_too_large() {
        let dir = TempDir::new();
        let file = dir.file("brief.pdf", "0123456789");
        assert_eq!(read(&file, 10).unwrap(), b"0123456789");
        assert_eq!(
            read(&file, 9).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
    }

    /// Every type served carries a sandboxing CSP but a PDF, and none lets another origin read it.
    #[test]
    fn every_type_but_a_pdf_is_sandboxed_and_none_is_shared() {
        for &(ext, content_type, served) in TYPES {
            let url = format!("/d/s/Notes/My%20Vault/a.{ext}");
            let response = serve(&url, check, reads(b"x")).into_http();
            assert_eq!(response.status(), 200, "{ext}");
            assert_eq!(
                header(&response, "Content-Type").as_deref(),
                Some(content_type)
            );
            assert_eq!(
                header(&response, "X-Content-Type-Options").as_deref(),
                Some("nosniff")
            );
            assert_eq!(
                header(&response, "Cache-Control").as_deref(),
                Some("no-cache")
            );
            assert_eq!(header(&response, "Access-Control-Allow-Origin"), None);
            let policy = header(&response, "Content-Security-Policy");
            if served == Served::Pdf {
                assert_eq!(policy, None, "{ext}");
            } else {
                let policy = policy.unwrap_or_default();
                assert!(
                    policy.split(';').any(|d| d.trim() == "sandbox"),
                    "{ext}: {policy}"
                );
                assert!(policy.contains("default-src 'none'"), "{ext}: {policy}");
            }
        }
        // Refusals too.
        for status in [403, 404, 413] {
            let response = PageResponse::empty(status).into_http();
            assert_eq!(response.status(), status);
            assert_eq!(header(&response, "Access-Control-Allow-Origin"), None);
            assert!(header(&response, "Content-Security-Policy")
                .unwrap_or_default()
                .ends_with("; sandbox"));
        }
    }

    /// The acceptance cases: an SVG and JSON are sandboxed, a PDF isn't, and a page runs nothing.
    #[test]
    fn svg_json_pdf_and_page_policies() {
        let policy = |url: &str| serve(url, check, reads(b"x")).into_http();
        let svg = policy("/d/s/Notes/My%20Vault/a.svg");
        assert!(header(&svg, "Content-Security-Policy")
            .unwrap()
            .contains("sandbox"));
        let pdf = policy("/d/s/Notes/My%20Vault/a.pdf");
        assert_eq!(header(&pdf, "Content-Security-Policy"), None);
        let json = policy("/d/s/Notes/My%20Vault/a.json");
        assert!(header(&json, "Content-Security-Policy")
            .unwrap()
            .contains("sandbox"));
        assert_eq!(header(&json, "Access-Control-Allow-Origin"), None);
        let html = policy("/d/s/Notes/My%20Vault/a.html");
        assert_eq!(
            header(&html, "Content-Security-Policy").as_deref(),
            Some("default-src 'none'; sandbox")
        );
    }
}
