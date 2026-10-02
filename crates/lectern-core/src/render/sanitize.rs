//! The final ammonia pass: defence in depth on top of the raw-HTML policy and the app's CSP. It
//! also points raw `<img>` and `<source>` tags at local files, keeps them from loading anything
//! on an untrusted network host, and makes every image load lazily.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use ammonia::{Builder, UrlRelative};

use super::internal::{is_internal_url, INTERNAL_SCHEMES};
use super::links::{ImageSources, ImageSrc, BLOCKED_IMAGE_TITLE};

/// What a blocked `<img>`'s `src` becomes for the length of the ammonia pass: a relative URL, so
/// it survives the scheme check, swapped afterwards for the blocked-image markup.
const BLOCKED_SRC: &str = "#lectern-blocked-image";
/// The same for a raw `<a href>` into the app's own origins, swapped for a broken link.
const INTERNAL_HREF: &str = "#lectern-internal-link";

/// Every tag comrak and Lectern emit, plus the raw-HTML allowlist.
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "button",
    "code",
    "del",
    "details",
    "div",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "s",
    "section",
    "small",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

const GENERIC_ATTRIBUTES: &[&str] = &["class", "id", "title", "align"];
const GENERIC_ATTRIBUTE_PREFIXES: &[&str] = &["data-", "aria-"];

const TAG_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("a", &["href"]),
    ("button", &["type"]),
    ("details", &["open"]),
    (
        "img",
        &["src", "alt", "width", "height", "loading", "decoding"],
    ),
    ("input", &["type", "checked", "disabled"]),
    // comrak writes `start` for ordered lists that don't begin at 1.
    ("ol", &["start"]),
    // `<picture>` sources. A `srcset` URL is only ever fetched as an image, and the app's CSP
    // limits where images load from.
    ("source", &["srcset", "type", "media"]),
];

/// `javascript:`, `data:` and every other scheme not listed here are dropped. Relative URLs pass.
const URL_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The allowlists, as a fresh builder. Each render builds its own, because the attribute filter
/// that rewrites image sources carries that note's folder and must be `'static`.
fn builder() -> Builder<'static> {
    let mut builder = Builder::default();
    builder
        .tags(TAGS.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRIBUTES.iter().copied().collect())
        .generic_attribute_prefixes(GENERIC_ATTRIBUTE_PREFIXES.iter().copied().collect())
        .tag_attributes(
            TAG_ATTRIBUTES
                .iter()
                .map(|(tag, attrs)| (*tag, attrs.iter().copied().collect::<HashSet<_>>()))
                .collect::<HashMap<_, _>>(),
        )
        // The app's own schemes pass this check only so the attribute filter sees them and
        // blocks them, rather than ammonia dropping them silently: no internal URL survives.
        .url_schemes(
            URL_SCHEMES
                .iter()
                .chain(INTERNAL_SCHEMES)
                .copied()
                .collect(),
        )
        .url_relative(UrlRelative::PassThrough)
        .link_rel(None);
    builder
}

/// Strips every tag, attribute and URL scheme not on the allowlists; `<script>` and `<style>` go
/// with their contents. Local `src` and `srcset` values on `<img>` (Markdown or raw) and
/// `<source>` tags become asset URLs, resolved by `images` from attributes the HTML parser has
/// already tokenized and entity-decoded. One on an untrusted network host, or pointing into the
/// app, is dropped: a `srcset` goes, and an `<img>` keeps only its alt text, marked
/// `img-blocked`. A raw link into the app becomes a broken link.
pub(crate) fn clean(html: &str, images: ImageSources) -> String {
    let html = encode_drive_srcs(html);
    let mut builder = builder();
    builder.attribute_filter(move |element, attribute, value| {
        let resolved = match (element, attribute) {
            ("img", "src") => images.resolve(value),
            ("source", "srcset") => images.srcset(value),
            ("a", "href") if is_internal_url(value) => {
                return Some(Cow::Borrowed(INTERNAL_HREF));
            }
            // Any other URL attribute naming an internal URL goes.
            (_, "href" | "src" | "srcset" | "cite" | "action" | "formaction" | "poster")
                if is_internal_url(value) =>
            {
                return None;
            }
            _ => ImageSrc::AsWritten,
        };
        match resolved {
            ImageSrc::Asset(url) => Some(Cow::Owned(url)),
            ImageSrc::AsWritten => Some(Cow::Borrowed(value)),
            ImageSrc::Blocked if element == "img" => Some(Cow::Borrowed(BLOCKED_SRC)),
            ImageSrc::Blocked => None,
        }
    });
    lazy_images(builder.clean(&html).to_string())
}

