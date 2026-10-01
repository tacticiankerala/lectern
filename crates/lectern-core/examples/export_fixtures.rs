//! Renders every Markdown file in `fixtures/vault`, with an index of the vault, for the UI's fake
//! backend, and writes them to `ui/dev/fixtures.json` with the library tree and the quick-open
//! candidates.
//!
//! Paths are rewritten under a fake Windows root, `C:\Fixtures\vault`, so the UI sees the paths it
//! sees on Windows. Images point at `/__asset/<encoded path>`, which `ui/dev/serve.mjs` maps back
//! into the vault.
//!
//! `cargo run -p lectern-core --example export_fixtures` (`npm run fixtures` in `ui/`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lectern_core::ipc::Candidate;
use lectern_core::library::pathmap::{asset_url, PathMapper};
use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::tree::{build_tree, TreeNode};
use lectern_core::library::LibraryIndex;
use lectern_core::render::{render, RenderContext, RenderedDoc};
use lectern_core::text::decode;
use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};
use serde::Serialize;

const VAULT: &str = "../../fixtures/vault";
const OUT: &str = "../../ui/dev/fixtures.json";
const FAKE_ROOT: &str = r"C:\Fixtures\vault";
/// Same origin as the fake server, which serves the vault's files under it.
const ASSET_BASE: &str = "/__asset/";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Fixtures {
    root: String,
    tree: TreeNode,
    candidates: Vec<Candidate>,
    /// By path.
    docs: BTreeMap<String, RenderedDoc>,
}

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vault = PathBuf::from(clean_root(
        &fs::canonicalize(manifest.join(VAULT)).expect("fixtures/vault exists"),
    ));
    let remap = Remap::new(&vault);

    let mut root = scan_root(&vault, &ScanOptions::default()).expect("the fixture vault scans");
    read_heads(&mut root);
    let index = LibraryIndex { roots: vec![root] };
    let root = &index.roots[0];
    let mapper = PathMapper::default();

    let mut docs = BTreeMap::new();
    let mut candidates = Vec::new();
    for file in root.md_files() {
        let path = root.abs(&file.rel);
        let bytes = fs::read(&path).expect("a fixture is readable");
        let text = decode(&bytes).expect("a fixture is text").text;
        let ctx = RenderContext {
            doc_path: &path,
            index: Some(&index),
            mapper: &mapper,
            asset_base: ASSET_BASE,
            trusted_unc_hosts: &[],
        };
        let mut doc = render(&text, &ctx);
        doc.html = remap.html(&doc.html);
        let fake_path = remap.path(&path.to_string_lossy());
        candidates.push(Candidate {
            path: fake_path.clone(),
            name: file.rel.rsplit('/').next().unwrap_or(&file.rel).to_owned(),
            rel: file.rel.clone(),
            root: FAKE_ROOT.to_owned(),
        });
        docs.insert(fake_path, doc);
    }

    let mut tree = build_tree(root);
    remap.tree(&mut tree);
    let fixtures = Fixtures {
        root: FAKE_ROOT.to_owned(),
        tree,
        candidates,
        docs,
    };
    let json = serde_json::to_string(&fixtures).expect("fixtures serialise");
    remap.assert_gone(&json);
    let out = manifest.join(OUT);
    fs::write(&out, &json).expect("ui/dev is writable");
    let out = fs::canonicalize(&out).unwrap_or(out);
    println!(
        "wrote {} documents ({} KB) to {}",
        fixtures.docs.len(),
        json.len() / 1024,
        out.display()
    );
}

