//! HTML pages as Lectern serves them into their frame (`lxpage`): Lectern's bridge script inserted
//! at the top, the content security policy each *HTML pages* setting gives, the encoding a page
//! declares (kept in the response's type, as the bridge pushes the page's own declaration down; a
//! UTF-16 page is served as UTF-8), and the policy reports through which the browser names the
//! hosts a page loaded from.
//!
//! Where the bridge goes, and which encoding a page declares, are read as the browser's tokenizer
//! and prescan read them, so no page can put a script of its own ahead of the bridge, or the bridge
//! inside an attribute value or a comment.
//!
//! A page always runs sandboxed, with an opaque origin: its policy starts with
//! `sandbox allow-scripts`, whatever frame it lands in. The bridge runs in every mode: `static`
//! allows it alone, by the nonce of its script element.
//!
//! The reports come from the browser, not the page, so a page can't hide what it loaded: under
//! `full` a report-only policy reports every load from the web, and under `offline` and `static`
//! the enforced policy reports every load it blocked. Each report names the open it is for by a
//! token minted when the window opened the page (`PageReports`).

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use url::Url;

use crate::ipc::{HtmlPages, PageLoads};
use crate::library::pathmap::PAGE_ORIGIN;

/// Lectern's bridge: one script, inserted at the top of every page, that reports the page's text,
/// headings, selection, scroll position and clicked links to Lectern, and paints its highlights.
pub const BRIDGE: &str = include_str!("../assets/page-bridge.js");

/// Where the policy reports of a page go, on the page protocol: `?t=<token>` names the open.
pub const REPORT_PATH: &str = "/__csp-report";

/// A window hears about a page's loads at most this often.
pub const REPORT_THROTTLE: Duration = Duration::from_millis(250);

/// The opens a window keeps reports for: its last few, so a page whose open raced another's (two
/// pages opened at once, the later first) still reports.
const OPENS_KEPT: usize = 4;

/// How far into a page the browser looks for the encoding it declares.
const CHARSET_PRESCAN: usize = 1024;

/// The hosts an open names; past these it only counts them.
const HOSTS_NAMED: usize = 50;

/// The hosts an open counts beyond those it names; past these the count stops.
const HOSTS_COUNTED: usize = 10_000;

/// What every page's policy ends with, in every mode.
const ALWAYS: &str = "object-src 'none'; form-action 'none'; base-uri 'self'";

/// The policies a page is served with: the one enforced, and the one only reported (`full` has
/// one, while a window waits for its reports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagePolicy {
    pub enforced: String,
    pub report_only: Option<String>,
}

/// The policies for a page served under `mode`, whose bridge script carries `nonce`, and whose
/// reports go to the open `report` names (a token), when there is one. Without one, nothing the
/// page loaded could be told (a page can drop its own `?t=`), so `full` gives the offline policy.
pub fn page_policy(mode: HtmlPages, nonce: &str, report: Option<&str>) -> PagePolicy {
    let mode = if mode == HtmlPages::Full && report.is_none() {
        HtmlPages::Offline
    } else {
        mode
    };
    let report_uri = report.map(|token| format!("report-uri {PAGE_ORIGIN}{REPORT_PATH}?t={token}"));
    match mode {
        HtmlPages::Full => PagePolicy {
            enforced: format!(
                "sandbox allow-scripts; default-src 'self' https: data: blob:; \
                 script-src 'self' https: 'unsafe-inline' 'unsafe-eval' blob:; \
                 style-src 'self' https: 'unsafe-inline'; img-src 'self' https: data: blob:; \
                 font-src 'self' https: data:; connect-src https:; \
                 media-src 'self' https: data: blob:; frame-src https:; {ALWAYS}"
            ),
            report_only: report_uri.map(|uri| {
                format!("default-src 'self' data: blob: 'unsafe-inline' 'unsafe-eval'; {uri}")
            }),
        },
        HtmlPages::Offline | HtmlPages::Static => {
            // With a nonce, `'unsafe-inline'` would be ignored anyway: the bridge runs alone.
            let scripts = if mode == HtmlPages::Static {
                format!("'nonce-{nonce}'")
            } else {
                "'self' 'unsafe-inline' 'unsafe-eval' blob:".to_owned()
            };
            let mut enforced = format!(
                "sandbox allow-scripts; default-src 'self' data: blob:; script-src {scripts}; \
                 style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
                 font-src 'self' data:; connect-src 'none'; media-src 'self' data: blob:; \
                 frame-src 'none'; {ALWAYS}"
            );
            if let Some(uri) = report_uri {
                enforced.push_str("; ");
                enforced.push_str(&uri);
            }
            PagePolicy {
                enforced,
                report_only: None,
            }
        }
    }
}