/// `html` with the drive colon of each raw `<img src>` percent-encoded (`C:/x.png` becomes
/// `C%3A/x.png`), as `format_image` writes Markdown images. ammonia reads `C:` as a URL scheme and
/// drops the attribute before the filter sees it; encoded, it stays a relative path, which
/// `ImageSources::resolve` decodes and judges like any other, trust checks and all. Only a drive
/// letter, a colon and a separator are touched, so no other scheme gets through.
///
/// comrak escapes every `<` in text, so each one left starts a tag. Tags are read as the HTML
/// tokenizer reads them, quoted values and all, so an `<img` inside another tag's attribute is
/// never taken for an image; like the parser, only an element's first `src` counts.
fn encode_drive_srcs(html: &str) -> Cow<'_, str> {
    let b = html.as_bytes();
    if !b
        .windows(4)
        .any(|w| w[0] == b'<' && w[1..].eq_ignore_ascii_case(b"img"))
    {
        return Cow::Borrowed(html);
    }
    let mut colons = Vec::new();
    let mut at = 0;
    while let Some(offset) = html[at..].find('<') {
        let name_start = at + offset + 1;
        let name_end = name_start
            + b[name_start..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric())
                .count();
        if name_end == name_start {
            at = name_start;
            continue;
        }
        let is_img = html[name_start..name_end].eq_ignore_ascii_case("img");
        let mut seen_src = false;
        at = read_attributes(b, name_end, |name, value_start, value| {
            if !is_img || seen_src || !name.eq_ignore_ascii_case(b"src") {
                return;
            }
            seen_src = true;
            let lead = value.iter().take_while(|c| c.is_ascii_whitespace()).count();
            if let [drive, b':', b'/' | b'\\', ..] = value[lead..] {
                if drive.is_ascii_alphabetic() {
                    colons.push(value_start + lead + 1);
                }
            }
        });
    }
    if colons.is_empty() {
        return Cow::Borrowed(html);
    }
    let mut out = String::with_capacity(html.len() + 2 * colons.len());
    let mut copied = 0;
    for colon in colons {
        out.push_str(&html[copied..colon]);
        out.push_str("%3A");
        copied = colon + 1;
    }
    out.push_str(&html[copied..]);
    Cow::Owned(out)
}

/// Reads a start tag's attributes from `at` (just past its name) to the end of the tag, calling
/// `attribute` with each one's name, where its value starts, and the value (empty without one).
/// Returns where the tag ends: past its `>`, or the end of `b`.
fn read_attributes(
    b: &[u8],
    mut at: usize,
    mut attribute: impl FnMut(&[u8], usize, &[u8]),
) -> usize {
    let skip = |at: &mut usize, skip: &dyn Fn(u8) -> bool| {
        while *at < b.len() && skip(b[*at]) {
            *at += 1;
        }
    };
    loop {
        skip(&mut at, &|c| c.is_ascii_whitespace() || c == b'/');
        if at >= b.len() {
            return at;
        }
        if b[at] == b'>' {
            return at + 1;
        }
        // A name runs to whitespace, `/`, `>` or `=`; a leading `=` is part of it.
        let name_start = at;
        at += 1;
        skip(&mut at, &|c| {
            !(c.is_ascii_whitespace() || matches!(c, b'/' | b'>' | b'='))
        });
        let name = &b[name_start..at];
        skip(&mut at, &|c| c.is_ascii_whitespace());
        if b.get(at) != Some(&b'=') {
            attribute(name, at, &[]);
            continue;
        }
        at += 1;
        skip(&mut at, &|c| c.is_ascii_whitespace());
        let (value_start, value_end) = match b.get(at) {
            Some(&quote @ (b'"' | b'\'')) => {
                let start = at + 1;
                let end = b[start..]
                    .iter()
                    .position(|&c| c == quote)
                    .map_or(b.len(), |i| start + i);
                at = (end + 1).min(b.len());
                (start, end)
            }
            _ => {
                let start = at;
                skip(&mut at, &|c| !(c.is_ascii_whitespace() || c == b'>'));
                (start, at)
            }
        };
        attribute(name, value_start, &b[value_start..value_end]);
    }
}

