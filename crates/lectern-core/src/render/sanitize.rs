//! The final ammonia pass: defence in depth on top of the raw-HTML policy and the app's CSP.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use ammonia::{Builder, UrlRelative};

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

static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
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
        .url_schemes(URL_SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::PassThrough)
        .link_rel(None);
    builder
});

/// Strips every tag, attribute and URL scheme not on the allowlists. `<script>` and `<style>` go
/// with their contents.
pub(crate) fn clean(html: &str) -> String {
    SANITIZER.clean(html).to_string()
}