/// `html` with the bridge inserted as `<script nonce="…">` (`inject_script`). The page's bytes are
/// kept as they are, whatever their encoding.
pub fn inject_bridge(html: &[u8], nonce: &str) -> Vec<u8> {
    inject_script(
        html,
        &format!("<script nonce=\"{nonce}\">{BRIDGE}</script>"),
    )
}

/// `html` with `script` inserted right after the `<head>` start tag, else after the `<html>` start
/// tag, else after a leading doctype, else at the start (after a byte order mark), so it runs
/// before any script of the page's. Only the page's prologue is looked at (comments, the doctype,
/// `<html>`, then `<head>`), read as the browser's tokenizer reads it: a `<head>` anywhere later is
/// text or script, never the head.
pub fn inject_script(html: &[u8], script: &str) -> Vec<u8> {
    let at = insertion_point(html);
    let mut out = Vec::with_capacity(html.len() + script.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(script.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

/// Where the bridge goes in `html` (see `inject_script`).
fn insertion_point(html: &[u8]) -> usize {
    let start = if html.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    let mut doctype = None;
    let mut html_tag = None;
    let mut at = start;
    loop {
        while html.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let rest = &html[at..];
        if rest.starts_with(b"<!--") {
            at = comment_end(html, at);
        } else if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            // A doctype, or a bogus comment: either ends at the next `>`.
            let end = find(html, at, b">").map_or(html.len(), |end| end + 1);
            if starts_with_ignore_case(&rest[2..], b"doctype") && html_tag.is_none() {
                doctype = Some(end);
            }
            at = end;
        } else if let Some(end) = start_tag(html, at, b"html").filter(|_| html_tag.is_none()) {
            html_tag = Some(end);
            at = end;
        } else if let Some(end) = start_tag(html, at, b"head") {
            return end;
        } else {
            // Anything else (an element, text, the end) comes after where a head could start.
            break;
        }
    }
    html_tag.or(doctype).unwrap_or(start)
}

/// The end of the comment starting at `at` (`<!--`): `<!-->` and `<!--->` are whole comments, and
/// any other ends at `-->` or `--!>`; an unclosed one runs to the end.
fn comment_end(html: &[u8], at: usize) -> usize {
    let body = at + 4;
    let rest = &html[body..];
    if rest.starts_with(b">") {
        return body + 1;
    }
    if rest.starts_with(b"->") {
        return body + 2;
    }
    let closed = find(html, body, b"-->").map(|i| i + 3);
    let banged = find(html, body, b"--!>").map(|i| i + 4);
    match (closed, banged) {
        (Some(a), Some(b)) => a.min(b),
        (Some(end), None) | (None, Some(end)) => end,
        (None, None) => html.len(),
    }
}

/// Where a start tag's tokenizer is, past its name.
#[derive(Clone, Copy)]
enum Attr {
    BeforeName,
    Name,
    AfterName,
    BeforeValue,
    Quoted(u8),
    AfterQuoted,
    Unquoted,
}

/// The end (just past its `>`) of the start tag of element `name` at `at`, in any letter case, read
/// as the browser's tokenizer reads it: a quote opens a value only right after `=` (and space), and
/// a `>` inside a quoted value doesn't end the tag. `None` when no such start tag is there, or it
/// never ends.
fn start_tag(html: &[u8], at: usize, name: &[u8]) -> Option<usize> {
    let rest = html.get(at..)?.strip_prefix(b"<")?;
    if !starts_with_ignore_case(rest, name) {
        return None;
    }
    match rest.get(name.len()) {
        Some(b'>' | b'/') => {}
        Some(c) if c.is_ascii_whitespace() => {}
        _ => return None,
    }
    let mut state = Attr::BeforeName;
    for (i, &c) in html.iter().enumerate().skip(at + 1 + name.len()) {
        let space = c.is_ascii_whitespace();
        state = match (state, c) {
            (Attr::Quoted(q), _) if c == q => Attr::AfterQuoted,
            (Attr::Quoted(q), _) => Attr::Quoted(q),
            (_, b'>') => return Some(i + 1),
            (Attr::BeforeName | Attr::AfterQuoted, b'/') => Attr::BeforeName,
            (Attr::BeforeName | Attr::AfterQuoted | Attr::Unquoted, _) if space => Attr::BeforeName,
            (Attr::Unquoted, _) => Attr::Unquoted,
            // `=` with no name before it is the start of a name.
            (Attr::BeforeName | Attr::AfterQuoted, _) => Attr::Name,
            (Attr::Name | Attr::AfterName, b'=') => Attr::BeforeValue,
            (Attr::Name | Attr::AfterName, b'/') => Attr::BeforeName,
            (Attr::Name | Attr::AfterName, _) if space => Attr::AfterName,
            (Attr::Name | Attr::AfterName, _) => Attr::Name,
            (Attr::BeforeValue, _) if space => Attr::BeforeValue,
            (Attr::BeforeValue, b'"' | b'\'') => Attr::Quoted(c),
            (Attr::BeforeValue, _) => Attr::Unquoted,
        };
    }
    None
}

fn starts_with_ignore_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

/// A page as served: its bytes with the bridge in, and the charset its type carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPage {
    pub body: Vec<u8>,
    pub charset: Option<String>,
}

/// `html` as served, with the bridge in (`inject_bridge`). A page in UTF-16 (by its byte order
/// mark, which the browser goes by first) is decoded, an unpaired surrogate or an odd last byte
/// becoming U+FFFD, and served as UTF-8: the bridge is UTF-8. Any other keeps its bytes, and the
/// charset it declares (`declared_charset`).
pub fn prepare_page(html: &[u8], nonce: &str) -> PreparedPage {
    let little = html.starts_with(&[0xFF, 0xFE]);
    if !little && !html.starts_with(&[0xFE, 0xFF]) {
        return PreparedPage {
            body: inject_bridge(html, nonce),
            charset: declared_charset(html),
        };
    }
    let (pairs, odd) = html[2..].as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|&pair| {
            if little {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        })
        .collect();
    let mut text = String::from_utf16_lossy(&units);
    if !odd.is_empty() {
        text.push('\u{FFFD}');
    }
    PreparedPage {
        body: inject_bridge(text.as_bytes(), nonce),
        charset: Some("utf-8".to_owned()),
    }
}

/// The encoding `html` declares in a `<meta>` within its first 1024 bytes, found as the browser's
/// prescan finds it: outside comments and other tags, from `charset=`, or from the `content=` of a
/// `http-equiv="content-type"`; lower-cased. UTF-16 labels mean UTF-8 there, as in the browser.
/// Served as the response's charset, as the bridge, inserted ahead of it, can push it past where
/// the browser looks.
pub fn declared_charset(html: &[u8]) -> Option<String> {
    let bytes = &html[..html.len().min(CHARSET_PRESCAN)];
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"<!--") {
            // To the `>` of the first `-->`, whose dashes may be the opening's own.
            at = find(bytes, at + 2, b"-->").map_or(bytes.len(), |i| i + 2);
        } else if starts_with_ignore_case(rest, b"<meta")
            && rest.get(5).is_some_and(|&c| c == b'/' || is_space(c))
        {
            at += 6;
            if let Some(charset) = meta_charset(bytes, &mut at) {
                return Some(charset);
            }
            continue;
        } else if rest.first() == Some(&b'<')
            && (rest.get(1).is_some_and(u8::is_ascii_alphabetic)
                || (rest.get(1) == Some(&b'/') && rest.get(2).is_some_and(u8::is_ascii_alphabetic)))
        {
            // Another tag: past its name, then its attributes.
            while at < bytes.len() && !is_space(bytes[at]) && bytes[at] != b'>' {
                at += 1;
            }
            while attribute(bytes, &mut at).is_some() {}
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") || rest.starts_with(b"<?") {
            at = find(bytes, at, b">").unwrap_or(bytes.len());
        }
        at += 1;
    }
    None
}