/// Adds `loading="lazy"` and `decoding="async"` to each `<img>` that doesn't set them, turns a
/// blocked image's marker `src` into `class="img-blocked"` and a tooltip, and a marked link into a
/// broken one. ammonia can add attributes too, but from a hash map, in no fixed order. This reads
/// ammonia's own output, where text never holds a raw `<` and every attribute value is
/// double-quoted with any `"` escaped, so tracking quotes is enough to find whole tags.
fn lazy_images(html: String) -> String {
    if !html.contains("<img") && !html.contains(INTERNAL_HREF) {
        return html;
    }
    let b = html.as_bytes();
    let mut out = String::with_capacity(html.len() + 1024);
    let mut copied = 0;
    let mut from = 0;
    while let Some(offset) = html[from..].find('<') {
        let start = from + offset;
        let mut end = start + 1;
        let mut quoted = false;
        while end < b.len() && (quoted || b[end] != b'>') {
            quoted ^= b[end] == b'"';
            end += 1;
        }
        let tag = &html[start..end];
        let is_img = tag
            .strip_prefix("<img")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '));
        if tag.starts_with("<a ") && end < b.len() {
            if let Cow::Owned(link) = internal_link(tag) {
                out.push_str(&html[copied..start]);
                out.push_str(&link);
                copied = end;
            }
        }
        if is_img && end < b.len() {
            out.push_str(&html[copied..start]);
            out.push_str(&blocked_image(tag));
            copied = end;
            for (name, value) in [("loading", "lazy"), ("decoding", "async")] {
                if !has_attribute(tag, name) {
                    // Writing to a String cannot fail.
                    let _ = write!(out, " {name}=\"{value}\"");
                }
            }
        }
        from = end;
    }
    out.push_str(&html[copied..]);
    out
}

/// `tag` (`<img a="…" …`, without its `>`) as is, or, when its `src` is the blocked marker,
/// without `src`, `class` and `title` and with the blocked-image class and tooltip instead.
fn blocked_image(tag: &str) -> Cow<'_, str> {
    let marker = format!(" src=\"{BLOCKED_SRC}\"");
    if !tag.contains(&marker) {
        return Cow::Borrowed(tag);
    }
    let mut out = String::from("<img");
    let mut rest = tag.split_once(' ').map_or("", |(_, attributes)| attributes);
    while let Some((name, after)) = rest.split_once("=\"") {
        let (value, next) = after.split_once('"').unwrap_or((after, ""));
        let name = name.trim();
        if !matches!(name, "src" | "class" | "title") {
            // Writing to a String cannot fail.
            let _ = write!(out, " {name}=\"{value}\"");
        }
        rest = next;
    }
    let _ = write!(
        out,
        " class=\"img-blocked\" title=\"{BLOCKED_IMAGE_TITLE}\""
    );
    Cow::Owned(out)
}

