//! Reading and rendering a document, and what surrounds it in the UI: breadcrumbs and folder
//! READMEs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lectern_core::ipc::{Crumb, OpenError, OpenErrorKind};
use lectern_core::library::pathmap::{page_url, PathMapper};
use lectern_core::library::{doc_kind, path_key, DocKind, LibraryIndex};
use lectern_core::render::{render, render_plain, RenderContext, RenderedDoc};
use lectern_core::text::{decode, DecodeError};

use super::assets::normalize;
use super::pages::{MAX_BYTES as MAX_PAGE_BYTES, MAX_PDF_BYTES};
use super::paths::{is_under, path_string, root_name};

/// The prefix of Lectern's image protocol (`lxasset`) for local images in rendered documents.
pub const ASSET_BASE: &str = "http://lxasset.localhost/";

/// The largest text file Lectern shows. Notes have no limit.
const MAX_TEXT_BYTES: u64 = 20 * 1024 * 1024;

/// A rendered file and what was known about it when it was read.
pub struct Rendered {
    pub kind: DocKind,
    /// Empty for an HTML page or a PDF, which shows in a frame (`RenderedDoc::framed`).
    pub doc: Arc<RenderedDoc>,
    /// Where an HTML page or a PDF loads into its frame from; `None` for every other kind.
    pub frame_url: Option<String>,
    pub mtime_ms: i64,
    pub lossy: bool,
    /// The index held the file's own root when it was rendered; without that, its wikilinks show
    /// as broken.
    pub covered: bool,
    /// The index generation it was rendered against.
    pub gen: u64,
}

/// The document rendered during boot, before the window exists.
pub struct EarlyDoc {
    pub path: PathBuf,
    /// Given on the command line, so an error is shown rather than the welcome screen.
    pub from_args: bool,
    pub outcome: Result<Rendered, OpenError>,
}

/// What boot found to open.
#[derive(Default)]
pub struct Early {
    pub doc: Option<EarlyDoc>,
    /// A folder given on the command line, to add as a library root.
    pub folder: Option<PathBuf>,
}

pub(super) struct Stamp {
    pub(super) mtime_ms: i64,
    pub(super) size: u64,
}

/// Reads and renders `path` without an index, as boot does. An HTML page or a PDF is only stated.
pub fn render_file(
    path: &Path,
    mapper: &PathMapper,
    trusted_hosts: &[String],
) -> Result<Rendered, OpenError> {
    let kind = opened_kind(path);
    let stamp = stat_doc(path)?;
    check_size(path, kind, stamp.size)?;
    if kind.framed() {
        return framed(path, kind, &stamp);
    }
    let text = read_text(path)?;
    let doc = render_doc(path, kind, &text.text, None, mapper, trusted_hosts);
    Ok(Rendered {
        kind,
        doc: Arc::new(doc),
        frame_url: None,
        mtime_ms: stamp.mtime_ms,
        lossy: text.lossy,
        covered: false,
        gen: 0,
    })
}

/// The kind `path` opens as: its own, or a note's for a file of no kind, which opens as one.
pub(super) fn opened_kind(path: &Path) -> DocKind {
    doc_kind(&path.to_string_lossy()).unwrap_or(DocKind::Markdown)
}

/// An HTML page or a PDF of kind `kind`, stated as `stamp`, opened without reading it: it shows
/// in a frame, from the page protocol's URL for where it is (`.` and `..` applied). A path that
/// has no such URL is an error rather than an empty frame.
pub(super) fn framed(path: &Path, kind: DocKind, stamp: &Stamp) -> Result<Rendered, OpenError> {
    let frame_url = normalize(&path_string(path))
        .and_then(|normal| page_url(Path::new(&normal)))
        .ok_or_else(|| OpenError {
            kind: OpenErrorKind::Io,
            message: format!(
                "Lectern can't show {}: it has no way to serve a file at that path",
                path.display()
            ),
            path: path_string(path),
        })?;
    Ok(Rendered {
        kind,
        doc: Arc::new(RenderedDoc::framed(path)),
        frame_url: Some(frame_url),
        mtime_ms: stamp.mtime_ms,
        lossy: false,
        covered: false,
        gen: 0,
    })
}

