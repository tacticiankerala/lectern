//! Links, wikilinks, path-like inline code and images, resolved against the library and written
//! as the markup the reading view follows:
//! - `data-kind`: `doc`, `file`, `path`, `external`, `anchor` or `broken`;
//! - `data-target`: the absolute path a `doc`, `file` or `path` link opens;
//! - `data-anchor`: the heading part of a `doc` link as written (percent-decoded), for an exact
//!   `id` match;
//! - `data-slug`: that heading, or an in-page link's fragment, slugged as heading ids are, for
//!   when no `id` matches exactly;
//! - `data-line`: the line a `doc`, `file` or `path` link cites.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use comrak::html::{self, ChildRendering, Context};
use comrak::nodes::{AstNode, NodeLink, NodeValue};
use percent_encoding::percent_decode_str;
use regex::Regex;

use super::internal::is_internal_url;
use super::slug::slugify;
use super::{Prepared, RenderContext};
use crate::library::pathmap::{
    asset_url, is_absolute, line_fragment, split_line_suffix, unc_host_trusted, LineRef, Mapped,
    PathMapper,
};
use crate::library::resolve::{resolve_relative, resolve_wikilink, WikiResolution};
use crate::library::scan::is_markdown;
use crate::library::LibraryIndex;

/// Inline code that names a file: an optional drive, UNC, `/`, `./` or `../` prefix, a folder
/// separator, a 1–6 character extension and an optional `:line` or `:line:col`. No spaces.
static PATH_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?:[A-Za-z]:[\\/]|\\\\|/|\.{1,2}/)?[^\s<>"|?*]+[\\/][^\s<>"|?*]*\.[A-Za-z0-9]{1,6}(?::\d+(?::\d+)?)?$"#,
    )
    .expect("the path pattern is valid")
});

/// An absolute path, where the prefix's separator stands in for a folder (`/x.md`, `C:\x.md`).
/// Windows paths may hold spaces (`S:\Notes\My Vault\…`); Linux ones may not.
static ABSOLUTE_PATH_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?:/[^\s<>"|?*]+|(?:[A-Za-z]:[\\/]|\\\\)(?:[^\s<>"|?*]| )+)\.[A-Za-z0-9]{1,6}(?::\d+(?::\d+)?)?$"#,
    )
    .expect("the absolute path pattern is valid")
});

/// A bare Markdown file name such as `README.md`, with an optional line suffix.
static MD_FILE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^[^\s<>"|?*/\\:]+\.(?i:md)(?::\d+(?::\d+)?)?$"#)
        .expect("the file name pattern is valid")
});

/// Resolves the links in one document. It rides along in the formatter's user data.
pub(super) struct Links<'c> {
    doc_path: &'c Path,
    index: Option<&'c LibraryIndex>,
    mapper: &'c PathMapper,
    /// Image sources, which also go to the sanitiser for raw `<img>` tags.
    pub(super) images: ImageSources,
    /// Set when a wikilink fails to resolve against an index. Without one, every wikilink shows
    /// as broken but none counts, since the index will settle them.
    pub(super) unresolved_wikilinks: bool,
    /// Raw `<a …>` tags open in the current paragraph, heading or table cell.
    raw_anchors: usize,
}

/// Resolves image sources the way browsers and GitHub do: relative to the note's folder, with
/// absolute paths mapped by the user's prefixes, `/mnt/<x>` and WSL. It owns what it needs, since
/// the sanitiser's attribute filter must be `'static`, so it never consults the library index.
#[derive(Debug, Clone)]
pub(super) struct ImageSources {
    doc_dir: PathBuf,
    mapper: PathMapper,
    asset_base: String,
    trusted_unc_hosts: Vec<String>,
}

/// What a render does with an image source.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ImageSrc {
    /// A local file, loaded through this asset URL.
    Asset(String),
    /// Remote, in-page, another scheme, or an absolute path nothing maps: kept as written, for
    /// the sanitiser and the CSP to judge.
    AsWritten,
    /// A file on a network host the user hasn't chosen: not loaded at all, since reaching the
    /// host would hand it the user's Windows credentials.
    Blocked,
}

