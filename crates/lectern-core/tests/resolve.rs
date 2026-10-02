mod common;

use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::library::resolve::{resolve_relative, resolve_wikilink, WikiResolution};
use lectern_core::library::LibraryIndex;

/// The fixture vault, copied and indexed.
struct Vault {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    ix: LibraryIndex,
}

impl Vault {
    fn new() -> Self {
        Self::with_files(&[])
    }

    /// The fixture vault plus extra Markdown files, written before the scan.
    fn with_files(extra: &[&str]) -> Self {
        let (tmp, root) = common::vault_copy();
        for rel in extra {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "# Extra\n").unwrap();
        }
        let ix = common::index_of(&root);
        Self {
            _tmp: tmp,
            root,
            ix,
        }
    }

    fn doc(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn wiki(&self, from: &str, target: &str) -> WikiResolution {
        resolve_wikilink(&self.ix, &self.doc(from), target)
    }

    fn found(&self, rel: &str) -> WikiResolution {
        WikiResolution::Found {
            path: self.doc(rel),
            anchor: None,
        }
    }

    fn relative(&self, from: &str, rel: &str) -> Option<PathBuf> {
        resolve_relative(&self.ix, &self.doc(from), rel)
    }
}

#[test]
fn wikilink_by_stem() {
    let v = Vault::new();
    assert!(matches!(
        v.wiki("memory/index.md", "README"),
        WikiResolution::Found { .. }
    ));
}

#[test]
fn wikilink_nearest_first() {
    let v = Vault::new();
    // From memory/, the vault's own README is an ancestor; work/alpha and archive/beta are not.
    assert_eq!(v.wiki("memory/index.md", "README"), v.found("README.md"));
    // From a task's notes, the task README is the nearest ancestor.
    assert_eq!(
        v.wiki("work/alpha/notes/2026-01-02-notes.md", "README"),
        v.found("work/alpha/README.md")
    );
    // A README links to itself before any other.
    assert_eq!(
        v.wiki("archive/beta/README.md", "README"),
        v.found("archive/beta/README.md")
    );
}

#[test]
fn wikilink_same_folder_beats_ancestors() {
    let v = Vault::with_files(&["work/alpha/notes/README.md"]);
    assert_eq!(
        v.wiki("work/alpha/notes/2026-01-02-notes.md", "README"),
        v.found("work/alpha/notes/README.md")
    );
}

#[test]
fn wikilink_elsewhere_prefers_the_shortest_path() {
    let v = Vault::with_files(&["deep/er/still/note.md", "other/note.md", "aaa/zzz/note.md"]);
    assert_eq!(v.wiki("memory/index.md", "note"), v.found("other/note.md"));
}

#[test]
fn wikilink_elsewhere_breaks_length_ties_in_natural_order() {
    // Plain string order would put `b10` first.
    let v = Vault::with_files(&["b10/note.md", "b9x/note.md"]);
    assert_eq!(v.wiki("memory/index.md", "note"), v.found("b9x/note.md"));
}

#[test]
fn wikilink_ignores_case() {
    let v = Vault::new();
    assert_eq!(v.wiki("memory/index.md", "readme"), v.found("README.md"));
    assert_eq!(
        v.wiki("memory/index.md", "FEEDBACK_INCREMENTAL_PRS"),
        v.found("memory/feedback_incremental_prs.md")
    );
}

#[test]
fn wikilink_by_frontmatter_name() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "feedback-incremental-prs"),
        v.found("memory/feedback_incremental_prs.md")
    );
}

#[test]
fn wikilink_by_hyphen_underscore_swap() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "feedback_incremental-prs"),
        v.found("memory/feedback_incremental_prs.md")
    );
}

#[test]
fn wikilink_swap_treats_hyphens_and_underscores_alike() {
    let v = Vault::with_files(&["elsewhere/mixed-up_name.md"]);
    assert_eq!(
        v.wiki("memory/index.md", "Mixed_Up-Name"),
        v.found("elsewhere/mixed-up_name.md")
    );
}

