//! HTML pages as Lectern serves them into their frame: the bridge inserted at the top, the policy
//! each *HTML pages* setting gives, the encoding a page declares, and the policy reports that name
//! the hosts a page loaded from.

use std::time::{Duration, Instant};

use lectern_core::ipc::{HtmlPages, PageLoads};
use lectern_core::page::{
    declared_charset, fresh_token, inject_bridge, page_policy, prepare_page, report_host,
    PageReports, ReportSend, BRIDGE, REPORT_THROTTLE,
};

const NONCE: &str = "0123456789abcdef0123456789abcdef";

/// The page with the bridge in, as text, with the bridge itself shortened to `BRIDGE`.
fn injected(html: &str) -> String {
    let out = String::from_utf8(inject_bridge(html.as_bytes(), NONCE)).unwrap();
    out.replace(BRIDGE, "BRIDGE")
}

fn script() -> String {
    format!(r#"<script nonce="{NONCE}">BRIDGE</script>"#)
}

#[test]
fn the_bridge_goes_right_after_the_head_start_tag() {
    let s = script();
    assert_eq!(
        injected("<!doctype html><html lang=\"en\"><head><title>T</title></head><body>x</body>"),
        format!("<!doctype html><html lang=\"en\"><head>{s}<title>T</title></head><body>x</body>")
    );
    // In any letter case, with attributes (one holding a `>`), after comments and whitespace.
    assert_eq!(
        injected("\u{feff}<!DOCTYPE html>\n<!-- <head> -->\n<HTML>\n  <HEAD data-x=\"a>b\" >\n<meta charset=\"utf-8\">"),
        format!("\u{feff}<!DOCTYPE html>\n<!-- <head> -->\n<HTML>\n  <HEAD data-x=\"a>b\" >{s}\n<meta charset=\"utf-8\">")
    );
    // `<header>` isn't `<head>`.
    assert_eq!(
        injected("<html><header>x</header>"),
        format!("<html>{s}<header>x</header>")
    );
}

#[test]
fn without_a_head_it_goes_after_the_html_start_tag_then_the_doctype_then_first() {
    let s = script();
    assert_eq!(
        injected("<!doctype html><html><body><p>Hi</p>"),
        format!("<!doctype html><html>{s}<body><p>Hi</p>")
    );
    assert_eq!(
        injected("<!doctype html>\n<p>Hi</p>"),
        format!("<!doctype html>{s}\n<p>Hi</p>")
    );
    assert_eq!(injected("<p>Hi</p>"), format!("{s}<p>Hi</p>"));
    assert_eq!(injected(""), s);
    // After a byte order mark, which must stay first.
    assert_eq!(
        injected("\u{feff}<p>Hi</p>"),
        format!("\u{feff}{s}<p>Hi</p>")
    );
    // A `<head>` in the body, as text or in a script, is no head start tag.
    assert_eq!(
        injected("<html><body><script>document.write('<head>')</script>"),
        format!("<html>{s}<body><script>document.write('<head>')</script>")
    );
    assert_eq!(injected("Hello <head>"), format!("{s}Hello <head>"));
}

/// The prologue as the browser's tokenizer reads it, so no page can put a script of its own ahead
/// of the bridge, or the bridge inside an attribute value.
#[test]
fn the_prologue_is_read_as_the_browser_reads_it() {
    let s = script();
    // `<!-->` and `<!--->` are whole comments: what follows is the page's script, so the bridge
    // goes first.
    for page in [
        "<!--><script>x()</script><!-- --><head>",
        "<!---><script>x()</script><!-- --><head>",
    ] {
        assert_eq!(injected(page), format!("{s}{page}"), "{page}");
    }
    // A comment also ends at `--!>`.
    assert_eq!(
        injected("<!-- a --!><head><title>T</title>"),
        format!("<!-- a --!><head>{s}<title>T</title>")
    );
    assert_eq!(
        injected("<!-- a --!><script>x()</script><head>"),
        format!("{s}<!-- a --!><script>x()</script><head>")
    );
    // A quote starts a value only after `=`: elsewhere it is part of an attribute's name.
    assert_eq!(
        injected(r#"<head a"><p title=">"#),
        format!(r#"<head a">{s}<p title=">"#)
    );
    assert_eq!(
        injected(r#"<html a"><script>x()</script><head b="1">"#),
        format!(r#"<html a">{s}<script>x()</script><head b="1">"#)
    );
    assert_eq!(
        injected(r#"<html lang = 'e>n' data-x=a>b><head>"#),
        format!(r#"<html lang = 'e>n' data-x=a>{s}b><head>"#)
    );
}

/// A page in UTF-16, by its byte order mark, is served as UTF-8 with the bridge in it.
#[test]
fn a_utf16_page_is_served_as_utf8() {
    let page = "<head><title>Café 日本</title></head>";
    let units: Vec<u16> = page.encode_utf16().collect();
    let le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(units.iter().flat_map(|u| u.to_le_bytes()))
        .collect();
    let be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain(units.iter().flat_map(|u| u.to_be_bytes()))
        .collect();
    for bytes in [le, be] {
        let served = prepare_page(&bytes, NONCE);
        assert_eq!(served.charset.as_deref(), Some("utf-8"));
        let text = String::from_utf8(served.body)
            .unwrap()
            .replace(BRIDGE, "BRIDGE");
        assert_eq!(
            text,
            format!("<head>{}<title>Café 日本</title></head>", script())
        );
    }
    // An unpaired surrogate and an odd last byte are each a replacement character.
    let broken = [0xFF, 0xFE, b'a', 0, 0x00, 0xD8, b'b'];
    let text = String::from_utf8(prepare_page(&broken, NONCE).body).unwrap();
    assert!(text.ends_with("a\u{fffd}\u{fffd}"), "{text:?}");
}

/// A page that isn't UTF-16 keeps its bytes, and the charset it declares rides on the type.
#[test]
fn other_pages_keep_their_bytes_and_declared_charset() {
    let served = prepare_page(
        b"<html><head><meta charset=\"windows-1252\"><title>Caf\xe9",
        NONCE,
    );
    assert_eq!(served.charset.as_deref(), Some("windows-1252"));
    assert!(served.body.ends_with(b"<title>Caf\xe9"));
    assert_eq!(prepare_page(b"<p>Hi</p>", NONCE).charset, None);
}

#[test]
fn a_page_that_is_not_utf8_keeps_its_bytes() {
    let page = b"<html><head><title>Caf\xe9</title>";
    let out = inject_bridge(page, NONCE);
    assert!(out.starts_with(b"<html><head><script nonce="));
    assert!(out.ends_with(b"</script><title>Caf\xe9</title>"));
}

#[test]
fn the_bridge_is_one_script_with_no_imports() {
    assert!(BRIDGE.trim_start().starts_with("//") || BRIDGE.trim_start().starts_with("("));
    assert!(!BRIDGE.contains("import "), "the bridge imports nothing");
    // Nothing in it may end the script element it is inserted in.
    assert!(!BRIDGE.to_ascii_lowercase().contains("</script"));
}

#[test]
fn a_meta_charset_in_the_first_1024_bytes_is_kept() {
    assert_eq!(
        declared_charset(b"<!doctype html><html><head><meta charset=\"UTF-8\"><title>x"),
        Some("utf-8".to_owned())
    );
    assert_eq!(
        declared_charset(
            b"<head><META http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">"
        ),
        Some("windows-1252".to_owned())
    );
    assert_eq!(
        declared_charset(b"<meta charset=shift_jis>"),
        Some("shift_jis".to_owned())
    );
    assert_eq!(declared_charset(b"<html><head><title>x</title>"), None);
    // Past the first 1024 bytes, the browser wouldn't have seen it either.
    let mut late = vec![b' '; 1024];
    late.extend_from_slice(b"<meta charset=\"utf-8\">");
    assert_eq!(declared_charset(&late), None);
    // Only a plain label.
    assert_eq!(declared_charset(b"<meta charset=\"\">"), None);
}

/// The encoding a page declares, found as the browser's prescan finds it.
#[test]
fn the_declared_charset_is_found_as_the_browser_finds_it() {
    // Not in a comment.
    assert_eq!(
        declared_charset(b"<!-- <meta charset=\"shift_jis\"> --><meta charset=\"utf-8\">"),
        Some("utf-8".to_owned())
    );
    assert_eq!(
        declared_charset(b"<!--><meta charset=\"koi8-r\">"),
        Some("koi8-r".to_owned())
    );
    // `content=` counts only with `http-equiv="content-type"`.
    assert_eq!(
        declared_charset(b"<meta content=\"text/html; charset=windows-1252\">"),
        None
    );
    assert_eq!(
        declared_charset(b"<meta http-equiv=\"refresh\" content=\"0; charset=koi8-r\">"),
        None
    );
    assert_eq!(
        declared_charset(
            b"<meta content='text/html; charset=\"iso-8859-2\"' http-equiv=Content-Type>"
        ),
        Some("iso-8859-2".to_owned())
    );
    // Not in another tag's attribute.
    assert_eq!(
        declared_charset(b"<p title='<meta charset=koi8-r>'><meta charset=utf-8>"),
        Some("utf-8".to_owned())
    );
    // A meta declaring UTF-16 means UTF-8.
    for label in ["utf-16", "UTF-16LE", "utf-16be"] {
        assert_eq!(
            declared_charset(format!("<meta charset=\"{label}\">").as_bytes()),
            Some("utf-8".to_owned()),
            "{label}"
        );
    }
}

#[test]
fn full_runs_scripts_from_anywhere_and_reports_what_comes_from_the_web() {
    let policy = page_policy(HtmlPages::Full, NONCE, Some("t1"));
    assert_eq!(
        policy.enforced,
        "sandbox allow-scripts; default-src 'self' https: data: blob:; \
         script-src 'self' https: 'unsafe-inline' 'unsafe-eval' blob:; \
         style-src 'self' https: 'unsafe-inline'; img-src 'self' https: data: blob:; \
         font-src 'self' https: data:; connect-src https:; media-src 'self' https: data: blob:; \
         frame-src https:; object-src 'none'; form-action 'none'; base-uri 'self'"
    );
    assert_eq!(
        policy.report_only.as_deref(),
        Some(
            "default-src 'self' data: blob: 'unsafe-inline' 'unsafe-eval'; \
             report-uri http://lxpage.localhost/__csp-report?t=t1"
        )
    );
    // Without an open to report to (a page can drop its own `?t=`), nothing it loads would be
    // seen: it gets the offline policy instead.
    assert_eq!(
        page_policy(HtmlPages::Full, NONCE, None),
        page_policy(HtmlPages::Offline, NONCE, None)
    );
}

#[test]
fn offline_runs_scripts_with_nothing_from_the_web_and_reports_what_it_blocks() {
    let policy = page_policy(HtmlPages::Offline, NONCE, Some("t1"));
    assert_eq!(
        policy.enforced,
        "sandbox allow-scripts; default-src 'self' data: blob:; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src 'none'; \
         media-src 'self' data: blob:; frame-src 'none'; object-src 'none'; form-action 'none'; \
         base-uri 'self'; report-uri http://lxpage.localhost/__csp-report?t=t1"
    );
    assert_eq!(policy.report_only, None);
    assert!(!page_policy(HtmlPages::Offline, NONCE, None)
        .enforced
        .contains("report-uri"));
}

#[test]
fn static_runs_the_bridge_alone() {
    let policy = page_policy(HtmlPages::Static, NONCE, Some("t1"));
    assert_eq!(
        policy.enforced,
        format!(
            "sandbox allow-scripts; default-src 'self' data: blob:; script-src 'nonce-{NONCE}'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
             connect-src 'none'; media-src 'self' data: blob:; frame-src 'none'; \
             object-src 'none'; form-action 'none'; base-uri 'self'; \
             report-uri http://lxpage.localhost/__csp-report?t=t1"
        )
    );
    assert_eq!(policy.report_only, None);
}

#[test]
fn every_mode_is_sandboxed_and_only_full_reaches_the_web() {
    for mode in [HtmlPages::Full, HtmlPages::Offline, HtmlPages::Static] {
        let policy = page_policy(mode, NONCE, Some("t1"));
        let directives: Vec<&str> = policy.enforced.split("; ").collect();
        assert_eq!(directives[0], "sandbox allow-scripts", "{mode:?}");
        for required in ["object-src 'none'", "form-action 'none'", "base-uri 'self'"] {
            assert!(directives.contains(&required), "{mode:?}: {required}");
        }
        assert!(
            directives.iter().any(|d| d.starts_with("frame-src ")),
            "{mode:?}"
        );
        assert_eq!(
            policy.enforced.contains("https:"),
            mode == HtmlPages::Full,
            "{mode:?}"
        );
        // A nonce in script-src would switch the page's own inline scripts off.
        assert_eq!(
            policy.enforced.contains("'nonce-"),
            mode == HtmlPages::Static,
            "{mode:?}"
        );
    }
}

#[test]
fn a_report_names_the_host_of_what_was_loaded() {
    let report = |uri: &str| {
        format!(
            r#"{{"csp-report":{{"document-uri":"http://lxpage.localhost/d/s/a.html","blocked-uri":"{uri}","violated-directive":"script-src-elem","disposition":"report"}}}}"#
        )
    };
    assert_eq!(
        report_host(report("https://cdn.example.test/lib.js").as_bytes()).as_deref(),
        Some("cdn.example.test")
    );
    assert_eq!(
        report_host(report("https://CDN.Example.test:8443").as_bytes()).as_deref(),
        Some("cdn.example.test")
    );
    // A web socket, which `connect-src https:` lets through.
    for uri in [
        "wss://live.example.test/feed",
        "ws://live.example.test/feed",
    ] {
        assert_eq!(
            report_host(report(uri).as_bytes()).as_deref(),
            Some("live.example.test"),
            "{uri}"
        );
    }
    // What has no host, and Lectern's own protocols, name none.
    for uri in [
        "inline",
        "eval",
        "data",
        "blob",
        "http://lxpage.localhost/d/s/a.js",
        "http://ipc.localhost/x",
    ] {
        assert_eq!(report_host(report(uri).as_bytes()), None, "{uri}");
    }
    for body in [&b"not json"[..], b"{}", br#"{"csp-report":{}}"#, b""] {
        assert_eq!(report_host(body), None);
    }
}

#[test]
fn tokens_are_fresh_hex() {
    let a = fresh_token();
    let b = fresh_token();
    assert_ne!(a, b);
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
}

fn loads(token: &str, hosts: &[&str], blocked: bool) -> PageLoads {
    PageLoads {
        token: token.to_owned(),
        hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
        count: u32::try_from(hosts.len()).unwrap(),
        blocked,
    }
}

#[test]
fn reports_reach_the_window_that_opened_the_page_at_most_every_250_ms() {
    let mut reports = PageReports::default();
    let t = reports.open("main");
    assert!(reports.served(&t, false));
    let now = Instant::now();
    assert_eq!(
        reports.report(&t, "cdn.example.test", now),
        ReportSend::Now("main".to_owned(), loads(&t, &["cdn.example.test"], false))
    );
    // A host already told of is nothing new.
    assert_eq!(
        reports.report(&t, "cdn.example.test", now),
        ReportSend::Nothing
    );
    // A new host soon after waits for the rest of the 250 ms; another meanwhile joins it.
    let soon = now + Duration::from_millis(100);
    assert_eq!(
        reports.report(&t, "fonts.example.test", soon),
        ReportSend::Later(REPORT_THROTTLE - Duration::from_millis(100))
    );
    assert_eq!(
        reports.report(&t, "img.example.test", soon),
        ReportSend::Nothing
    );
    let later = now + REPORT_THROTTLE;
    assert_eq!(
        reports.flush(&t, later),
        Some((
            "main".to_owned(),
            loads(
                &t,
                &["cdn.example.test", "fonts.example.test", "img.example.test"],
                false
            )
        ))
    );
    assert_eq!(reports.flush(&t, later), None);
    // Long after, a new host goes at once.
    assert!(matches!(
        reports.report(&t, "late.example.test", later + REPORT_THROTTLE),
        ReportSend::Now(..)
    ));
}

#[test]
fn a_page_served_offline_reports_what_it_blocked() {
    let mut reports = PageReports::default();
    let t = reports.open("win-2");
    assert!(reports.served(&t, true));
    assert_eq!(
        reports.report(&t, "cdn.example.test", Instant::now()),
        ReportSend::Now("win-2".to_owned(), loads(&t, &["cdn.example.test"], true))
    );
}

#[test]
fn unknown_and_old_opens_are_ignored() {
    let mut reports = PageReports::default();
    assert!(!reports.served("nope", false));
    assert_eq!(
        reports.report("nope", "cdn.example.test", Instant::now()),
        ReportSend::Nothing
    );
    // A window keeps its last four opens, so a page whose open raced another's still reports;
    // older ones go nowhere.
    let first = reports.open("main");
    let other = reports.open("win-1");
    let later: Vec<String> = (0..4).map(|_| reports.open("main")).collect();
    assert!(!later.contains(&first));
    assert!(!reports.served(&first, false));
    for token in &later {
        assert!(reports.served(token, false), "{token}");
    }
    assert!(reports.served(&other, false));
    assert_eq!(
        reports.report(&first, "cdn.example.test", Instant::now()),
        ReportSend::Nothing
    );
    assert_eq!(reports.flush(&first, Instant::now()), None);
}

/// An open names its first 50 hosts, and counts the rest.
#[test]
fn an_open_names_50_hosts_and_counts_the_rest() {
    let mut reports = PageReports::default();
    let t = reports.open("main");
    let mut at = Instant::now();
    let mut last = None;
    for n in 0..60 {
        at += REPORT_THROTTLE;
        if let ReportSend::Now(_, loads) = reports.report(&t, &format!("h{n}.example.test"), at) {
            last = Some(loads);
        }
    }
    let last = last.unwrap();
    assert_eq!(last.hosts.len(), 50);
    assert_eq!(last.hosts[49], "h49.example.test");
    assert_eq!(last.count, 60);
    // A host told of already, named or counted, is nothing new.
    at += REPORT_THROTTLE;
    assert_eq!(
        reports.report(&t, "h3.example.test", at),
        ReportSend::Nothing
    );
    assert_eq!(
        reports.report(&t, "h55.example.test", at),
        ReportSend::Nothing
    );
}

/// A window that closes, or turns to another workspace, takes its opens with it.
#[test]
fn a_windows_opens_go_with_it() {
    let mut reports = PageReports::default();
    let gone = reports.open("win-1");
    let kept = reports.open("main");
    reports.forget("win-1");
    assert!(!reports.served(&gone, false));
    assert!(reports.served(&kept, false));
}
