//! Lectern's page protocol, `lxpage` (`http://lxpage.localhost/o/<token>/…`, from
//! `pathmap::page_url`), which serves HTML pages and PDFs into a frame, and the files a page loads
//! beside it. Every page or PDF open mints a token bound to its window, its page and the page's
//! folder (`PageOpens`): a page's relative URLs resolve under `/o/<token>/`, so it reaches its own
//! folder and the folders below it, and nothing else, no library root and no other note's folder.
//!
//! A request is decided from strings, in this order, before anything touches the file system: the
//! token's open (an unknown token, or a URL of any other form, is 404); the path below it, decoded
//! a segment at a time under the open's folder (`pathmap::page_path`); a trusted host; normalised;
//! a type served here; inside the open's folder. Only then is the file read.
//!
//! The file is then read by its real path, links and junctions resolved, which must still be in the
//! real path of the open's folder: a junction in the folder reaches nothing outside it.
//!
//! Every response but a PDF's carries a sandboxing CSP of its own, so a page or an SVG that ends up
//! in an unsandboxed frame (a link in a PDF can navigate the PDF's frame) is sandboxed all the same.
//! Within a valid open, a script or a font (`SHARED_EXTENSIONS`, by the file's extension) carries
//! `Access-Control-Allow-Origin: null`, as the sandboxed page needs it to load them as module
//! scripts and fonts. Nothing else does, so a page saved among other files can't read the
//! documents or data around it and send them out. The extension decides, as WebView2 tells the
//! protocol nothing of what a request is for (no `Sec-Fetch-Dest`).
//!
//! An open's own HTML page is served with Lectern's bridge inserted at its top, carrying the open's
//! key, its script a nonce fresh for each response, under the policy its *HTML pages* setting gives
//! (`lectern_core::page`). Any other page served through the open (one the frame navigated to, or
//! fetched) gets no bridge and no key, and reports nowhere. Reports come back to `REPORT_PATH`,
//! which takes them without a scope check: they name hosts, never files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lectern_core::ipc::HtmlPages;
use lectern_core::library::path_key;
use lectern_core::library::pathmap::{page_path, page_token};
use lectern_core::page::{
    fresh_token, page_policy, prepare_page, reported_load, PageScope, ReportedLoad,
    SHARED_EXTENSIONS,
};

use super::assets::{self, key_within, Refusal};
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
    /// A script or a font, which the page may read within its open (`SHARED_EXTENSIONS`).
    pub shared: bool,
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
            shared: SHARED_EXTENSIONS.contains(&ext.as_str()),
        })
}

/// A file the protocol may serve.
#[derive(Debug, PartialEq, Eq)]
pub struct PageFile {
    pub path: PathBuf,
    pub kind: PageType,
}

/// Decides whether the file `rest` names (the URL path below the open's `/o/<token>/`, still
/// percent-encoded) may be served for the open `scope`, without touching the file system.
pub fn decide(scope: &PageScope, rest: &str, trust: &Trust) -> Result<PageFile, Refusal> {
    let raw = page_path(&scope.folder, rest).ok_or(Refusal::NotLocal)?;
    let folder = path_key(Path::new(&scope.folder));
    let (path, kind) = assets::admit(&raw, trust, page_type, |path| {
        key_within(&path_key(Path::new(path)), &folder)
    })?;
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
    /// A script or a font answered within a valid open: `Access-Control-Allow-Origin: null`, so
    /// the sandboxed page, whose origin is opaque, may load it.
    pub shared: bool,
    pub body: Vec<u8>,
}

/// How an HTML page is served: under the *HTML pages* setting, its reports going to the open
/// `report` names (a token a window minted), when there is one, and with the bridge carrying `key`
/// when it is the open's own page; without one, it has no bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageOptions {
    pub mode: HtmlPages,
    pub report: Option<String>,
    pub key: Option<String>,
}

impl PageResponse {
    /// A file, as its type is served; `options` are asked for only for an HTML page.
    fn file(kind: PageType, body: Vec<u8>, options: impl FnOnce() -> PageOptions) -> Self {
        match kind.served {
            Served::Page => page(kind, &body, &options()),
            Served::Pdf => Self {
                content_type: Some(kind.content_type.to_owned()),
                policy: None,
                ..Self::ok(body)
            },
            Served::Resource => Self {
                content_type: Some(kind.content_type.to_owned()),
                ..Self::ok(body)
            },
        }
    }

