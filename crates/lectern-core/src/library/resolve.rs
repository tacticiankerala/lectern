//! Resolving wikilinks, relative paths and path suffixes against the library index.

use std::path::{Path, PathBuf};

use super::tree::natural_cmp;
use super::{key_under, path_key, FileEntry, LibraryIndex, RootIndex};
use crate::render::slug::slugify;

/// Where a wikilink points.
#[derive(Debug, PartialEq)]
pub enum WikiResolution {
    Found {
        path: PathBuf,
        /// The heading slug after `#`, if any.
        anchor: Option<String>,
    },
    Broken,
}

/// Resolves `[[target]]` or `[[target#Heading]]` written in `from_doc`, case-insensitively. The
/// rules, each tried nearest-first:
/// 1. a target with a `/` is a path suffix of a note, extension left off;
/// 2. a note's file stem equals the target;
/// 3. a note's frontmatter `name:` equals the target;
/// 4. a note's file stem equals the target with `-` and `_` treated alike.
///
/// Nearest-first means the doc's own folder, then its ancestor folders, nearest first, then
/// anywhere in the doc's root, the shortest path winning and natural order breaking ties.
/// `[[#Heading]]` is a heading in `from_doc` itself.
pub fn resolve_wikilink(index: &LibraryIndex, from_doc: &Path, target: &str) -> WikiResolution {
    let (name, heading) = match target.split_once('#') {
        Some((name, heading)) => (name.trim(), Some(heading)),
        None => (target.trim(), None),
    };
    let anchor = heading.map(slugify).filter(|slug| !slug.is_empty());
    if name.is_empty() {
        return WikiResolution::Found {
            path: from_doc.to_path_buf(),
            anchor,
        };
    }
    let Some((root, doc_dir)) = root_and_dir(index, from_doc) else {
        return WikiResolution::Broken;
    };
    let found = (name.contains('/'))
        .then(|| nearest(&doc_dir, with_path_suffix(root, name)))
        .flatten()
        .or_else(|| nearest(&doc_dir, root.with_stem(name).collect()))
        .or_else(|| nearest(&doc_dir, root.with_fm_name(name).collect()))
        .or_else(|| nearest(&doc_dir, with_loose_stem(root, name)));
    match found {
        Some(file) => WikiResolution::Found {
            path: root.abs(&file.rel),
            anchor,
        },
        None => WikiResolution::Broken,
    }
}

/// Resolves the relative path `rel` written in `from_doc`: against the doc's folder, then each
/// ancestor folder up to its root, then by a library suffix match. Only files the index holds
/// are returned, with the spelling the index has. `rel` may use either separator.
pub fn resolve_relative(index: &LibraryIndex, from_doc: &Path, rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if let Some((root, doc_dir)) = root_and_dir(index, from_doc) {
        let mut dir: Vec<&str> = doc_dir.split('/').filter(|d| !d.is_empty()).collect();
        loop {
            if let Some(file) = join_within(&dir, &parts).and_then(|rel| root.get(&rel)) {
                return Some(root.abs(&file.rel));
            }
            if dir.pop().is_none() {
                break;
            }
        }
    }
    library_suffix_match(index, &parts)
}

/// For each root, a component of `parts` equal to the root's folder name, and the components
/// after it naming a file in that root. Later components are tried first.
pub(crate) fn library_suffix_match(index: &LibraryIndex, parts: &[&str]) -> Option<PathBuf> {
    index.roots.iter().find_map(|root| {
        let name = root.root.file_name()?.to_string_lossy();
        (0..parts.len().saturating_sub(1))
            .rev()
            .filter(|&i| eq_ignore_case(parts[i], &name))
            .find_map(|i| root.get(&parts[i + 1..].join("/")))
            .map(|file| root.abs(&file.rel))
    })
}

/// The one indexed file whose path ends with the last k components of `parts`, trying k = 4, 3
/// and 2. Catches links to notes that have since moved, such as `work/x` → `archive/x`. `None`
/// when no k gives exactly one file.
pub(crate) fn moved_file_match(index: &LibraryIndex, parts: &[&str]) -> Option<PathBuf> {
    let max_k = parts.len().min(4);
    if max_k < 2 {
        return None;
    }
    let tail = &parts[parts.len() - max_k..];
    // Each file sharing at least two trailing components, with how many it shares.
    let mut hits: Vec<(usize, String, PathBuf)> = Vec::new();
    for root in &index.roots {
        for file in &root.files {
            let shared = file
                .rel
                .rsplit('/')
                .zip(tail.iter().rev())
                .take_while(|(a, b)| eq_ignore_case(a, b))
                .count();
            if shared >= 2 {
                let abs = root.abs(&file.rel);
                hits.push((shared, path_key(&abs), abs));
            }
        }
    }
    for k in (2..=max_k).rev() {
        let mut matches = hits.iter().filter(|(shared, ..)| *shared >= k);
        let Some((_, first_key, first)) = matches.next() else {
            continue;
        };
        // Nested roots index the same file twice; that's still one file.
        return matches
            .all(|(_, key, _)| key == first_key)
            .then(|| first.clone());
    }
    None
}