/// The tooltip of an image that wasn't loaded because of where it lives.
pub(super) const BLOCKED_IMAGE_TITLE: &str = "Image on an unknown network location was not loaded";

/// A local place a link points at.
enum Target {
    /// A Markdown note, opened in Lectern. `anchor` is the heading part as written.
    Doc {
        path: PathBuf,
        anchor: Option<String>,
        line: Option<u32>,
    },
    /// Any other file the index holds, opened with the shell.
    File { path: PathBuf, line: Option<u32> },
    /// A path that may not exist, checked when it's followed.
    Path { path: PathBuf, line: Option<u32> },
}

/// What a Markdown link's href points at.
enum Href {
    External,
    /// One of the app's own origins: never followed, shown as a broken link.
    Internal,
    /// A heading in this note: the href keeps its fragment, and this is its slug.
    Anchor {
        slug: String,
    },
    Local(Target),
}

impl<'c> Links<'c> {
    pub(super) fn new(ctx: &RenderContext<'c>) -> Self {
        Self {
            doc_path: ctx.doc_path,
            index: ctx.index,
            mapper: ctx.mapper,
            images: ImageSources::new(ctx),
            unresolved_wikilinks: false,
            raw_anchors: 0,
        }
    }

    /// Classifies an href: one to the app's own origins is internal, `http(s)`, `mailto` and
    /// protocol-relative `//host/…` are external, `#x` is in-page, and anything else without a
    /// scheme is a path. `None` for an empty href or another scheme, which keep comrak's markup
    /// for the sanitiser to judge.
    fn classify(&self, url: &str) -> Option<Href> {
        if url.is_empty() {
            return None;
        }
        if is_internal_url(url) {
            return Some(Href::Internal);
        }
        if let Some(fragment) = url.strip_prefix('#') {
            let slug = slugify(&decode(fragment));
            return Some(Href::Anchor { slug });
        }
        if url.starts_with("//") {
            return Some(Href::External);
        }
        if let Some(scheme) = scheme(url) {
            let external = ["http", "https", "mailto"]
                .iter()
                .any(|s| scheme.eq_ignore_ascii_case(s));
            return external.then_some(Href::External);
        }
        let (path, fragment) = split_url(url);
        let LineRef { path, line, .. } = split_line_suffix(&decode(path));
        let (anchor, line) = match fragment.map(decode) {
            Some(fragment) => match line_fragment(&fragment) {
                Some(n) => (None, Some(n)),
                None => (Some(fragment.into_owned()).filter(|f| !f.is_empty()), line),
            },
            None => (None, line),
        };
        Some(Href::Local(self.link_target(&path, anchor, line)))
    }

    /// An explicit link's path. One the index can't confirm still links: a Markdown note opens
    /// (and says when it's missing), anything else is checked when followed.
    fn link_target(&self, path: &str, anchor: Option<String>, line: Option<u32>) -> Target {
        if is_absolute(path) {
            return self.absolute_target(path, anchor, line);
        }
        if let Some(found) = self
            .index
            .and_then(|index| resolve_relative(index, self.doc_path, path))
        {
            return local_target(found, anchor, line);
        }
        let joined = join_lexically(self.doc_path.parent().unwrap_or(Path::new("")), path);
        if is_markdown(&joined.to_string_lossy()) {
            Target::Doc {
                path: joined,
                anchor,
                line,
            }
        } else {
            Target::Path { path: joined, line }
        }
    }

    /// A link to an absolute path. One on a network host the user hasn't chosen is always a
    /// `path` link, which the app checks (and refuses) when followed.
    fn absolute_target(&self, path: &str, anchor: Option<String>, line: Option<u32>) -> Target {
        match self.mapper.map(path, self.index) {
            Mapped::Verified(found) if self.images.trusts(&found) => {
                local_target(found, anchor, line)
            }
            Mapped::Verified(mapped) | Mapped::Unverified(mapped) => {
                Target::Path { path: mapped, line }
            }
            Mapped::Unresolved => Target::Path {
                path: PathBuf::from(path),
                line,
            },
        }
    }