/// The prescan's whitespace: tab, line feed, form feed, carriage return, space.
fn is_space(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

/// The encoding a `<meta>` declares, its attributes read from `at` (just past `<meta `), as the
/// prescan reads them: each attribute once, the first of each name counting.
fn meta_charset(bytes: &[u8], at: &mut usize) -> Option<String> {
    let mut seen = HashSet::new();
    let mut pragma = false;
    // Whether the charset came from `content=` (and needs the pragma), or from `charset=`.
    let mut needs_pragma = None;
    let mut charset = None;
    while let Some((name, value)) = attribute(bytes, at) {
        if !seen.insert(name.clone()) {
            continue;
        }
        match name.as_slice() {
            b"http-equiv" if value == b"content-type" => pragma = true,
            b"content" if charset.is_none() => {
                if let Some(found) = content_charset(&value) {
                    charset = Some(found);
                    needs_pragma = Some(true);
                }
            }
            b"charset" => {
                charset = Some(value);
                needs_pragma = Some(false);
            }
            _ => {}
        }
    }
    match needs_pragma {
        Some(true) if !pragma => return None,
        None => return None,
        _ => {}
    }
    let label = String::from_utf8(charset?).ok()?;
    let plain = !label.is_empty()
        && label
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':' | b'-'));
    if !plain {
        return None;
    }
    Some(if is_utf16(&label) {
        "utf-8".to_owned()
    } else {
        label
    })
}