/// Renders `text`, read from `path`, as its kind shows: a text file as its paragraphs, as written,
/// and anything else as a note.
pub(super) fn render_doc(
    path: &Path,
    kind: DocKind,
    text: &str,
    index: Option<&LibraryIndex>,
    mapper: &PathMapper,
    trusted_hosts: &[String],
) -> RenderedDoc {
    match kind {
        DocKind::Text => render_plain(text, path),
        _ => render_text(path, text, index, mapper, trusted_hosts),
    }
}

/// Renders `text` as Markdown, as a note at `path`: a note, or a comment on any document.
pub(super) fn render_text(
    path: &Path,
    text: &str,
    index: Option<&LibraryIndex>,
    mapper: &PathMapper,
    trusted_hosts: &[String],
) -> RenderedDoc {
    render(
        text,
        &RenderContext {
            doc_path: path,
            index,
            mapper,
            asset_base: ASSET_BASE,
            trusted_unc_hosts: trusted_hosts,
        },
    )
}

pub(super) fn stat_doc(path: &Path) -> Result<Stamp, OpenError> {
    let meta = fs::metadata(path).map_err(|e| open_error(path, &e))?;
    if meta.is_dir() {
        return Err(OpenError {
            kind: OpenErrorKind::Io,
            message: format!("{} is a folder, not a document", path.display()),
            path: path_string(path),
        });
    }
    Ok(Stamp {
        mtime_ms: meta.modified().map_or(0, unix_ms),
        size: meta.len(),
    })
}

/// Refuses a text file over `MAX_TEXT_BYTES` before it is read, and a page or a PDF the page
/// protocol wouldn't serve (over its cap) before its frame loads. A note opens whatever its size.
pub(super) fn check_size(path: &Path, kind: DocKind, size: u64) -> Result<(), OpenError> {
    let (cap, message) = match kind {
        DocKind::Markdown => return Ok(()),
        DocKind::Text => (
            MAX_TEXT_BYTES,
            "This file is too large to show (over 20 MB).",
        ),
        DocKind::Html => (
            MAX_PAGE_BYTES,
            "This page is too large to show (over 20 MB).",
        ),
        DocKind::Pdf => (
            MAX_PDF_BYTES,
            "This PDF is too large to show (over 100 MB).",
        ),
    };
    if size > cap {
        return Err(OpenError {
            kind: OpenErrorKind::TooLarge,
            message: message.to_owned(),
            path: path_string(path),
        });
    }
    Ok(())
}

pub(super) fn read_text(path: &Path) -> Result<lectern_core::text::Decoded, OpenError> {
    let bytes = fs::read(path).map_err(|e| open_error(path, &e))?;
    decode(&bytes).map_err(|DecodeError::Binary| OpenError {
        kind: OpenErrorKind::Binary,
        message: format!("{} isn't a text file", path.display()),
        path: path_string(path),
    })
}

pub(super) fn open_error(path: &Path, e: &io::Error) -> OpenError {
    let (kind, message) = match e.kind() {
        io::ErrorKind::NotFound => (
            OpenErrorKind::NotFound,
            format!("Couldn't find {}", path.display()),
        ),
        io::ErrorKind::PermissionDenied => (
            OpenErrorKind::Permission,
            format!("Windows denied access to {}", path.display()),
        ),
        _ => (
            OpenErrorKind::Io,
            format!("Couldn't read {}: {e}", path.display()),
        ),
    };
    OpenError {
        kind,
        message,
        path: path_string(path),
    }
}

pub(super) fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub(super) fn now_ms() -> i64 {
    unix_ms(SystemTime::now())
}

