//! What a window's top-level page may navigate to. Only Lectern's own page loads in a window (its
//! origin, and `about:blank`); a web or mail link opens in the default browser or mail app, as a
//! note's external link does; anything else is dropped. A new window is never opened. Frames are
//! not governed here: `on_navigation` never sees them, and the app's CSP limits what they load.
//!
//! An unsandboxed frame (the PDF viewer) can navigate the top-level page, through a link or
//! `top.location`, and that would replace Lectern's interface with whatever it loaded.

use tauri::webview::NewWindowResponse;
use tauri::{Manager, Runtime, Url, WebviewWindowBuilder};

use crate::shell;

/// What happens to a top-level navigation or a new window's URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// Lectern's own page, or `about:blank`: it goes ahead.
    Allow,
    /// An http, https or mailto link: it opens outside Lectern instead.
    External(String),
    /// Anything else: dropped.
    Deny,
}

/// Decides what happens to a navigation to `url` of a window whose page comes from `app`: its own
/// origin and `about:blank` go ahead, a web or mail link opens outside (`shell::web_link`'s rules,
/// as for a note's external link), and anything else is dropped.
pub fn decide(url: &Url, app: &Url) -> Navigation {
    // The scheme too: a `blob:` URL has the origin of the page that made it.
    if url.as_str() == "about:blank"
        || (url.scheme() == app.scheme() && url.origin() == app.origin())
    {
        return Navigation::Allow;
    }
    // Lectern's protocols (`tauri`, `ipc`, `lxasset`, `lxpage`) live at `<name>.localhost`, which
    // names nothing outside Lectern: never handed to the browser.
    if url
        .host_str()
        .is_some_and(|host| host.ends_with(".localhost"))
    {
        return Navigation::Deny;
    }
    match shell::web_link(url.as_str()) {
        Some(link) => Navigation::External(link),
        None => Navigation::Deny,
    }
}

/// Where Lectern's own page loads from, as Tauri loads it: `served_from` (the dev server in a
/// development build, or a URL given as the frontend), else Tauri's protocol at
/// `http://tauri.localhost` (`https://` when the window asks for it).
pub fn app_url(served_from: Option<&Url>, https: bool) -> Url {
    served_from.cloned().unwrap_or_else(|| {
        let scheme = if https { "https" } else { "http" };
        Url::parse(&format!("{scheme}://tauri.localhost")).expect("Tauri's own URL")
    })
}

/// Guards the page of the window `builder` builds, which loads from `app` (`decide`): it
/// navigates only within Lectern's own page and opens no window, and a web or mail link it tries
/// opens outside instead.
pub fn guard<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
    app: Url,
) -> WebviewWindowBuilder<'a, R, M> {
    let opener = app.clone();
    builder
        .on_navigation(move |url| match decide(url, &app) {
            Navigation::Allow => true,
            refused => {
                log::debug!("a window's page tried to navigate to {url}");
                open_outside(refused);
                false
            }
        })
        .on_new_window(move |url, _| {
            log::debug!("a window's page tried to open a window on {url}");
            open_outside(decide(&url, &opener));
            NewWindowResponse::Deny
        })
}

/// Opens an external link in the default browser or mail app, off the main thread the handlers
/// run on: the shell can take a while to answer.
fn open_outside(navigation: Navigation) {
    if let Navigation::External(url) = navigation {
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = shell::open_url(&url) {
                log::warn!("couldn't open {url}: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn release() -> Url {
        app_url(None, false)
    }

    #[test]
    fn the_app_loads_from_tauri_or_the_dev_server() {
        assert_eq!(app_url(None, false).as_str(), "http://tauri.localhost/");
        assert_eq!(app_url(None, true).as_str(), "https://tauri.localhost/");
        let dev = url("http://localhost:1430/");
        assert_eq!(app_url(Some(&dev), false), dev);
    }

    #[test]
    fn lecterns_own_page_and_about_blank_go_ahead() {
        for to in [
            "http://tauri.localhost/",
            "http://tauri.localhost/index.html?open=x#top",
            "http://TAURI.localhost/",
            "about:blank",
        ] {
            assert_eq!(decide(&url(to), &release()), Navigation::Allow, "{to}");
        }
        let dev = url("http://localhost:1430/");
        assert_eq!(
            decide(&url("http://localhost:1430/index.html"), &dev),
            Navigation::Allow
        );
        assert_eq!(
            decide(&url("http://tauri.localhost/"), &dev),
            Navigation::Deny
        );
    }

    #[test]
    fn web_and_mail_links_open_outside() {
        for to in [
            "https://example.com/a?b=c#d",
            "http://example.com/",
            "http://localhost:3000/",
            // A look-alike of the app's origin is just another website.
            "http://tauri.localhost.example.com/",
            "mailto:someone@example.com",
        ] {
            assert_eq!(
                decide(&url(to), &release()),
                Navigation::External(url(to).to_string()),
                "{to}"
            );
        }
    }

    #[test]
    fn everything_else_is_dropped() {
        for to in [
            // Lectern's other protocols, which only ever load inside the page.
            "http://lxpage.localhost/d/s/Notes/report.html",
            "http://lxasset.localhost/S%3A%5Cx.png",
            "http://ipc.localhost/plugin",
            // The app's host on another scheme or port.
            "https://tauri.localhost/",
            "http://tauri.localhost:8080/",
            // Schemes Lectern never hands on.
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "data:text/html,<p>x</p>",
            "blob:http://tauri.localhost/0e1d",
            "ms-settings:privacy",
            "search-ms:query=x",
            "about:srcdoc",
        ] {
            assert_eq!(decide(&url(to), &release()), Navigation::Deny, "{to}");
        }
    }
}