/// Whether `label` names UTF-16 (`utf-16le` or `utf-16be`, by any of their labels).
fn is_utf16(label: &str) -> bool {
    label.starts_with("utf-16")
        || matches!(
            label,
            "unicode"
                | "unicodefeff"
                | "unicodefffe"
                | "ucs-2"
                | "csunicode"
                | "iso-10646-ucs-2"
                | "unicode-1-1"
        )
}

/// The next attribute of a tag, read from `at` as the prescan reads one ("get an attribute"): its
/// name and value, lower-cased. `None` at the tag's end, or the bytes'.
fn attribute(bytes: &[u8], at: &mut usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let byte = |i: usize| bytes.get(i).copied();
    while byte(*at).is_some_and(|c| is_space(c) || c == b'/') {
        *at += 1;
    }
    if byte(*at)? == b'>' {
        return None;
    }
    let mut name = Vec::new();
    loop {
        let c = byte(*at)?;
        if c == b'=' && !name.is_empty() {
            *at += 1;
            break;
        }
        if is_space(c) {
            while byte(*at).is_some_and(is_space) {
                *at += 1;
            }
            if byte(*at)? != b'=' {
                return Some((name, Vec::new()));
            }
            *at += 1;
            break;
        }
        if c == b'/' || c == b'>' {
            return Some((name, Vec::new()));
        }
        name.push(c.to_ascii_lowercase());
        *at += 1;
    }
    while byte(*at).is_some_and(is_space) {
        *at += 1;
    }
    let mut value = Vec::new();
    match byte(*at)? {
        q @ (b'"' | b'\'') => loop {
            *at += 1;
            let c = byte(*at)?;
            if c == q {
                *at += 1;
                return Some((name, value));
            }
            value.push(c.to_ascii_lowercase());
        },
        b'>' => Some((name, value)),
        _ => loop {
            let c = byte(*at)?;
            if is_space(c) || c == b'>' {
                return Some((name, value));
            }
            value.push(c.to_ascii_lowercase());
            *at += 1;
        },
    }
}

/// The charset in a meta's `content` value (`text/html; charset=…`), as the browser finds it.
fn content_charset(content: &[u8]) -> Option<Vec<u8>> {
    let mut at = 0;
    loop {
        at = find(content, at, b"charset")? + 7;
        while content.get(at).copied().is_some_and(is_space) {
            at += 1;
        }
        if content.get(at) == Some(&b'=') {
            at += 1;
            break;
        }
    }
    while content.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    match content.get(at)? {
        q @ (b'"' | b'\'') => {
            let end = content[at + 1..].iter().position(|c| c == q)?;
            Some(content[at + 1..at + 1 + end].to_vec())
        }
        _ => {
            let end = content[at..]
                .iter()
                .position(|&c| is_space(c) || c == b';')
                .map_or(content.len(), |i| at + i);
            (end > at).then(|| content[at..end].to_vec())
        }
    }
}

/// The host a policy report (`application/csp-report`) says was loaded or blocked: that of its
/// `blocked-uri`, when that is a web or web-socket URL. Inline code, `data:` and `blob:` have none, and
/// Lectern's own protocols (`*.localhost`) are no load from the web.
pub fn report_host(body: &[u8]) -> Option<String> {
    let report: serde_json::Value = serde_json::from_slice(body).ok()?;
    let uri = report.get("csp-report")?.get("blocked-uri")?.as_str()?;
    let url = Url::parse(uri).ok()?;
    // Web sockets too: `connect-src https:` lets `wss:` through.
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    (host != "localhost" && !host.ends_with(".localhost")).then_some(host)
}

/// A fresh random token, 32 hex digits: a script nonce, or the name of a page's open. Keyed by
/// the process's random hash keys, so no page can guess one.
pub fn fresh_token() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let keys = RandomState::new();
    let high = keys.hash_one((n, nanos, 0u8));
    let low = keys.hash_one((n, nanos, 1u8));
    format!("{high:016x}{low:016x}")
}