    /// Where a path-like code span points. Relative paths link only when the index holds the
    /// file; absolute ones always link and are checked when followed. Needs the index.
    fn code_target(&self, code: &str) -> Option<Target> {
        let index = self.index?;
        if !is_path_like(code) {
            return None;
        }
        let LineRef { path, line, .. } = split_line_suffix(code);
        if is_absolute(&path) {
            return Some(self.absolute_target(&path, None, line));
        }
        resolve_relative(index, self.doc_path, &path).map(|found| local_target(found, None, line))
    }

    /// Resolves a wikilink, noting a failure when there's an index to fail against.
    fn wikilink(&mut self, target: &str) -> WikiResolution {
        let Some(index) = self.index else {
            return WikiResolution::Broken;
        };
        let resolution = resolve_wikilink(index, self.doc_path, target);
        if resolution == WikiResolution::Broken {
            self.unresolved_wikilinks = true;
        }
        resolution
    }

    /// Notes a raw `<a …>` opening or `</a>` closing, so code inside it isn't linked again.
    fn note_raw_html(&mut self, html: &str) {
        match raw_anchor_tag(html) {
            Some(RawAnchor::Open) => self.raw_anchors += 1,
            Some(RawAnchor::Close) => self.raw_anchors = self.raw_anchors.saturating_sub(1),
            None => {}
        }
    }
}

impl ImageSources {
    fn new(ctx: &RenderContext) -> Self {
        Self {
            doc_dir: ctx
                .doc_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            mapper: ctx.mapper.clone(),
            asset_base: ctx.asset_base.to_owned(),
            trusted_unc_hosts: ctx.trusted_unc_hosts.to_vec(),
        }
    }

    /// Whether `path` is local, or on a network host the user has chosen.
    fn trusts(&self, path: &Path) -> bool {
        unc_host_trusted(&path.to_string_lossy(), &self.trusted_unc_hosts)
    }

    /// Where an image source points: an asset URL for a local file, keeping any `#fragment` (an
    /// SVG sprite's symbol); as written for a remote source, a protocol-relative one, any other
    /// scheme, or an absolute path nothing maps; blocked for a file on an untrusted network host
    /// and for a URL to one of the app's own origins.
    /// Surrounding ASCII whitespace is ignored, as browsers ignore it.
    pub(super) fn resolve(&self, src: &str) -> ImageSrc {
        let src = src.trim_matches(|c: char| c.is_ascii_whitespace());
        // An author-written URL to the app's own image protocol (or IPC) never loads.
        if is_internal_url(src) {
            return ImageSrc::Blocked;
        }
        if src.is_empty() || src.starts_with('#') || src.starts_with("//") || scheme(src).is_some()
        {
            return ImageSrc::AsWritten;
        }
        let (path, fragment) = split_url(src);
        let path = decode(path);
        let local = if is_absolute(&path) {
            match self.mapper.map(&path, None) {
                Mapped::Verified(path) | Mapped::Unverified(path) => path,
                Mapped::Unresolved => return ImageSrc::AsWritten,
            }
        } else {
            join_lexically(&self.doc_dir, &path)
        };
        if !self.trusts(&local) {
            return ImageSrc::Blocked;
        }
        let url = asset_url(&self.asset_base, &local);
        ImageSrc::Asset(match fragment {
            Some(fragment) => format!("{url}#{fragment}"),
            None => url,
        })
    }

    /// A `srcset` with each local candidate's URL passed through `url`, descriptors kept. The
    /// candidates split as the HTML spec splits them: a URL runs to the next whitespace, so it
    /// may hold commas (less any trailing ones, which end the candidate), and its descriptors run
    /// to the next comma. As written when no candidate changes; blocked when any candidate is.
    pub(super) fn srcset(&self, srcset: &str) -> ImageSrc {
        let mut changed = false;
        let mut candidates = Vec::new();
        let mut rest = srcset;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
            if rest.is_empty() {
                break;
            }
            let run_end = rest
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            let (run, after) = rest.split_at(run_end);
            let src = run.trim_end_matches(',');
            let (descriptors, next) = if src.len() < run.len() {
                ("", after)
            } else {
                after.split_once(',').unwrap_or((after, ""))
            };
            rest = next;
            let url = match self.resolve(src) {
                ImageSrc::Asset(url) => {
                    changed = true;
                    url
                }
                ImageSrc::AsWritten => src.to_owned(),
                ImageSrc::Blocked => return ImageSrc::Blocked,
            };
            let descriptors = descriptors.trim_matches(|c: char| c.is_ascii_whitespace());
            candidates.push(match descriptors {
                "" => url,
                descriptors => format!("{url} {descriptors}"),
            });
        }
        if changed {
            ImageSrc::Asset(candidates.join(", "))
        } else {
            ImageSrc::AsWritten
        }
    }
}

