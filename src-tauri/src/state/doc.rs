//! Reading and rendering a document, and what surrounds it in the UI: breadcrumbs and folder
//! READMEs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lectern_core::ipc::{Crumb, OpenError, OpenErrorKind};
use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::{path_key, LibraryIndex};
use lectern_core::render::{render, RenderContext, RenderedDoc};
use lectern_core::text::{decode, DecodeError};

use super::paths::{is_under, path_string, root_name};

/// The prefix of Lectern's image protocol (`lxasset`) for local images in rendered documents.
pub const ASSET_BASE: &str = "http://lxasset.localhost/";

/// A rendered file and what was known about it when it was read.
pub struct Rendered {
    pub doc: Arc<RenderedDoc>,
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

/// Reads and renders `path` without an index, as boot does.
pub fn render_file(
    path: &Path,
    mapper: &PathMapper,
    trusted_hosts: &[String],
) -> Result<Rendered, OpenError> {
    let stamp = stat_doc(path)?;
    let text = read_text(path)?;
    let doc = render_text(path, &text.text, None, mapper, trusted_hosts);
    Ok(Rendered {
        doc: Arc::new(doc),
        mtime_ms: stamp.mtime_ms,
        lossy: text.lossy,
        covered: false,
        gen: 0,
    })
}

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
        let crumbs = breadcrumbs(doc, Path::new(r"s:\Notes\My Vault\DEV\"), |_| None);
        assert_eq!(names(&crumbs), ["DEV", "HOME.md"]);
    }

    #[test]
    fn crumbs_for_a_doc_outside_the_root_start_at_its_folder() {
        let doc = Path::new(r"C:\Users\me\Downloads\notes.md");
        let crumbs = breadcrumbs(doc, Path::new(r"S:\Notes\My Vault\dev"), |_| None);
        assert_eq!(names(&crumbs), ["Downloads", "notes.md"]);
        assert_eq!(crumbs[0].path, r"C:\Users\me\Downloads");
    }

    #[test]
    fn crumbs_under_a_drive_or_share_root_name_the_root_in_full() {
        let crumbs = breadcrumbs(Path::new(r"C:\notes\a.md"), Path::new(r"C:\"), |_| None);
        assert_eq!(names(&crumbs), [r"C:\", "notes", "a.md"]);
        let crumbs = breadcrumbs(
            Path::new(r"\\nas\Shared\a.md"),
            Path::new(r"\\nas\Shared"),
            |_| None,
        );
        assert_eq!(names(&crumbs), [r"\\nas\Shared", "a.md"]);
    }
}