/// The breadcrumbs of `doc`: the root, each folder below it, then the file. `readme` gives a
/// folder's README, opened when its crumb is clicked. A doc outside `root` is shown under its own
/// folder.
pub fn breadcrumbs(
    doc: &Path,
    root: &Path,
    readme: impl Fn(&Path) -> Option<String>,
) -> Vec<Crumb> {
    let parent = doc.parent().unwrap_or(doc);
    let root = if is_under(parent, root) { root } else { parent };
    let root_key = path_key(root);
    let mut folders: Vec<&Path> = doc
        .ancestors()
        .skip(1)
        .take_while(|dir| path_key(dir) != root_key)
        .collect();
    folders.reverse();
    let mut crumbs = vec![Crumb {
        name: root_name(root),
        path: path_string(root),
        readme: readme(root),
    }];
    crumbs.extend(folders.into_iter().map(|dir| Crumb {
        name: root_name(dir),
        path: path_string(dir),
        readme: readme(dir),
    }));
    crumbs.push(Crumb {
        name: root_name(doc),
        path: path_string(doc),
        readme: None,
    });
    crumbs
}

/// The README of `dir`, when the index holds one.
pub(super) fn readme_in(index: &LibraryIndex, dir: &Path) -> Option<String> {
    let root = index.root_for(dir)?;
    let root_key = path_key(&root.root);
    let dir_key = path_key(dir);
    let rel = dir_key.strip_prefix(&root_key)?.trim_start_matches('/');
    let readme = if rel.is_empty() {
        "README.md".to_owned()
    } else {
        format!("{rel}/README.md")
    };
    root.get(&readme)
        .map(|file| path_string(&root.abs(&file.rel)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::TempDir;

    fn names(crumbs: &[Crumb]) -> Vec<&str> {
        crumbs.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn crumbs_run_from_the_root_through_each_folder_to_the_file() {
        let root = Path::new(r"S:\Notes\My Vault\dev");
        let doc = Path::new(r"S:\Notes\My Vault\dev\work\my task\plans\2026-01-01-plan.md");
        let crumbs = breadcrumbs(doc, root, |dir| {
            (dir == Path::new(r"S:\Notes\My Vault\dev\work\my task"))
                .then(|| r"S:\Notes\My Vault\dev\work\my task\README.md".to_owned())
        });
        assert_eq!(
            names(&crumbs),
            ["dev", "work", "my task", "plans", "2026-01-01-plan.md"]
        );
        let paths: Vec<&str> = crumbs.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                r"S:\Notes\My Vault\dev",
                r"S:\Notes\My Vault\dev\work",
                r"S:\Notes\My Vault\dev\work\my task",
                r"S:\Notes\My Vault\dev\work\my task\plans",
                r"S:\Notes\My Vault\dev\work\my task\plans\2026-01-01-plan.md",
            ]
        );
        let readmes: Vec<Option<&str>> = crumbs.iter().map(|c| c.readme.as_deref()).collect();
        assert_eq!(
            readmes,
            [
                None,
                None,
                Some(r"S:\Notes\My Vault\dev\work\my task\README.md"),
                None,
                None
            ]
        );
    }

    #[test]
    fn crumbs_match_the_root_whatever_its_case_or_trailing_separator() {
        let doc = Path::new(r"S:\Notes\My Vault\dev\HOME.md");
        let crumbs = breadcrumbs(doc, Path::new(r"s:\notes\my vault\DEV\"), |_| None);
        assert_eq!(names(&crumbs), ["DEV", "HOME.md"]);
    }

    #[test]
    fn crumbs_for_a_doc_outside_the_root_start_at_its_folder() {
        let doc = Path::new(r"C:\Users\me\Downloads\notes.md");
        let crumbs = breadcrumbs(doc, Path::new(r"S:\Notes\My Vault\dev"), |_| None);
        assert_eq!(names(&crumbs), ["Downloads", "notes.md"]);
        assert_eq!(crumbs[0].path, r"C:\Users\me\Downloads");
    }

    /// An HTML page or a PDF shows in a frame, so opening one only stats it: these hold bytes no
    /// text decoder takes, and still open.
    #[test]
    fn an_html_page_or_a_pdf_is_never_read() {
        let dir = TempDir::new();
        for (name, kind) in [("page.html", DocKind::Html), ("brief.PDF", DocKind::Pdf)] {
            let path = dir.file(name, "\0\u{1}\u{2}");
            let rendered = render_file(&path, &PathMapper::default(), &[])
                .unwrap_or_else(|e| panic!("{name}: {}", e.message));
            assert_eq!(rendered.kind, kind, "{name}");
            assert_eq!(rendered.doc.html, "", "{name}");
            assert_eq!(rendered.doc.title, name);
            assert!(rendered.mtime_ms > 0, "{name}");
            assert!(!rendered.lossy, "{name}");
        }
    }

    /// A note renders as Markdown; a text file as written, in paragraphs, as boot shows them.
    #[test]
    fn notes_and_text_files_are_read() {
        let dir = TempDir::new();
        let note = dir.file("plan.md", "# Plan\n");
        let text = dir.file("readme.txt", "# Plain *words*.\n");
        let rendered = render_file(&note, &PathMapper::default(), &[]).unwrap();
        assert_eq!(rendered.kind, DocKind::Markdown);
        assert!(rendered.doc.html.contains("Plan"));
        let rendered = render_file(&text, &PathMapper::default(), &[]).unwrap();
        assert_eq!(rendered.kind, DocKind::Text);
        assert_eq!(
            rendered.doc.html,
            "<p class=\"lx-plain\" data-sourcepos=\"1:1-1:16\"># Plain *words*.</p>\n"
        );
        assert!(rendered.doc.outline.is_empty());
        assert_eq!(rendered.doc.title, "readme.txt");
        // Any other file opens as a note, as it always has.
        let other = dir.file("notes.log", "# Log\n");
        let rendered = render_file(&other, &PathMapper::default(), &[]).unwrap();
        assert_eq!(rendered.kind, DocKind::Markdown);
    }

    /// Only text files have a size limit: a note opens whatever its size, as it always has.
    #[test]
    fn a_text_file_over_20_mib_is_too_large_to_show() {
        const LIMIT: u64 = 20 * 1024 * 1024;
        let path = Path::new(r"S:\Notes\My Vault\huge.txt");
        assert!(check_size(path, DocKind::Text, LIMIT).is_ok());
        let error = check_size(path, DocKind::Text, LIMIT + 1).unwrap_err();
        assert!(matches!(error.kind, OpenErrorKind::TooLarge));
        assert_eq!(
            error.message,
            "This file is too large to show (over 20 MB)."
        );
        assert_eq!(error.path, path_string(path));
        // A note opens whatever its size; a page or a PDF has the page protocol's caps (open.rs).
        assert!(check_size(path, DocKind::Markdown, 64 * LIMIT).is_ok());
        // Refused before it is read.
        let dir = TempDir::new();
        let huge = dir.0.join("huge.txt");
        fs::File::create(&huge).unwrap().set_len(LIMIT + 1).unwrap();
        let Err(error) = render_file(&huge, &PathMapper::default(), &[]) else {
            panic!("a text file over the limit opened");
        };
        assert!(matches!(error.kind, OpenErrorKind::TooLarge));
    }

    #[test]
    fn crumbs_under_a_drive_or_share_root_name_the_root_in_full() {
        let crumbs = breadcrumbs(Path::new(r"C:\notes\a.md"), Path::new(r"C:\"), |_| None);
        assert_eq!(names(&crumbs), [r"C:\", "notes", "a.md"]);
        let crumbs = breadcrumbs(
            Path::new(r"\\nas\share\a.md"),
            Path::new(r"\\nas\share"),
            |_| None,
        );
        assert_eq!(names(&crumbs), [r"\\nas\share", "a.md"]);
    }
}