/// `rel` joined onto `dir`, with `.` and `..` applied.
fn join_lexically(dir: &Path, rel: &str) -> PathBuf {
    let mut joined = dir.to_path_buf();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                joined.pop();
            }
            part => joined.push(part),
        }
    }
    joined
}

fn local_target(path: PathBuf, anchor: Option<String>, line: Option<u32>) -> Target {
    if is_markdown(&path.to_string_lossy()) {
        Target::Doc { path, anchor, line }
    } else {
        Target::File { path, line }
    }
}

fn is_path_like(code: &str) -> bool {
    PATH_LIKE.is_match(code) || ABSOLUTE_PATH_LIKE.is_match(code) || MD_FILE_NAME.is_match(code)
}

/// The scheme of `url`, such as `https`. A drive letter (`C:\`) is not one, and nor is a name
/// before a line number (`notes.md:12`).
fn scheme(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once(':')?;
    let is_scheme = scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    let is_line = !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b':');
    (is_scheme && !is_line).then_some(scheme)
}

/// The path and fragment of a URL, without its `?query`.
fn split_url(url: &str) -> (&str, Option<&str>) {
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (url, None),
    };
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    (path, fragment)
}

/// `s` with its `%XX` escapes decoded as UTF-8. Escapes that aren't valid (`%zz`) stay as they
/// are, and so does the whole string when the bytes they encode aren't UTF-8.
fn decode(s: &str) -> Cow<'_, str> {
    percent_decode_str(s)
        .decode_utf8()
        .unwrap_or(Cow::Borrowed(s))
}

#[derive(Debug, PartialEq)]
enum RawAnchor {
    Open,
    Close,
}

/// Whether a raw inline tag opens or closes an `<a>`, in any letter case. comrak gives each
/// inline tag a node of its own.
fn raw_anchor_tag(html: &str) -> Option<RawAnchor> {
    let (rest, kind) = match html.strip_prefix("</") {
        Some(rest) => (rest, RawAnchor::Close),
        None => (html.strip_prefix('<')?, RawAnchor::Open),
    };
    let after = rest.strip_prefix(['a', 'A'])?;
    let name_ends = after.is_empty()
        || after.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/');
    // `<a/>` opens nothing.
    let self_closing = kind == RawAnchor::Open && html.trim_end().ends_with("/>");
    (name_ends && !self_closing).then_some(kind)
}

/// Starts a paragraph, heading or table cell: raw anchors don't carry over from the last one.
pub(super) fn start_inlines(context: &mut Context<'_, '_, Prepared<'_>>) {
    context.user.links.raw_anchors = 0;
}

/// A raw inline tag, written as comrak writes it, after noting raw anchors.
pub(super) fn format_raw_inline<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    html: &str,
) -> Result<ChildRendering, fmt::Error> {
    if entering {
        context.user.links.note_raw_html(html);
    }
    html::format_node_default(context, node, entering)
}

/// `<a>` for a Markdown link, carrying where it points. Links the reading view doesn't act on
/// keep comrak's markup.
pub(super) fn format_link<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    link: &NodeLink,
) -> Result<ChildRendering, fmt::Error> {
    let href = match entering {
        true => context.user.links.classify(&link.url),
        false => None,
    };
    let Some(href) = href else {
        return html::format_node_default(context, node, entering);
    };
    context.write_str("<a")?;
    html::render_sourcepos(context, node)?;
    match href {
        Href::External => {
            context.write_str(" href=\"")?;
            context.escape_href(&link.url)?;
            context.write_str("\" data-kind=\"external\"")?;
        }
        Href::Internal => context.write_str(" href=\"#\" data-kind=\"broken\"")?,
        Href::Anchor { slug } => {
            context.write_str(" href=\"")?;
            context.escape_href(&link.url)?;
            context.write_str("\" data-kind=\"anchor\"")?;
            write_slug(context, &slug)?;
        }
        Href::Local(target) => {
            context.write_str(" href=\"#\"")?;
            if matches!(target, Target::Doc { .. }) {
                context.write_str(" class=\"link-doc\"")?;
            }
            write_target(context, &target)?;
        }
    }
    if !link.title.is_empty() {
        write_attribute(context, "title", &link.title)?;
    }
    context.write_str(">")?;
    Ok(ChildRendering::HTML)
}

