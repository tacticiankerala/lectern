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
//!
//! An HTML page is served with Lectern's bridge inserted at its top, its script carrying a nonce
//! fresh for each response, under the policy its *HTML pages* setting gives (`lectern_core::page`).
//! The browser's reports on what the page loaded come back to `REPORT_PATH`, which takes them
//! without a scope check: they name hosts, never files.

use std::io;
use std::path::{Path, PathBuf};

use lectern_core::ipc::HtmlPages;
use lectern_core::library::pathmap::page_path;
use lectern_core::page::{fresh_token, page_policy, prepare_page, report_host};

use super::assets::{self, AssetScope, Refusal};
use super::trust::Trust;

/// The largest file served, but for a PDF. An HTML page over it doesn't open (`doc::check_size`).
pub(super) const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// The largest PDF served. One over it doesn't open (`doc::check_size`).
pub(super) const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

/// The policy of everything a page loads, and of every refusal: nothing runs or loads, only
/// inline styles apply (an SVG's), and it is sandboxed when opened on its own.
const SANDBOXED: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";
/// The largest policy report taken.
const MAX_REPORT_BYTES: usize = 64 * 1024;

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

/// A request on the page protocol, as Tauri hands it over.
#[derive(Debug, Clone, Copy)]
pub struct PageRequest<'a> {
    pub method: &'a str,
    /// The URL path, still percent-encoded.
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
}

/// What the protocol answers.
#[derive(Debug, PartialEq, Eq)]
pub struct PageResponse {
    pub status: u16,
    /// The `Content-Type`, an HTML page's with the charset it declares.
    pub content_type: Option<String>,
    /// The `Content-Security-Policy`: `None` only for a PDF.
    pub policy: Option<String>,
    /// The `Content-Security-Policy-Report-Only`: an HTML page's under `full`, while a window
    /// waits for its reports.
    pub report_only: Option<String>,
    pub body: Vec<u8>,
}

/// How an HTML page is served: under the *HTML pages* setting, its reports going to the open
/// `report` names (a token a window minted), when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageOptions {
    pub mode: HtmlPages,
    pub report: Option<String>,
}

impl PageResponse {
    /// A file, as its type is served; `options` are asked for only for an HTML page.
    fn file(kind: PageType, body: Vec<u8>, options: impl FnOnce() -> PageOptions) -> Self {
        match kind.served {
            Served::Page => page(kind, &body, &options()),
            Served::Pdf => Self {
                status: 200,
                content_type: Some(kind.content_type.to_owned()),
                policy: None,
                report_only: None,
                body,
            },
            Served::Resource => Self {
                status: 200,
                content_type: Some(kind.content_type.to_owned()),
                policy: Some(SANDBOXED.to_owned()),
                report_only: None,
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
        if let Some(policy) = self.report_only {
            response = response.header("Content-Security-Policy-Report-Only", policy);
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
            policy: Some(SANDBOXED.to_owned()),
            report_only: None,
            body: Vec::new(),
        }
    }
}

/// An HTML page, as served: as written, with the bridge inserted at its top under a nonce fresh
/// for this response, sandboxed under `options`' policy (`lectern_core::page`). The charset the
/// page declares is kept in the type, as the bridge, ahead of it, may push its declaration past
/// where the browser looks; a UTF-16 page is served as UTF-8.
fn page(kind: PageType, body: &[u8], options: &PageOptions) -> PageResponse {
    let nonce = fresh_token();
    let policy = page_policy(options.mode, &nonce, options.report.as_deref());
    let page = prepare_page(body, &nonce);
    let content_type = match page.charset {
        Some(charset) => format!("{}; charset={charset}", kind.content_type),
        None => kind.content_type.to_owned(),
    };
    PageResponse {
        status: 200,
        content_type: Some(content_type),
        policy: Some(policy.enforced),
        report_only: policy.report_only,
        body: page.body,
    }
}

/// Takes a policy report: a `POST` of `application/csp-report`, at most 64 KiB, whose host (when
/// it names one) goes to `accept`. 204 once taken, whatever it said; 405, 415 or 413 otherwise.
pub fn report(
    method: &str,
    content_type: Option<&str>,
    body: &[u8],
    accept: impl FnOnce(String),
) -> PageResponse {
    if method != "POST" {
        return PageResponse::empty(405);
    }
    let reported = content_type.is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/csp-report"))
    });
    if !reported {
        return PageResponse::empty(415);
    }
    if body.len() > MAX_REPORT_BYTES {
        return PageResponse::empty(413);
    }
    if let Some(host) = report_host(body) {
        accept(host);
    }
    PageResponse::empty(204)
}