/// What to do after a report (`PageReports::report`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportSend {
    /// Nothing new, or the open is unknown.
    Nothing,
    /// Tell this window now.
    Now(String, PageLoads),
    /// Tell the window this much later (`PageReports::flush`): it heard about the page lately.
    Later(Duration),
}

/// An open page's reports.
#[derive(Debug)]
struct Open {
    window: String,
    /// Numbers the window's opens, oldest first.
    seq: u64,
    /// The first hosts reported, at most `HOSTS_NAMED`.
    hosts: Vec<String>,
    /// The hosts reported past those, by their hash: counted, not named.
    beyond: HashSet<u64>,
    blocked: bool,
    /// When its window last heard about it.
    told: Option<Instant>,
    /// A later telling is due (`ReportSend::Later`).
    due: bool,
}

/// The pages the windows have open, by token, and what each has loaded from the web. A window
/// keeps its last few opens (`OPENS_KEPT`); older ones, and those of a window that has closed or
/// turned to another workspace (`forget`), are forgotten.
#[derive(Debug, Default)]
pub struct PageReports {
    opens: HashMap<String, Open>,
    seq: u64,
    /// Hashes the hosts an open counts without naming.
    keys: RandomState,
}

impl PageReports {
    /// A token for a page the window `window` is opening, for its frame URL and its reports.
    pub fn open(&mut self, window: &str) -> String {
        let mut kept: Vec<u64> = self
            .opens
            .values()
            .filter(|open| open.window == window)
            .map(|open| open.seq)
            .collect();
        if kept.len() >= OPENS_KEPT {
            kept.sort_unstable();
            let oldest = kept[kept.len() - OPENS_KEPT];
            self.opens
                .retain(|_, open| open.window != window || open.seq > oldest);
        }
        self.seq += 1;
        let token = fresh_token();
        self.opens.insert(
            token.clone(),
            Open {
                window: window.to_owned(),
                seq: self.seq,
                hosts: Vec::new(),
                beyond: HashSet::new(),
                blocked: false,
                told: None,
                due: false,
            },
        );
        token
    }

    /// Forgets the opens of the window `window`, which has closed or turned to another workspace:
    /// their reports go nowhere from now on.
    pub fn forget(&mut self, window: &str) {
        self.opens.retain(|_, open| open.window != window);
    }

    /// The page of the open `token` is being served, its loads `blocked` (`offline`, `static`)
    /// or not; false when the open is unknown, and then the page reports nowhere.
    pub fn served(&mut self, token: &str, blocked: bool) -> bool {
        match self.opens.get_mut(token) {
            Some(open) => {
                open.blocked = blocked;
                true
            }
            None => false,
        }
    }

    /// A report for the open `token` named `host`, at `now`: a new host is told to the window at
    /// once, or, when it heard less than `REPORT_THROTTLE` ago, once that has passed. Past the
    /// first `HOSTS_NAMED` hosts, a new one is only counted, up to `HOSTS_COUNTED` more.
    pub fn report(&mut self, token: &str, host: &str, now: Instant) -> ReportSend {
        let hash = self.keys.hash_one(host);
        let Some(open) = self.opens.get_mut(token) else {
            return ReportSend::Nothing;
        };
        if open.hosts.iter().any(|known| known == host) {
            return ReportSend::Nothing;
        }
        if open.hosts.len() < HOSTS_NAMED {
            open.hosts.push(host.to_owned());
        } else if open.beyond.len() >= HOSTS_COUNTED || !open.beyond.insert(hash) {
            return ReportSend::Nothing;
        }
        if open.due {
            return ReportSend::Nothing;
        }
        match open.told.map(|told| now.saturating_duration_since(told)) {
            Some(since) if since < REPORT_THROTTLE => {
                open.due = true;
                ReportSend::Later(REPORT_THROTTLE - since)
            }
            _ => {
                open.told = Some(now);
                ReportSend::Now(open.window.clone(), loads(token, open))
            }
        }
    }

    /// The telling `ReportSend::Later` asked for, at `now`: the window and what to tell it, unless
    /// the open is gone or nothing is due.
    pub fn flush(&mut self, token: &str, now: Instant) -> Option<(String, PageLoads)> {
        let open = self.opens.get_mut(token).filter(|open| open.due)?;
        open.due = false;
        open.told = Some(now);
        Some((open.window.clone(), loads(token, open)))
    }
}

fn loads(token: &str, open: &Open) -> PageLoads {
    PageLoads {
        token: token.to_owned(),
        hosts: open.hosts.clone(),
        count: u32::try_from(open.hosts.len() + open.beyond.len()).unwrap_or(u32::MAX),
        blocked: open.blocked,
    }
}
