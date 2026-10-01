//! Recognising URLs to the app's own origins. A note must never point an image or a link at
//! Lectern's internal endpoints (the image protocol, IPC, the app page): those are for the app,
//! and an author-written URL to them could reach files or hosts the note shouldn't.

use std::sync::LazyLock;

use url::Url;

/// Schemes the app's webview treats as its own.
pub(super) const INTERNAL_SCHEMES: &[&str] = &["asset", "lxasset", "ipc", "tauri"];

/// Hosts that serve the app's custom protocols on Windows (`http://<scheme>.localhost/`).
const INTERNAL_HOSTS: &[&str] = &[
    "asset.localhost",
    "lxasset.localhost",
    "ipc.localhost",
    "tauri.localhost",
];

/// A stand-in for the page's URL, so a relative URL resolves to a host that isn't ours. The page
/// is served over `http`, so `//host/x` and the slash pairs browsers read as `//` take `http`.
static PAGE_BASE: LazyLock<Url> =
    LazyLock::new(|| Url::parse("http://lectern.invalid/").expect("the base URL is valid"));

/// Whether `url` points at one of the app's internal origins: an internal scheme (`lxasset:…`),
/// or any URL whose host is an internal host once parsed as the browser parses it. The WHATWG
/// parser does the work: it trims controls, drops tabs and newlines, reads `\` as `/` and the
/// pairs `//`, `/\`, `\/` and `\\` as an authority, ignores user info and the port, and maps the
/// host as IDNA does (`ℓ` is `l`, full-width letters are ASCII, `。` is `.`, case is folded).
pub(super) fn is_internal_url(url: &str) -> bool {
    let Some(parsed) = parse_as_browser(url) else {
        return false;
    };
    if INTERNAL_SCHEMES.contains(&parsed.scheme()) {
        return true;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // A trailing dot names the same host to the resolver; a non-special scheme keeps its host's
    // letter case.
    let host = host.to_ascii_lowercase();
    INTERNAL_HOSTS.contains(&host.trim_end_matches('.'))
}

/// `url` parsed as a browser on the app page would: on its own when it carries a scheme, else
/// against the page. A scheme-bearing URL is read as absolute even where the page's own scheme
/// would make it relative (`http:host/x` on an `http` page), since being strict here costs
/// nothing: such a URL could only ever reach the page's own origin.
fn parse_as_browser(url: &str) -> Option<Url> {
    Url::parse(url).or_else(|_| PAGE_BASE.join(url)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_schemes_and_hosts_are_recognised() {
        for url in [
            "lxasset://localhost/C%3A%5Cx.png",
            "ASSET:x",
            "ipc://localhost/plugin",
            "tauri://localhost/",
            "http://lxasset.localhost/%5C%5Cattacker%5Cs%5Cx.png",
            "http://lxasset.localhost/C%3A%5Cx.png",
            "https://LXASSET.localhost./x",
            "http://user:pw@ipc.localhost:8080/x",
            "//tauri.localhost/index.html",
            "http:asset.localhost/x",
            r"http:\\asset.localhost\x",
            "http:///asset.localhost/x",
            "  http://lxasset.localhost/x  ",
            "ht\ntp://asset.localhost/x",
            "http://asset%2Elocalhost/x",
            "http://asset\u{3002}localhost/x",
            "http://\u{ff41}sset.localhost/x",
            "ws://ipc.localhost/",
        ] {
            assert!(is_internal_url(url), "{url}");
        }
    }

    /// Hosts as the browser reads them, after IDNA mapping (`ℓ` is `l`, full-width letters are
    /// ASCII, case is folded), and the slash pairs browsers take for `//` in an `http` page.
    #[test]
    fn browser_host_mapping_and_slash_pairs_are_recognised() {
        for url in [
            "http://\u{2113}xasset.localhost/%5C%5Cattacker%5Cs%5Cx.png",
            "//\u{2113}xasset.localhost/x",
            "http://\u{ff4c}\u{ff58}\u{ff41}\u{ff53}\u{ff53}\u{ff45}\u{ff54}.localhost/x",
            "http://LXASSET.LOCALHOST/x",
            "//LXASSET.LOCALHOST/x",
            r"/\tauri.localhost/index.html",
            r"\/tauri.localhost/index.html",
            r"\\lxasset.localhost\x.png",
            r"http:/\lxasset.localhost/x",
        ] {
            assert!(is_internal_url(url), "{url}");
        }
    }

    #[test]
    fn other_urls_and_paths_are_not_internal() {
        for url in [
            "https://example.com/a.png",
            "//example.com/a.png",
            "http://localhost:8080/a.png",
            "http://asset.localhost.example.com/x",
            "http://example.com/asset.localhost",
            "img/logo.png",
            "../a.png",
            r"C:\pics\a.png",
            r"S:\Notes\My Vault\x.md",
            // A UNC path reads as an authority, but a real host is never one of ours.
            r"\\nas\Shared\a.png",
            r"\\server\share\x.png",
            "/home/me/a.png",
            "notes.md:12",
            "mailto:a@asset.localhost",
            "data:image/png;base64,xx",
            "#frag",
            "",
        ] {
            assert!(!is_internal_url(url), "{url}");
        }
    }
}