/// `<a class="wikilink">` for a resolved wikilink, `<a class="wikilink broken">` otherwise.
pub(super) fn format_wikilink<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    target: &str,
) -> Result<ChildRendering, fmt::Error> {
    if !entering {
        context.write_str("</a>")?;
        return Ok(ChildRendering::HTML);
    }
    let resolution = context.user.links.wikilink(target);
    let (name, heading) = match target.split_once('#') {
        Some((name, heading)) => (name, Some(heading)),
        None => (target, None),
    };
    context.write_str("<a")?;
    html::render_sourcepos(context, node)?;
    match resolution {
        WikiResolution::Found { path, .. } => {
            context.write_str(" href=\"#\" class=\"wikilink\"")?;
            let target = Target::Doc {
                path,
                anchor: heading.filter(|h| !h.is_empty()).map(str::to_owned),
                line: None,
            };
            write_target(context, &target)?;
        }
        WikiResolution::Broken => {
            context.write_str(" href=\"#\" class=\"wikilink broken\" data-kind=\"broken\"")?;
            let title = format!("No note named {}", name.trim());
            write_attribute(context, "title", &title)?;
        }
    }
    context.write_str(">")?;
    Ok(ChildRendering::HTML)
}

/// A code span, wrapped in `<a class="code-link">` when it names a file. Code inside a link,
/// Markdown or raw, is left alone, since links can't nest.
pub(super) fn format_code<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    code: &str,
) -> Result<ChildRendering, fmt::Error> {
    let linkable = entering && context.user.links.raw_anchors == 0 && !in_link(node);
    let target = match linkable {
        true => context.user.links.code_target(code),
        false => None,
    };
    let Some(target) = target else {
        return html::format_node_default(context, node, entering);
    };
    context.write_str("<a href=\"#\" class=\"code-link\"")?;
    write_target(context, &target)?;
    context.write_str("><code")?;
    html::render_sourcepos(context, node)?;
    context.write_str(">")?;
    context.escape(code)?;
    context.write_str("</code></a>")?;
    Ok(ChildRendering::HTML)
}

/// `<img>` with its `src` as the note wrote it, like a raw `<img>`: the sanitiser resolves every
/// image source in one place (an asset URL for a local file, nothing at all for a blocked one).
/// The alt text comes from the children, written as plain text between the two halves.
pub(super) fn format_image<'a>(
    context: &mut Context<'_, '_, Prepared<'_>>,
    node: &'a AstNode<'a>,
    entering: bool,
    image: &NodeLink,
) -> Result<ChildRendering, fmt::Error> {
    if entering {
        context.write_str("<img")?;
        html::render_sourcepos(context, node)?;
        context.write_str(" src=\"")?;
        // A drive path (`C:\…`) would read as a URL scheme to the sanitiser, which drops
        // unknown schemes; with its colon encoded it stays a path, which `resolve` decodes.
        match drive_letter(&image.url) {
            Some((drive, rest)) => {
                context.escape_href(drive)?;
                context.write_str("%3A")?;
                context.escape_href(rest)?;
            }
            None => context.escape_href(&image.url)?,
        }
        context.write_str("\" alt=\"")?;
        return Ok(ChildRendering::Plain);
    }
    if !image.title.is_empty() {
        context.write_str("\" title=\"")?;
        context.escape(&image.title)?;
    }
    context.write_str("\" />")?;
    Ok(ChildRendering::HTML)
}

/// The drive letter of a drive path (`C:\…`, `C:/…`) and what follows its colon.
fn drive_letter(url: &str) -> Option<(&str, &str)> {
    let b = url.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
        .then(|| (&url[..1], &url[2..]))
}