/// Answers a request for `url_path` (the URL path, still percent-encoded). `decide` judges it;
/// `read` reads the file, up to the cap its type allows, once it has been allowed, and never
/// before; `options` says how an HTML page read is served. 403 when refused, 404 for a type not
/// served or a missing file, 413 over the cap.
pub fn serve(
    url_path: &str,
    decide: impl FnOnce(&str) -> Result<PageFile, Refusal>,
    read: impl FnOnce(&Path, u64) -> io::Result<Vec<u8>>,
    options: impl FnOnce() -> PageOptions,
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
        Ok(body) => PageResponse::file(file.kind, body, options),
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
    use lectern_core::page::BRIDGE;

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

    /// How a page is served by default: scripts and the web, its reports going to the open `t1`.
    fn full() -> PageOptions {
        PageOptions {
            mode: HtmlPages::Full,
            report: Some("t1".to_owned()),
        }
    }

    fn no_options() -> PageOptions {
        panic!("only a page asks how it is served")
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
            let response = serve(url, check, no_reads, no_options);
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
            no_options,
        );
        assert_eq!(
            response,
            PageResponse {
                status: 200,
                content_type: Some("application/pdf".to_owned()),
                policy: None,
                report_only: None,
                body: b"%PDF".to_vec(),
            }
        );
        let page = serve(
            "/d/s/Notes/My%20Vault/report.htm",
            check,
            |_, cap| {
                assert_eq!(cap, 20 * 1024 * 1024);
                Ok(b"<p>Hi</p>".to_vec())
            },
            full,
        );
        assert_eq!(page.content_type.as_deref(), Some("text/html"));
        assert!(page.body.ends_with(b"</script><p>Hi</p>"));
        let missing = serve(
            "/d/s/Notes/My%20Vault/gone.css",
            check,
            |_, _| Err(io::ErrorKind::NotFound.into()),
            no_options,
        );
        assert_eq!(missing, PageResponse::empty(404));
        let large = serve(
            "/d/s/Notes/My%20Vault/huge.pdf",
            check,
            |_, _| Err(io::ErrorKind::FileTooLarge.into()),
            no_options,
        );
        assert_eq!(large, PageResponse::empty(413));
        let denied = serve(
            "/d/s/Notes/My%20Vault/locked.png",
            check,
            |_, _| Err(io::ErrorKind::PermissionDenied.into()),
            no_options,
        );
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
    /// A page may run scripts in its sandbox (`sandbox allow-scripts`); nothing else may.
    #[test]
    fn every_type_but_a_pdf_is_sandboxed_and_none_is_shared() {
        for &(ext, content_type, served) in TYPES {
            let url = format!("/d/s/Notes/My%20Vault/a.{ext}");
            let response = serve(&url, check, reads(b"x"), full).into_http();
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
            match served {
                Served::Pdf => assert_eq!(policy, None, "{ext}"),
                Served::Page => {
                    let policy = policy.unwrap_or_default();
                    assert!(
                        policy.starts_with("sandbox allow-scripts;"),
                        "{ext}: {policy}"
                    );
                    assert!(!policy.contains("allow-same-origin"), "{ext}: {policy}");
                }
                Served::Resource => {
                    let policy = policy.unwrap_or_default();
                    assert!(
                        policy.split(';').any(|d| d.trim() == "sandbox"),
                        "{ext}: {policy}"
                    );
                    assert!(policy.contains("default-src 'none'"), "{ext}: {policy}");
                }
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

    /// The acceptance cases: an SVG and JSON are sandboxed, a PDF isn't, and a page runs only
    /// scripts, in its sandbox.
    #[test]
    fn svg_json_pdf_and_page_policies() {
        let policy = |url: &str| serve(url, check, reads(b"x"), full).into_http();
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
        assert!(header(&html, "Content-Security-Policy")
            .unwrap()
            .starts_with("sandbox allow-scripts; default-src 'self' https:"));
    }

    /// The nonce in a page's script element, and the page after it.
    fn nonce_of(body: &[u8]) -> String {
        let body = String::from_utf8_lossy(body);
        let at = body.find("<script nonce=\"").expect("the bridge is in") + 15;
        body[at..at + 32].to_owned()
    }

    /// A page gets the bridge at its top, under a nonce fresh for each response, and the policy of
    /// its setting, its reports going to its open.
    #[test]
    fn a_page_gets_the_bridge_under_its_settings_policy() {
        let html = b"<!doctype html><html><head><title>Report</title></head><body>x</body></html>";
        let served = |options: PageOptions| {
            serve(
                "/d/s/Notes/My%20Vault/report.html",
                check,
                reads(html),
                || options,
            )
        };
        let page = served(full());
        let body = String::from_utf8(page.body.clone()).unwrap();
        assert!(body.starts_with("<!doctype html><html><head><script nonce=\""));
        assert!(body.contains(BRIDGE));
        assert!(body.ends_with("</script><title>Report</title></head><body>x</body></html>"));
        let nonce = nonce_of(&page.body);
        let policy = page_policy(HtmlPages::Full, &nonce, Some("t1"));
        assert_eq!(page.policy, Some(policy.enforced));
        assert_eq!(page.report_only, policy.report_only);
        let response = served(full()).into_http();
        assert!(header(&response, "Content-Security-Policy-Report-Only")
            .unwrap()
            .ends_with("report-uri http://lxpage.localhost/__csp-report?t=t1"));
        // A fresh nonce for every response.
        assert_ne!(nonce_of(&served(full()).body), nonce);
        // Without its scripts: the bridge alone runs, by its nonce.
        let still = served(PageOptions {
            mode: HtmlPages::Static,
            report: None,
        });
        let nonce = nonce_of(&still.body);
        assert!(still
            .policy
            .unwrap()
            .contains(&format!("script-src 'nonce-{nonce}';")));
        assert_eq!(still.report_only, None);
    }

    /// The charset a page declares rides on its type, as the bridge goes in ahead of it.
    #[test]
    fn a_page_keeps_the_charset_it_declares() {
        let page = serve(
            "/d/s/Notes/My%20Vault/report.html",
            check,
            reads(b"<html><head><meta charset=\"Windows-1252\"><title>Caf\xe9</title>"),
            full,
        );
        assert_eq!(
            page.content_type.as_deref(),
            Some("text/html; charset=windows-1252")
        );
        assert!(page.body.ends_with(b"<title>Caf\xe9</title>"));
    }

    /// A page in UTF-16 goes out as UTF-8, its type saying so.
    #[test]
    fn a_utf16_page_is_served_as_utf8() {
        let mut page = vec![0xFF, 0xFE];
        page.extend(
            "<head><title>Café</title>"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        let page: &'static [u8] = page.leak();
        let response = serve(
            "/d/s/Notes/My%20Vault/report.html",
            check,
            reads(page),
            full,
        )
        .into_http();
        assert_eq!(
            header(&response, "Content-Type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        let body = String::from_utf8(response.body().clone()).unwrap();
        assert!(body.starts_with("<head><script nonce=\""), "{body}");
        assert!(body.ends_with("</script><title>Café</title>"), "{body}");
    }

    /// A report is a POST of a CSP report, of at most 64 KiB; its host, if it names one, is taken.
    #[test]
    fn a_policy_report_is_taken_only_as_the_browser_sends_one() {
        let body = br#"{"csp-report":{"blocked-uri":"https://cdn.example.test/lib.js"}}"#;
        let mut taken = Vec::new();
        let response = report("POST", Some("application/csp-report"), body, |host| {
            taken.push(host);
        });
        assert_eq!(response.status, 204);
        assert_eq!(taken, ["cdn.example.test"]);
        let never = |_: String| panic!("taken");
        assert_eq!(
            report("GET", Some("application/csp-report"), body, never).status,
            405
        );
        assert_eq!(
            report("POST", Some("application/json"), body, never).status,
            415
        );
        assert_eq!(report("POST", None, body, never).status, 415);
        let huge = vec![b' '; MAX_REPORT_BYTES + 1];
        assert_eq!(
            report("POST", Some("application/csp-report"), &huge, never).status,
            413
        );
        // An inline script names no host: taken, and nothing comes of it.
        let inline = br#"{"csp-report":{"blocked-uri":"inline"}}"#;
        assert_eq!(
            report("POST", Some("application/csp-report"), inline, never).status,
            204
        );
        let response = report(
            "POST",
            Some("application/csp-report; charset=utf-8"),
            body,
            |_| (),
        )
        .into_http();
        assert_eq!(header(&response, "Access-Control-Allow-Origin"), None);
    }
}