/// The canonical vault path as the renderer will write it: without Windows' verbatim `\\?\`
/// prefix, which `fs::canonicalize` adds there.
fn clean_root(canonical: &Path) -> String {
    let text = canonical.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

/// Rewrites paths under the real vault to paths under `FAKE_ROOT`.
struct Remap {
    real: String,
    /// The real vault in rendered HTML: as written in attributes, and percent-encoded in asset
    /// URLs.
    plain: Regex,
    asset: Regex,
}

impl Remap {
    fn new(vault: &Path) -> Self {
        let real = vault.to_string_lossy().into_owned();
        Self {
            plain: Regex::new(&format!(r#"{}[^"<]*"#, regex::escape(&real)))
                .expect("the vault pattern is valid"),
            asset: Regex::new(&format!(r#"{}([^"\s,]+)"#, regex::escape(ASSET_BASE)))
                .expect("the asset pattern is valid"),
            real,
        }
    }

    /// `path` under `FAKE_ROOT` with `\` separators when it lies in the vault; as given otherwise.
    /// Either separator may follow the vault: Windows joins with `\`, Linux with `/`.
    fn path(&self, path: &str) -> String {
        let Some(rest) = path.strip_prefix(&self.real) else {
            return path.to_owned();
        };
        if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
            return path.to_owned();
        }
        rest.split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .fold(FAKE_ROOT.to_owned(), |out, part| format!("{out}\\{part}"))
    }

    /// Rewrites asset URLs first (their paths are percent-encoded), then plain paths in
    /// attributes such as `data-target`.
    fn html(&self, html: &str) -> String {
        let html = self.asset.replace_all(html, |c: &Captures| {
            let decoded = percent_decode_str(&c[1]).decode_utf8_lossy();
            asset_url(ASSET_BASE, Path::new(&self.path(&decoded)))
        });
        self.plain
            .replace_all(&html, |c: &Captures| self.path(&c[0]))
            .into_owned()
    }

    fn tree(&self, node: &mut TreeNode) {
        node.path = self.path(&node.path);
        node.readme = node.readme.as_deref().map(|r| self.path(r));
        for child in &mut node.children {
            self.tree(child);
        }
    }

    /// Panics when the output still names the real vault, in either form.
    fn assert_gone(&self, json: &str) {
        let encoded = asset_url("", Path::new(&self.real));
        assert!(
            !json.contains(&self.real) && !json.contains(&encoded),
            "the export still holds a path under {}",
            self.real
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_VAULT: &str = r"D:\a\lectern\fixtures\vault";

    #[test]
    fn paths_under_a_windows_vault_move_under_the_fake_root() {
        let remap = Remap::new(Path::new(WINDOWS_VAULT));
        let path = |p: &str| remap.path(p);
        assert_eq!(
            path(r"D:\a\lectern\fixtures\vault\memory\index.md"),
            r"C:\Fixtures\vault\memory\index.md"
        );
        assert_eq!(path(WINDOWS_VAULT), FAKE_ROOT);
        assert_eq!(
            path(r"D:\a\lectern\fixtures\vault/notes/x.md"),
            r"C:\Fixtures\vault\notes\x.md"
        );
        // Not inside the vault: left alone.
        assert_eq!(
            path(r"D:\a\lectern\fixtures\vault2\x.md"),
            r"D:\a\lectern\fixtures\vault2\x.md"
        );
        assert_eq!(path("/home/me/x.md"), "/home/me/x.md");
    }

    #[test]
    fn paths_under_a_linux_vault_move_under_the_fake_root() {
        let remap = Remap::new(Path::new("/home/u/lectern/fixtures/vault"));
        assert_eq!(
            remap.path("/home/u/lectern/fixtures/vault/notes/résumé notes.md"),
            r"C:\Fixtures\vault\notes\résumé notes.md"
        );
    }

    #[test]
    fn targets_and_asset_urls_in_html_are_rewritten() {
        let remap = Remap::new(Path::new(WINDOWS_VAULT));
        let html = concat!(
            r#"<a data-kind="doc" data-target="D:\a\lectern\fixtures\vault\README.md">x</a>"#,
            r#"<img src="/__asset/D%3A%5Ca%5Clectern%5Cfixtures%5Cvault%5Cimg%5Clogo.png">"#,
        );
        let out = remap.html(html);
        assert_eq!(
            out,
            concat!(
                r#"<a data-kind="doc" data-target="C:\Fixtures\vault\README.md">x</a>"#,
                r#"<img src="/__asset/C%3A%5CFixtures%5Cvault%5Cimg%5Clogo.png">"#,
            )
        );
        remap.assert_gone(&out);
    }

    #[test]
    fn a_verbatim_windows_root_loses_its_prefix() {
        assert_eq!(
            clean_root(Path::new(r"\\?\D:\a\lectern\fixtures\vault")),
            WINDOWS_VAULT
        );
        assert_eq!(clean_root(Path::new("/home/u/vault")), "/home/u/vault");
    }
}