#[test]
fn wikilink_stem_beats_frontmatter_name() {
    // The big plan's `name:` is `big-plan`; a note whose stem is `big-plan` wins over it.
    let v = Vault::with_files(&["elsewhere/big-plan.md"]);
    assert_eq!(
        v.wiki("memory/index.md", "big-plan"),
        v.found("elsewhere/big-plan.md")
    );
}

#[test]
fn wikilink_heading_anchor() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "big-plan#Section 2"),
        WikiResolution::Found {
            path: v.doc("work/alpha/plans/2026-01-01-big-plan.md"),
            anchor: Some("section-2".into()),
        }
    );
}

#[test]
fn wikilink_to_a_heading_in_the_same_note() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "#Memory index"),
        WikiResolution::Found {
            path: v.doc("memory/index.md"),
            anchor: Some("memory-index".into()),
        }
    );
}

#[test]
fn wikilink_with_a_folder_matches_the_path_suffix() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "alpha/README"),
        v.found("work/alpha/README.md")
    );
    assert_eq!(
        v.wiki("memory/index.md", "beta/readme"),
        v.found("archive/beta/README.md")
    );
    assert_eq!(
        v.wiki("memory/index.md", "work/alpha/plans/2026-01-01-big-plan"),
        v.found("work/alpha/plans/2026-01-01-big-plan.md")
    );
    assert_eq!(
        v.wiki("memory/index.md", "lpha/README"),
        WikiResolution::Broken
    );
}

#[test]
fn wikilink_broken() {
    let v = Vault::new();
    assert_eq!(
        v.wiki("memory/index.md", "missing-note"),
        WikiResolution::Broken
    );
}

#[test]
fn wikilink_from_outside_every_root_is_broken() {
    let v = Vault::new();
    assert_eq!(
        resolve_wikilink(&v.ix, Path::new("/elsewhere/note.md"), "README"),
        WikiResolution::Broken
    );
}

#[test]
fn relative_to_task_root() {
    let v = Vault::new();
    assert_eq!(
        v.relative(
            "work/alpha/notes/2026-01-02-notes.md",
            "plans/2026-01-01-big-plan.md"
        ),
        Some(v.doc("work/alpha/plans/2026-01-01-big-plan.md"))
    );
    assert_eq!(
        v.relative("work/alpha/notes/2026-01-02-notes.md", "README.md"),
        Some(v.doc("work/alpha/README.md"))
    );
}

#[test]
fn relative_to_the_doc_folder() {
    let v = Vault::new();
    let from = "work/alpha/notes/2026-01-02-notes.md";
    assert_eq!(
        v.relative(from, "../README.md"),
        Some(v.doc("work/alpha/README.md"))
    );
    assert_eq!(v.relative(from, "./2026-01-02-notes.md"), Some(v.doc(from)));
    assert_eq!(
        v.relative("README.md", "notes/résumé notes.md"),
        Some(v.doc("notes/résumé notes.md"))
    );
}

#[test]
fn relative_returns_the_indexed_spelling() {
    let v = Vault::new();
    assert_eq!(
        v.relative("work/alpha/README.md", r"PLANS\2026-01-01-BIG-PLAN.MD"),
        Some(v.doc("work/alpha/plans/2026-01-01-big-plan.md"))
    );
}

#[test]
fn relative_finds_any_indexed_file() {
    let v = Vault::new();
    assert_eq!(
        v.relative("friends/readme-style.md", "img/logo.png"),
        Some(v.doc("friends/img/logo.png"))
    );
}

#[test]
fn relative_by_library_suffix() {
    let v = Vault::new();
    assert_eq!(
        v.relative("memory/index.md", "shared/vault/work/alpha/README.md"),
        Some(v.doc("work/alpha/README.md"))
    );
}

#[test]
fn relative_missing_or_escaping_the_root_is_none() {
    let v = Vault::new();
    assert_eq!(v.relative("work/alpha/README.md", "plans/nope.md"), None);
    assert_eq!(v.relative("README.md", "../README.md"), None);
    assert_eq!(v.relative("README.md", "work/alpha/"), None);
}