    /// As an HTTP response: sniff-proof, never cached, and readable from the page's opaque origin
    /// only when shared.
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
        if self.shared {
            response = response.header("Access-Control-Allow-Origin", "null");
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

    /// `body`, sandboxed and not shared, to be typed.
    fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: None,
            policy: Some(SANDBOXED.to_owned()),
            report_only: None,
            shared: false,
            body,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            ..Self::ok(Vec::new())
        }
    }

    /// This, a script or a font answered within a valid open.
    fn shared(self) -> Self {
        Self {
            shared: true,
            ..self
        }
    }
}

/// An HTML page, as served: as written, with the bridge of its open's `key` (in `options`, for the
/// open's own page) inserted at its top under a nonce fresh for this response, sandboxed under
/// `options`' policy (`lectern_core::page`). The charset the page declares is kept in the type, as
/// the bridge, ahead of it, may push its declaration past where the browser looks; a UTF-16 page
/// is served as UTF-8.
fn page(kind: PageType, body: &[u8], options: &PageOptions) -> PageResponse {
    let nonce = fresh_token();
    let policy = page_policy(options.mode, &nonce, options.report.as_deref());
    let page = prepare_page(
        body,
        options.key.as_deref().map(|key| (nonce.as_str(), key)),
    );
    let content_type = match page.charset {
        Some(charset) => format!("{}; charset={charset}", kind.content_type),
        None => kind.content_type.to_owned(),
    };
    PageResponse {
        content_type: Some(content_type),
        policy: Some(policy.enforced),
        report_only: policy.report_only,
        ..PageResponse::ok(page.body)
    }
}

/// Takes a policy report: a `POST` of `application/csp-report`, at most 64 KiB, whose load (when it
/// names a host) goes to `accept`. 204 once taken, whatever it said; 405, 415 or 413 otherwise.
pub fn report(
    method: &str,
    content_type: Option<&str>,
    body: &[u8],
    accept: impl FnOnce(ReportedLoad),
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
    if let Some(load) = reported_load(body) {
        accept(load);
    }
    PageResponse::empty(204)
}