/// The root holding `doc` and the doc's folder within it, as a path key (`/`-separated,
/// lowercase, empty at the root).
fn root_and_dir<'i>(index: &'i LibraryIndex, doc: &Path) -> Option<(&'i RootIndex, String)> {
    let root = index.root_for(doc)?;
    let doc_key = path_key(doc);
    let rel = key_under(&doc_key, &path_key(&root.root))?;
    let dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
    Some((root, dir))
}

/// `parts` appended to `dir`, applying `..`. `None` when `..` climbs above the root.
fn join_within(dir: &[&str], parts: &[&str]) -> Option<String> {
    let mut joined: Vec<&str> = dir.to_vec();
    for &part in parts {
        if part == ".." {
            joined.pop()?;
        } else {
            joined.push(part);
        }
    }
    Some(joined.join("/"))
}

/// The best of `candidates` for a link in `doc_dir`: the doc's folder, then its ancestors from
/// the nearest, then anywhere else, the shortest path first and natural order breaking ties.
fn nearest<'r>(doc_dir: &str, candidates: Vec<&'r FileEntry>) -> Option<&'r FileEntry> {
    candidates.into_iter().min_by(|a, b| {
        levels_up(doc_dir, &a.rel)
            .cmp(&levels_up(doc_dir, &b.rel))
            .then_with(|| a.rel.len().cmp(&b.rel.len()))
            .then_with(|| natural_cmp(&a.rel, &b.rel))
    })
}

/// How many folders up from `doc_dir` the file at `rel` sits: 0 in `doc_dir` itself, 1 in its
/// parent and so on. `usize::MAX` when the file's folder isn't `doc_dir` or one of its ancestors.
fn levels_up(doc_dir: &str, rel: &str) -> usize {
    let file_dir = path_key(Path::new(rel.rsplit_once('/').map_or("", |(dir, _)| dir)));
    let depth = |dir: &str| dir.split('/').filter(|d| !d.is_empty()).count();
    let is_ancestor = file_dir.is_empty()
        || doc_dir == file_dir
        || doc_dir
            .strip_prefix(file_dir.as_str())
            .is_some_and(|rest| rest.starts_with('/'));
    if is_ancestor {
        depth(doc_dir) - depth(&file_dir)
    } else {
        usize::MAX
    }
}

/// Markdown files whose path, without its extension, ends with the `/`-separated `target`.
fn with_path_suffix<'r>(root: &'r RootIndex, target: &str) -> Vec<&'r FileEntry> {
    let target = path_key(Path::new(target.trim_start_matches('/')));
    root.md_files()
        .filter(|file| {
            let rel = path_key(Path::new(&file.rel));
            let stem = Path::new(&rel).with_extension("");
            ends_with_path(&stem.to_string_lossy(), &target) || ends_with_path(&rel, &target)
        })
        .collect()
}

/// Whether the path key `path` is `suffix` or ends with `/suffix`.
fn ends_with_path(path: &str, suffix: &str) -> bool {
    path.strip_suffix(suffix)
        .is_some_and(|head| head.is_empty() || head.ends_with('/'))
}

/// Markdown files whose stem equals `name` once `-` and `_` count as the same character.
fn with_loose_stem<'r>(root: &'r RootIndex, name: &str) -> Vec<&'r FileEntry> {
    if !name.contains(['-', '_']) {
        return Vec::new();
    }
    let loose = |s: &str| {
        s.chars()
            .flat_map(char::to_lowercase)
            .map(|c| if c == '-' { '_' } else { c })
            .collect::<String>()
    };
    let name = loose(name);
    root.md_files()
        .filter(|file| {
            Path::new(&file.rel)
                .file_stem()
                .is_some_and(|stem| loose(&stem.to_string_lossy()) == name)
        })
        .collect()
}

/// Case-insensitive equality, as Windows compares file names.
pub(crate) fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}