/// `tag` (`<a …`, without its `>`) as is, or, when its `href` is the internal-link marker, as a
/// broken link: `href="#"` and `data-kind="broken"`, without any other `data-` attribute a note
/// may have forged.
fn internal_link(tag: &str) -> Cow<'_, str> {
    let marker = format!(" href=\"{INTERNAL_HREF}\"");
    if !tag.contains(&marker) {
        return Cow::Borrowed(tag);
    }
    let mut out = String::from("<a");
    let mut rest = tag.split_once(' ').map_or("", |(_, attributes)| attributes);
    while let Some((name, after)) = rest.split_once("=\"") {
        let (value, next) = after.split_once('"').unwrap_or((after, ""));
        let name = name.trim();
        if name != "href" && !name.starts_with("data-") {
            // Writing to a String cannot fail.
            let _ = write!(out, " {name}=\"{value}\"");
        }
        rest = next;
    }
    out.push_str(" href=\"#\" data-kind=\"broken\"");
    Cow::Owned(out)
}

/// Whether a serialized start tag (`<name a="…" b="…"`) has the attribute `wanted`.
fn has_attribute(tag: &str, wanted: &str) -> bool {
    let mut rest = tag.split_once(' ').map_or("", |(_, attributes)| attributes);
    while let Some((name, after)) = rest.split_once("=\"") {
        if name.trim() == wanted {
            return true;
        }
        rest = after.split_once('"').map_or("", |(_, next)| next);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_image_keeps_only_its_alt_text_and_gets_marked() {
        let html = format!(
            r#"<p><img class="c" src="{BLOCKED_SRC}" alt="logo" title="t"> <img src="a.png"></p>"#
        );
        assert_eq!(
            lazy_images(html),
            format!(
                concat!(
                    r#"<p><img alt="logo" class="img-blocked" title="{}" loading="lazy" decoding="async"> "#,
                    r#"<img src="a.png" loading="lazy" decoding="async"></p>"#
                ),
                BLOCKED_IMAGE_TITLE
            )
        );
    }

    #[test]
    fn images_get_lazy_loading_unless_they_set_it() {
        assert_eq!(
            lazy_images(r#"<p><img src="a.png" alt="x"></p><img loading="eager">"#.to_owned()),
            concat!(
                r#"<p><img src="a.png" alt="x" loading="lazy" decoding="async"></p>"#,
                r#"<img loading="eager" decoding="async">"#
            )
        );
        assert_eq!(
            lazy_images("<img>".to_owned()),
            r#"<img loading="lazy" decoding="async">"#
        );
    }

    #[test]
    fn lazy_loading_ignores_attribute_values_and_other_tags() {
        let html = r#"<div title="<img src='a.png'>"><imgx></imgx><img alt="x loading=" src="b.png"></div>"#;
        assert_eq!(
            lazy_images(html.to_owned()),
            r#"<div title="<img src='a.png'>"><imgx></imgx><img alt="x loading=" src="b.png" loading="lazy" decoding="async"></div>"#
        );
        assert_eq!(
            lazy_images("<p>&lt;img&gt;</p>".to_owned()),
            "<p>&lt;img&gt;</p>"
        );
    }

    #[test]
    fn only_drive_colons_in_an_images_first_src_are_encoded() {
        assert_eq!(
            encode_drive_srcs(r#"<p><img alt=">" src=c:\x.png><Img SRC = 'D:/y.png'></p>"#),
            r#"<p><img alt=">" src=c%3A\x.png><Img SRC = 'D%3A/y.png'></p>"#
        );
        for untouched in [
            r#"<img src="javascript:alert(1)">"#,
            r#"<img src="ab:/x.png"><img src="C:x.png"><img src="1:/x.png">"#,
            r#"<img data-src="C:/x.png"><a href="C:/x.png"><imgx src="C:/x.png">"#,
            r#"<img src="x.png" src="C:/x.png">"#,
            r#"<div title="<img src='C:/x.png'>">"#,
            "<p>&lt;img src=\"C:/x.png\"&gt;</p>",
        ] {
            assert!(
                matches!(encode_drive_srcs(untouched), Cow::Borrowed(_)),
                "{untouched}"
            );
        }
    }
}