fn in_link<'a>(node: &'a AstNode<'a>) -> bool {
    node.ancestors()
        .any(|n| matches!(n.data().value, NodeValue::Link(_) | NodeValue::WikiLink(_)))
}

/// ` data-kind="…" data-target="…"`, plus `data-anchor`, `data-slug` and `data-line` when there
/// are any.
fn write_target<T>(context: &mut Context<'_, '_, T>, target: &Target) -> fmt::Result {
    let (kind, path, anchor, line) = match target {
        Target::Doc { path, anchor, line } => ("doc", path, anchor.as_deref(), line),
        Target::File { path, line } => ("file", path, None, line),
        Target::Path { path, line } => ("path", path, None, line),
    };
    write!(context, " data-kind=\"{kind}\"")?;
    write_attribute(context, "data-target", &path.to_string_lossy())?;
    if let Some(anchor) = anchor {
        write_attribute(context, "data-anchor", anchor)?;
        write_slug(context, &slugify(anchor))?;
    }
    if let Some(line) = line {
        write!(context, " data-line=\"{line}\"")?;
    }
    Ok(())
}

/// ` data-slug="…"`, unless the slug is empty (a heading of emoji alone has no id to aim at).
fn write_slug<T>(context: &mut Context<'_, '_, T>, slug: &str) -> fmt::Result {
    if slug.is_empty() {
        return Ok(());
    }
    write_attribute(context, "data-slug", slug)
}