/// Answers a request for `url_path` (the URL path, still percent-encoded): `/o/<token>/<rest>`.
/// `scope` gives the token's open, else it is 404; `decide` judges `rest` within it; `read` reads
/// the file for the open, up to the cap its type allows, once it has been allowed, and never
/// before (`read_within`); `options` says how an HTML page read is served, given its open and its
/// path. Within the open, 403 when refused, 404 for a type not served or a missing file, 413 over
/// the cap; every answer for a script or a font it allowed is shared, by the file's extension.
pub fn serve(
    url_path: &str,
    scope: impl FnOnce(&str) -> Option<PageScope>,
    decide: impl FnOnce(&PageScope, &str) -> Result<PageFile, Refusal>,
    read: impl FnOnce(&PageScope, &Path, u64) -> io::Result<Vec<u8>>,
    options: impl FnOnce(&PageScope, &Path) -> PageOptions,
) -> PageResponse {
    let Some((token, rest)) = page_token(url_path) else {
        log::debug!("lxpage has nothing at {url_path}");
        return PageResponse::empty(404);
    };
    let Some(open) = scope(token) else {
        log::debug!("lxpage has no open {token}");
        return PageResponse::empty(404);
    };
    let file = match decide(&open, rest) {
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
    let response = match read(&open, &file.path, file.kind.cap()) {
        Ok(body) => PageResponse::file(file.kind, body, || options(&open, &file.path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => PageResponse::empty(404),
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => PageResponse::empty(413),
        Err(e) => {
            log::debug!("lxpage couldn't serve {}: {e}", file.path.display());
            PageResponse::empty(403)
        }
    };
    if file.kind.shared {
        response.shared()
    } else {
        response
    }
}

/// Reads a file for the protocol, up to `cap` bytes.
pub fn read(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    assets::read_at_most(path, cap)
}

/// Reads the file at `path` for an open whose folder's real path is `real_folder`, up to `cap`
/// bytes: by its own real path, which must be inside that folder, or it is `PermissionDenied`.
/// Links and junctions are resolved, so one in the folder reaches nothing outside it.
pub fn read_within(real_folder: &Path, path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let real = fs::canonicalize(path)?;
    if !real_within(&real, real_folder) {
        log::debug!("lxpage refused {}: it is elsewhere", path.display());
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    read(&real, cap)
}

/// Whether the real path `real` is the real folder `folder` or inside it, compared as their
/// verbatim forms come (`\\?\C:\…`, `\\?\UNC\…`), in any letter case.
pub fn real_within(real: &Path, folder: &Path) -> bool {
    key_within(&path_key(real), &path_key(folder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::TempDir;
    use lectern_core::page::bridge_for;

    fn trust() -> Trust {
        Trust::new(
            &[
                r"S:\Notes\My Vault".to_owned(),
                r"\\nas\share\notes".to_owned(),
            ],
            &[],
        )
    }

    /// The open `t1`: the report in the vault's artifacts folder.
    fn open() -> PageScope {
        open_of(r"S:\Notes\My Vault\artifacts\report.html")
    }

    /// An open `t1` of `page`, in the folder it is in.
    fn open_of(page: &str) -> PageScope {
        let (folder, _) = page.rsplit_once('\\').unwrap();
        PageScope {
            token: "t1".to_owned(),
            page: page.to_owned(),
            folder: if folder.ends_with(':') {
                format!(r"{folder}\")
            } else {
                folder.to_owned()
            },
            key: "k1".to_owned(),
            real_folder: None,
        }
    }

    /// The open `t1`, the only one there is.
    fn opens(token: &str) -> Option<PageScope> {
        (token == "t1").then(open)
    }

    fn check(rest: &str) -> Result<PageFile, Refusal> {
        decide(&open(), rest, &trust())
    }

    fn checks(scope: &PageScope, rest: &str) -> Result<PageFile, Refusal> {
        decide(scope, rest, &trust())
    }

    /// How a page is served by default: scripts and the web, as its open's own page, with its
    /// bridge and its reports going to its open.
    fn full(open: &PageScope, _: &Path) -> PageOptions {
        PageOptions {
            mode: HtmlPages::Full,
            report: Some(open.token.clone()),
            key: Some(open.key.clone()),
        }
    }

    fn no_scope(_: &str) -> Option<PageScope> {
        panic!("an open was looked for")
    }

    fn no_decision(_: &PageScope, _: &str) -> Result<PageFile, Refusal> {
        panic!("a path was decided")
    }

    fn no_options(_: &PageScope, _: &Path) -> PageOptions {
        panic!("only a page asks how it is served")
    }

    fn no_reads(_: &PageScope, _: &Path, _: u64) -> io::Result<Vec<u8>> {
        panic!("the file system was touched")
    }

    /// `body`, whatever is asked for.
    fn reads(body: &'static [u8]) -> impl FnOnce(&PageScope, &Path, u64) -> io::Result<Vec<u8>> {
        move |_, _, _| Ok(body.to_vec())
    }

    fn header(response: &tauri::http::Response<Vec<u8>>, name: &str) -> Option<String> {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().unwrap().to_owned())
    }

    #[test]
    fn pages_pdfs_and_what_pages_load_are_decided_after_decoding_under_the_opens_folder() {
        assert_eq!(
            check("report.HTML"),
            Ok(PageFile {
                path: PathBuf::from(r"S:\Notes\My Vault\artifacts\report.HTML"),
                kind: PageType {
                    content_type: "text/html",
                    served: Served::Page,
                    shared: false,
                },
            })
        );
        assert_eq!(
            check("img/a%20b.svg").map(|f| f.path),
            Ok(PathBuf::from(r"S:\Notes\My Vault\artifacts\img\a b.svg"))
        );
        assert_eq!(
            check("C%23/100%25/Caf%C3%A9%20%E6%97%A5%E6%9C%AC.css").map(|f| f.path),
            Ok(PathBuf::from(
                r"S:\Notes\My Vault\artifacts\C#\100%\Café 日本.css"
            ))
        );
        let pdf = open_of(r"\\nas\share\notes\brief.pdf");
        assert_eq!(
            checks(&pdf, "brief.pdf").map(|f| f.kind.served),
            Ok(Served::Pdf)
        );
        // An open at a drive's root reaches the drive: its folder is all it has.
        let root = open_of(r"C:\report.html");
        assert_eq!(
            checks(&root, "style.css").map(|f| f.path),
            Ok(PathBuf::from(r"C:\style.css"))
        );
    }

    #[test]
    fn every_check_holds_after_decoding() {
        // Segments that decode to a climb, another drive, a stream or a device.
        for rest in [
            "..%2F..%2Fsecret.txt",
            "..%2Fstyle.css",
            "%2E%2E/style.css",
            "%3A%5CWindows%5Cwin.ini",
            "a%00.html",
            "a.html%3Astream",
            "work./a.html",
            "..%20/a.html",
            "CON.html",
            "img/com1.png",
        ] {
            assert_eq!(check(rest), Err(Refusal::NotLocal), "{rest}");
        }
        // An open on a host the user hasn't chosen serves nothing from it.
        let stranger = open_of(r"\\attacker.invalid\s\x.html");
        assert_eq!(checks(&stranger, "x.html"), Err(Refusal::UntrustedHost));
        for rest in ["run.exe", "notes.md", "notes.txt", "README"] {
            assert_eq!(check(rest), Err(Refusal::WrongType), "{rest}");
        }
        // What `..` in an open's folder would reach is outside it: the folder is normalised when
        // the open is minted, and the path checked against it after normalising.
        let climbing = PageScope {
            folder: r"S:\Notes\My Vault\artifacts\..".to_owned(),
            ..open()
        };
        assert_eq!(checks(&climbing, "a.css"), Err(Refusal::Outside));
    }

    /// A URL of no open, an open no window has, and the old drive and share URLs: 404, before any
    /// path is decided or the file system touched, and never shared.
    #[test]
    fn a_request_outside_every_open_is_404_untouched() {
        for url in [
            "/d/s/Notes/My%20Vault/artifacts/report.html",
            "/u/nas/share/notes/brief.pdf",
            "/o/t1",
            "/o//report.html",
            "/report.html",
            "/",
        ] {
            let response = serve(url, no_scope, no_decision, no_reads, no_options);
            assert_eq!(response, PageResponse::empty(404), "{url}");
            assert!(!response.shared);
        }
        let response = serve(
            "/o/0123abcd/report.html",
            opens,
            no_decision,
            no_reads,
            no_options,
        );
        assert_eq!(response, PageResponse::empty(404));
        let response = response.into_http();
        assert_eq!(header(&response, "Access-Control-Allow-Origin"), None);
    }

    /// A refusal within the open is made before the file system is touched, and, as no file was
    /// allowed, is never shared, a script's or a font's included.
    #[test]
    fn refusals_never_touch_the_file_system() {
        for (url, status) in [
            ("/o/t1/..%2F..%2Fsecret.txt", 403),
            ("/o/t1/..%2Fnotes%2Fx.css", 403),
            ("/o/t1/..%2Flib%2Fx.mjs", 403),
            ("/o/t1/%FF.woff2", 403),
            ("/o/t1/%FF.html", 403),
            ("/o/t1/run.exe", 404),
            ("/o/t1/notes.md", 404),
        ] {
            let response = serve(url, opens, check_open, no_reads, no_options);
            assert_eq!(response, PageResponse::empty(status), "{url}");
        }
    }

    fn check_open(open: &PageScope, rest: &str) -> Result<PageFile, Refusal> {
        decide(open, rest, &trust())
    }

    #[test]
    fn an_allowed_file_is_read_up_to_its_types_cap() {
        let pdf = |token: &str| (token == "t1").then(|| open_of(r"S:\Notes\My Vault\brief.pdf"));
        let response = serve(
            "/o/t1/brief.pdf?v=2#page=1",
            pdf,
            check_open,
            |_, path, cap| {
                assert_eq!(path, Path::new(r"S:\Notes\My Vault\brief.pdf"));
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
                shared: false,
                body: b"%PDF".to_vec(),
            }
        );
        let page = serve(
            "/o/t1/report.htm",
            opens,
            check_open,
            |_, _, cap| {
                assert_eq!(cap, 20 * 1024 * 1024);
                Ok(b"<p>Hi</p>".to_vec())
            },
            full,
        );
        assert_eq!(page.content_type.as_deref(), Some("text/html"));
        assert!(page.body.ends_with(b"</script><p>Hi</p>"));
        let failed = |url: &str, error: io::ErrorKind| {
            serve(
                url,
                opens,
                check_open,
                |_, _, _| Err(error.into()),
                no_options,
            )
        };
        assert_eq!(
            failed("/o/t1/gone.css", io::ErrorKind::NotFound),
            PageResponse::empty(404)
        );
        assert_eq!(
            failed("/o/t1/huge.json", io::ErrorKind::FileTooLarge),
            PageResponse::empty(413)
        );
        assert_eq!(
            failed("/o/t1/locked.png", io::ErrorKind::PermissionDenied),
            PageResponse::empty(403)
        );
        // A script or a font the open allows says why it isn't there to the page that loads it.
        assert_eq!(
            failed("/o/t1/gone.mjs", io::ErrorKind::NotFound),
            PageResponse::empty(404).shared()
        );
        assert_eq!(
            failed("/o/t1/huge.woff2", io::ErrorKind::FileTooLarge),
            PageResponse::empty(413).shared()
        );
        assert_eq!(
            failed("/o/t1/locked.ttf", io::ErrorKind::PermissionDenied),
            PageResponse::empty(403).shared()
        );
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

    /// Every type served carries a sandboxing CSP but a PDF. A page may run scripts in its sandbox
    /// (`sandbox allow-scripts`); nothing else may.
    #[test]
    fn every_type_but_a_pdf_is_sandboxed() {
        for &(ext, content_type, served) in TYPES {
            let url = format!("/o/t1/a.{ext}");
            let response = serve(&url, opens, check_open, reads(b"x"), full).into_http();
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
        // Refusals are sandboxed too.
        for status in [403, 404, 413] {
            let response = PageResponse::empty(status).into_http();
            assert_eq!(response.status(), status);
            assert!(header(&response, "Content-Security-Policy")
                .unwrap_or_default()
                .ends_with("; sandbox"));
        }
    }

    /// The scripts and fonts beside a page, by extension: the only files it may read.
    const SHARED: [&str; 6] = ["js", "mjs", "woff", "woff2", "ttf", "otf"];

    /// Within its open, a page may read the scripts and fonts beside it, which it loads as module
    /// scripts and fonts (`Access-Control-Allow-Origin: null`), by their extension alone, as
    /// WebView2 says nothing of what a request is for. Never a page, a PDF, data, an image or a
    /// style sheet, and nothing outside a valid open.
    #[test]
    fn a_page_reads_only_the_scripts_and_fonts_beside_it() {
        let acao = |url: &str| {
            let response = serve(url, opens, check_open, reads(b"x"), full).into_http();
            header(&response, "Access-Control-Allow-Origin")
        };
        for &(ext, ..) in TYPES {
            let expected = SHARED.contains(&ext).then(|| "null".to_owned());
            assert_eq!(acao(&format!("/o/t1/a.{ext}")), expected, "{ext}");
            let upper = ext.to_ascii_uppercase();
            assert_eq!(acao(&format!("/o/t1/lib/A.{upper}")), expected, "{upper}");
            // Outside a valid open, never.
            assert_eq!(acao(&format!("/o/t9/a.{ext}")), None, "{ext}");
            assert_eq!(acao(&format!("/a.{ext}")), None, "{ext}");
        }
        for ext in SHARED {
            assert!(TYPES.iter().any(|&(served, ..)| served == ext), "{ext}");
        }
    }

    /// A file's real path, junctions and links resolved, must be in the real path of its open's
    /// folder: in any letter case, through a drive or a share, each in its verbatim form.
    #[test]
    fn a_real_path_is_within_its_folders_by_their_verbatim_forms() {
        let within = |file: &str, folder: &str| real_within(Path::new(file), Path::new(folder));
        assert!(within(
            r"\\?\C:\Vault\artifacts\a.css",
            r"\\?\C:\Vault\artifacts"
        ));
        assert!(within(
            r"\\?\c:\vault\ARTIFACTS\img\a.png",
            r"\\?\C:\Vault\artifacts\"
        ));
        assert!(within(
            r"\\?\UNC\nas\share\notes\a.css",
            r"\\?\UNC\nas\share\notes"
        ));
        for (file, folder) in [
            (r"\\?\C:\Vault\secret\a.css", r"\\?\C:\Vault\artifacts"),
            (r"\\?\C:\Vault\artifactsx\a.css", r"\\?\C:\Vault\artifacts"),
            (r"\\?\D:\Vault\artifacts\a.css", r"\\?\C:\Vault\artifacts"),
            (
                r"\\?\UNC\nas\share\notesx\a.css",
                r"\\?\UNC\nas\share\notes",
            ),
            (
                r"\\?\UNC\evil\share\notes\a.css",
                r"\\?\UNC\nas\share\notes",
            ),
        ] {
            assert!(!within(file, folder), "{file} in {folder}");
        }
    }

    /// The bridge goes into the open's own page alone, with the open's key; any other page served
    /// through the open (one the frame navigated to) has neither.
    #[test]
    fn only_the_opens_own_page_gets_the_bridge_and_its_key() {
        let html = b"<head><title>Other</title>";
        let own = serve("/o/t1/report.html", opens, check_open, reads(html), full);
        let body = String::from_utf8(own.body).unwrap();
        assert!(body.contains("<script nonce=\""), "{body}");
        assert!(body.contains("\"k1\""), "{body}");
        let other = serve(
            "/o/t1/other.html",
            opens,
            check_open,
            reads(html),
            |_, _| PageOptions {
                mode: HtmlPages::Full,
                report: None,
                key: None,
            },
        );
        assert_eq!(other.body, html);
    }

    /// The acceptance cases: an SVG and JSON are sandboxed, a PDF isn't, and a page runs only
    /// scripts, in its sandbox.
    #[test]
    fn svg_json_pdf_and_page_policies() {
        let policy = |url: &str| serve(url, opens, check_open, reads(b"x"), full).into_http();
        let svg = policy("/o/t1/a.svg");
        assert!(header(&svg, "Content-Security-Policy")
            .unwrap()
            .contains("sandbox"));
        let pdf = policy("/o/t1/a.pdf");
        assert_eq!(header(&pdf, "Content-Security-Policy"), None);
        let json = policy("/o/t1/a.json");
        assert!(header(&json, "Content-Security-Policy")
            .unwrap()
            .contains("sandbox"));
        let html = policy("/o/t1/a.html");
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
                "/o/t1/report.html",
                opens,
                check_open,
                reads(html),
                |_, path| {
                    assert_eq!(path, Path::new(r"S:\Notes\My Vault\artifacts\report.html"));
                    options
                },
            )
        };
        let page = served(full(&open(), Path::new("")));
        let body = String::from_utf8(page.body.clone()).unwrap();
        assert!(body.starts_with("<!doctype html><html><head><script nonce=\""));
        assert!(body.contains(&bridge_for("k1")));
        assert!(body.ends_with("</script><title>Report</title></head><body>x</body></html>"));
        let nonce = nonce_of(&page.body);
        let policy = page_policy(HtmlPages::Full, &nonce, Some("t1"));
        assert_eq!(page.policy, Some(policy.enforced));
        assert_eq!(page.report_only, policy.report_only);
        let response = served(full(&open(), Path::new(""))).into_http();
        assert!(header(&response, "Content-Security-Policy-Report-Only")
            .unwrap()
            .ends_with("report-uri http://lxpage.localhost/__csp-report?t=t1"));
        // A fresh nonce for every response.
        assert_ne!(nonce_of(&served(full(&open(), Path::new(""))).body), nonce);
        // Without its scripts: the bridge alone runs, by its nonce.
        let still = served(PageOptions {
            mode: HtmlPages::Static,
            report: None,
            key: Some("k1".to_owned()),
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
            "/o/t1/report.html",
            opens,
            check_open,
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
        let response = serve("/o/t1/report.html", opens, check_open, reads(page), full).into_http();
        assert_eq!(
            header(&response, "Content-Type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        let body = String::from_utf8(response.body().clone()).unwrap();
        assert!(body.starts_with("<head><script nonce=\""), "{body}");
        assert!(body.ends_with("</script><title>Café</title>"), "{body}");
    }

    /// A report is a POST of a CSP report, of at most 64 KiB; its load, if it names a host, is
    /// taken, blocked or not as the report says.
    #[test]
    fn a_policy_report_is_taken_only_as_the_browser_sends_one() {
        let body = br#"{"csp-report":{"blocked-uri":"https://cdn.example.test/lib.js","disposition":"enforce"}}"#;
        let mut taken = Vec::new();
        let response = report("POST", Some("application/csp-report"), body, |load| {
            taken.push(load);
        });
        assert_eq!(response.status, 204);
        assert_eq!(
            taken,
            [ReportedLoad {
                host: "cdn.example.test".to_owned(),
                blocked: true,
            }]
        );
        let never = |_: ReportedLoad| panic!("taken");
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