fn write_attribute<T>(context: &mut Context<'_, '_, T>, name: &str, value: &str) -> fmt::Result {
    write!(context, " {name}=\"")?;
    context.escape(value)?;
    context.write_str("\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn images() -> ImageSources {
        ImageSources {
            doc_dir: PathBuf::from("/vault/notes"),
            mapper: PathMapper::default(),
            asset_base: "asset:".to_owned(),
            trusted_unc_hosts: vec!["nas".to_owned()],
        }
    }

    /// The rewritten value, or `None` when the source stays as written.
    fn rewritten(src: ImageSrc) -> Option<String> {
        match src {
            ImageSrc::Asset(url) => Some(url),
            ImageSrc::AsWritten => None,
            ImageSrc::Blocked => panic!("blocked"),
        }
    }

    #[test]
    fn images_pointing_into_the_app_are_blocked() {
        let images = images();
        for src in [
            "http://lxasset.localhost/%5C%5Cattacker%5Cs%5Cx.png",
            "http://lxasset.localhost/C%3A%5Cvault%5Ca.png",
            "lxasset://localhost/x.png",
            "//ipc.localhost/x",
        ] {
            assert_eq!(images.resolve(src), ImageSrc::Blocked, "{src}");
        }
        assert_eq!(
            images.srcset("a.png 1x, http://lxasset.localhost/x.png 2x"),
            ImageSrc::Blocked
        );
    }

    #[test]
    fn images_on_untrusted_network_hosts_are_blocked() {
        let images = images();
        assert_eq!(images.resolve(r"\\attacker\share\x.png"), ImageSrc::Blocked);
        assert_eq!(
            images.resolve("%5C%5Cattacker%5Cs%5Cx.png"),
            ImageSrc::Blocked
        );
        assert_eq!(
            images.srcset(r"a.png 1x, \\attacker\s\b.png 2x"),
            ImageSrc::Blocked
        );
        assert_eq!(
            images.resolve(r"\\nas\Shared\x.png"),
            ImageSrc::Asset("asset:%5C%5Cnas%5CShared%5Cx.png".to_owned())
        );
        assert_eq!(
            images.resolve(r"C:\pics\a.png"),
            ImageSrc::Asset("asset:C%3A%5Cpics%5Ca.png".to_owned())
        );
    }

    #[test]
    fn image_urls_keep_fragments_and_skip_remote_sources() {
        let images = images();
        assert_eq!(
            rewritten(images.resolve("img/sprite.svg#icon")).as_deref(),
            Some("asset:%2Fvault%2Fnotes%2Fimg%2Fsprite.svg#icon")
        );
        assert_eq!(
            rewritten(images.resolve("../a%20b.png?v=2")).as_deref(),
            Some("asset:%2Fvault%2Fa%20b.png")
        );
        for remote in [
            "https://e.com/a.png",
            "//e.com/a.png",
            "data:image/png;base64,xx",
            "#x",
            "",
        ] {
            assert_eq!(rewritten(images.resolve(remote)), None, "{remote}");
        }
    }

    #[test]
    fn srcset_candidates_are_rewritten_with_their_descriptors() {
        let images = images();
        assert_eq!(
            rewritten(images.srcset("a.png 1x, https://e.com/b.png 2x,c.png")).as_deref(),
            Some("asset:%2Fvault%2Fnotes%2Fa.png 1x, https://e.com/b.png 2x, asset:%2Fvault%2Fnotes%2Fc.png")
        );
        assert_eq!(rewritten(images.srcset("https://e.com/b.png 2x")), None);
    }

    #[test]
    fn srcset_urls_may_hold_commas() {
        let images = images();
        // A comma inside a URL (no whitespace before it) is part of the URL.
        assert_eq!(
            rewritten(images.srcset("https://example.com/a,b.png 1x")),
            None
        );
        assert_eq!(
            rewritten(images.srcset("a,b.png 1x,c.png  2x ,, d.png,")).as_deref(),
            Some(concat!(
                "asset:%2Fvault%2Fnotes%2Fa%2Cb.png 1x, ",
                "asset:%2Fvault%2Fnotes%2Fc.png 2x, ",
                "asset:%2Fvault%2Fnotes%2Fd.png"
            ))
        );
        assert_eq!(
            rewritten(images.srcset("a.png 1x, b.png 2x")).as_deref(),
            Some("asset:%2Fvault%2Fnotes%2Fa.png 1x, asset:%2Fvault%2Fnotes%2Fb.png 2x")
        );
    }

    #[test]
    fn image_urls_ignore_surrounding_whitespace() {
        let images = images();
        assert_eq!(rewritten(images.resolve(" https://e.com/a.png\t")), None);
        assert_eq!(
            rewritten(images.resolve("\n img/a.png ")).as_deref(),
            Some("asset:%2Fvault%2Fnotes%2Fimg%2Fa.png")
        );
    }

    #[test]
    fn raw_anchor_tags() {
        assert_eq!(raw_anchor_tag(r#"<a href="x">"#), Some(RawAnchor::Open));
        assert_eq!(raw_anchor_tag("<A\nHREF='x'>"), Some(RawAnchor::Open));
        assert_eq!(raw_anchor_tag("</a>"), Some(RawAnchor::Close));
        assert_eq!(raw_anchor_tag("</A >"), Some(RawAnchor::Close));
        for other in [
            "<abbr>", "</abbr>", "<a/>", "<b>", "<br>", "<", "</", "<é>", "",
        ] {
            assert_eq!(raw_anchor_tag(other), None, "{other}");
        }
    }

    #[test]
    fn schemes_are_not_drives_or_line_numbers() {
        assert_eq!(scheme("https://e.com"), Some("https"));
        assert_eq!(scheme("mailto:a@b.c"), Some("mailto"));
        assert_eq!(scheme(r"C:\x\y.md"), None);
        assert_eq!(scheme("notes.md:12"), None);
        assert_eq!(scheme("notes.md:12:4"), None);
        assert_eq!(scheme("plans/x.md"), None);
    }

    #[test]
    fn path_like_code() {
        for code in [
            "app/models/user.rb",
            "app/models/user.rb:17",
            "a/b.md:3:9",
            "../README.md",
            "./x/y.json",
            "/home/dev/app/k.rb",
            "/x.md",
            "/x.md:4",
            r"C:\Users\a.txt",
            r"C:\x y.md",
            r"S:\Notes\My Vault\x.md",
            r"\\nas\Shared\x.md",
            "README.md",
            "README.md:12",
        ] {
            assert!(is_path_like(code), "{code}");
        }
        for code in [
            "foo()",
            "a/b",
            "v1.2",
            "x.rb",
            "notes/résumé notes.md",
            "/x y.md",
            "work/alpha/",
            "a | b",
            "#tag",
            "",
        ] {
            assert!(!is_path_like(code), "{code}");
        }
    }
}
