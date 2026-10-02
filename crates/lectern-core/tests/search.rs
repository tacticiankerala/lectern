mod common;

use std::fs::{self, File};
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

use common::{index_of, native_join, vault_copy};
use lectern_core::library::scan::{scan_root, ScanOptions};
use lectern_core::library::LibraryIndex;
use lectern_core::search::{ContentCache, FileHits, SearchHit};

/// A library of one root holding `files` (`rel`, contents) in a fresh temporary directory.
fn library(files: &[(&str, &[u8])]) -> (tempfile::TempDir, LibraryIndex) {
    let tmp = tempfile::tempdir().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let index = index_of(tmp.path());
    (tmp, index)
}

fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = native_join(root, rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn search(index: &LibraryIndex, query: &str) -> Vec<FileHits> {
    ContentCache::new().search(index, query)
}

fn result<'a>(results: &'a [FileHits], rel: &str) -> &'a FileHits {
    let rels: Vec<&str> = results.iter().map(|f| f.rel.as_str()).collect();
    results
        .iter()
        .find(|f| f.rel == rel)
        .unwrap_or_else(|| panic!("{rel} not found; got {rels:?}"))
}

fn line_numbers(file: &FileHits) -> Vec<u32> {
    file.hits.iter().map(|h| h.line).collect()
}

/// The text of each marked segment.
fn marked(hit: &SearchHit) -> Vec<&str> {
    hit.segments
        .iter()
        .filter(|s| s.hit)
        .map(|s| s.text.as_str())
        .collect()
}

/// The whole snippet, ellipses included.
fn snippet(hit: &SearchHit) -> String {
    hit.segments.iter().map(|s| s.text.as_str()).collect()
}

fn assert_alternates(hit: &SearchHit) {
    assert!(
        hit.segments.windows(2).all(|w| w[0].hit != w[1].hit),
        "segments must alternate: {:?}",
        hit.segments
    );
    assert!(hit.segments.iter().all(|s| !s.text.is_empty()));
}

#[test]
fn smart_case() {
    let (_tmp, index) = library(&[(
        "notes.md",
        "Plan A\nthe plan\nPLANS\nnothing here\nÉmile wrote\nsaw émile\n".as_bytes(),
    )]);

    let lower = search(&index, "plan");
    assert_eq!(line_numbers(&lower[0]), [1, 2, 3]);
    let texts: Vec<_> = lower[0].hits.iter().flat_map(marked).collect();
    assert_eq!(texts, ["Plan", "plan", "PLAN"]);

    let upper = search(&index, "Plan");
    assert_eq!(line_numbers(&upper[0]), [1]);
    assert_eq!(marked(&upper[0].hits[0]), ["Plan"]);

    // Lowercase non-ASCII queries ignore case too; any uppercase letter makes the search exact.
    assert_eq!(line_numbers(&search(&index, "émile")[0]), [5, 6]);
    assert_eq!(line_numbers(&search(&index, "Émile")[0]), [5]);
}

#[test]
fn segments_mark_hits() {
    let long = format!(
        "{}needle and another needle {}",
        "a ".repeat(50),
        "b ".repeat(50)
    );
    let text = format!("intro\n\n{long}\nlast line\n");
    let (_tmp, index) = library(&[("doc.md", text.as_bytes())]);

    let results = search(&index, "needle");
    assert_eq!(results.len(), 1);
    let hits = &results[0].hits;
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit.line, 3, "line numbers are 1-based");
    assert_eq!(marked(hit), ["needle", "needle"]);
    assert_alternates(hit);

    let snippet = snippet(hit);
    assert!(
        snippet.starts_with('…') && snippet.ends_with('…'),
        "{snippet:?}"
    );
    // 60 characters either side of the first hit, plus the hit and two ellipses.
    assert!(snippet.chars().count() <= 60 + 6 + 60 + 2, "{snippet:?}");
    assert!(long.contains(snippet.trim_matches('…')));
}

#[test]
fn short_lines_are_shown_whole_and_trimmed() {
    let (_tmp, index) = library(&[("doc.md", b"  - [ ] ship the needle  \r\nend\r\n")]);
    let hit = &search(&index, "needle")[0].hits[0];
    assert_eq!(hit.line, 1);
    assert_eq!(snippet(hit), "- [ ] ship the needle");
    assert_eq!(marked(hit), ["needle"]);
}

#[test]
fn adjacent_hits_merge_into_one_segment() {
    let (_tmp, index) = library(&[("doc.md", b"xx abab yy\n")]);
    let hit = &search(&index, "ab")[0].hits[0];
    assert_eq!(marked(hit), ["abab"]);
    assert_alternates(hit);
}

#[test]
fn caps_per_file_and_total() {
    let big: String = (1..=1000)
        .map(|i| format!("step {i}: keep the marker here\n"))
        .collect();
    let small = "marker\n".repeat(6);
    let mut files: Vec<(String, Vec<u8>)> = vec![("big.md".to_owned(), big.into_bytes())];
    for n in 0..110 {
        files.push((format!("notes/n{n:03}.md"), small.clone().into_bytes()));
    }
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(rel, text)| (rel.as_str(), text.as_slice()))
        .collect();
    let (_tmp, index) = library(&files);

    let results = search(&index, "marker");
    assert_eq!(results[0].rel, "big.md", "the most hits rank first");
    assert_eq!(line_numbers(&results[0]), [1, 2, 3, 4, 5]);
    assert_eq!(results[0].total, 1000);
    assert!(results.iter().all(|f| f.hits.len() <= 5));
    assert!(results[1..].iter().all(|f| f.total == 6));
    let shown: usize = results.iter().map(|f| f.hits.len()).sum();
    assert_eq!(shown, 500);
    assert_eq!(results.len(), 100, "files past the cap are left out");
}

#[test]
fn name_match_ranks_first() {
    let (_tmp, vault) = vault_copy();
    let index = index_of(&vault);
    let results = search(&index, "alpha");
    assert!(results.len() > 1);
    // A README stands for its folder, as in the sidebar.
    assert_eq!(results[0].rel, "work/alpha/README.md");
    assert!(results[0].name_match);
    assert!(results[1..].iter().all(|f| !f.name_match));
}

#[test]
fn name_match_beats_hit_count() {
    let (_tmp, index) = library(&[
        ("other.md", b"alpha\nalpha\nalpha\nAlpha\n"),
        ("alpha-notes.md", b"Alpha\n"),
        ("Alpha Upper.md", b"alpha\n"),
    ]);
    let ranked = |query: &str| -> Vec<(String, bool)> {
        search(&index, query)
            .into_iter()
            .map(|f| (f.rel, f.name_match))
            .collect()
    };
    let named = |rel: &str, name_match: bool| (rel.to_owned(), name_match);
    assert_eq!(
        ranked("alpha"),
        [
            named("Alpha Upper.md", true),
            named("alpha-notes.md", true),
            named("other.md", false),
        ],
        "name matches first (then by path), then by hit count"
    );
    // File names follow the same smart case as the contents; a name alone is not a hit.
    assert_eq!(
        ranked("Alpha"),
        [named("alpha-notes.md", false), named("other.md", false)]
    );
}

#[test]
fn a_top_level_readme_goes_by_the_root_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("garden");
    write(&root, "README.md", b"garden\n");
    write(&root, "other.md", b"garden\ngarden\n");
    let results = search(&index_of(&root), "garden");
    assert_eq!(results[0].rel, "README.md");
    assert!(results[0].name_match);
    assert!(!results[1].name_match);
}

#[test]
fn the_cache_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ContentCache>();
}

#[test]
fn cache_invalidates_on_mtime_change() {
    let (tmp, index) = library(&[("a.md", b"first draft\n")]);
    let path = tmp.path().join("a.md");
    let set_modified = |t: SystemTime| {
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(t).unwrap();
    };
    // No validation window: every search checks every stamp.
    let cache = ContentCache::with_validation_window(Duration::ZERO);
    assert_eq!(cache.search(&index, "first").len(), 1);
    assert!(cache.search(&index, "other").is_empty());

    // Same size and modification time: the cached text is used.
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "other draft\n").unwrap();
    set_modified(modified);
    assert_eq!(cache.search(&index, "first").len(), 1, "served from cache");

    // Same size, new modification time.
    set_modified(modified + Duration::from_secs(5));
    assert_eq!(cache.search(&index, "other").len(), 1);
    assert!(cache.search(&index, "first").is_empty());

    // A new size.
    fs::write(&path, "second, longer draft\n").unwrap();
    assert_eq!(cache.search(&index, "second").len(), 1);
}

#[test]
fn a_search_within_the_window_trusts_the_cache() {
    let (tmp, index) = library(&[("a.md", b"first draft\n")]);
    let cache = ContentCache::with_validation_window(Duration::from_secs(3600));
    assert_eq!(cache.search(&index, "draft").len(), 1);

    // A new size would show on a stat, so finding the old text proves there was none.
    fs::write(tmp.path().join("a.md"), "second, longer draft\n").unwrap();
    assert_eq!(cache.search(&index, "first").len(), 1);
    assert!(cache.search(&index, "second").is_empty());

    // A file the cache hasn't seen is still read.
    write(tmp.path(), "b.md", b"a second note\n");
    let index = index_of(tmp.path());
    assert_eq!(result(&cache.search(&index, "second"), "b.md").total, 1);
}

#[test]
fn a_search_after_the_window_checks_again() {
    let window = Duration::from_millis(50);
    let (tmp, index) = library(&[("a.md", b"first draft\n")]);
    let cache = ContentCache::with_validation_window(window);
    assert_eq!(cache.search(&index, "first").len(), 1);

    fs::write(tmp.path().join("a.md"), "second, longer draft\n").unwrap();
    // Sleeping at least the window guarantees it has passed; a longer sleep changes nothing.
    thread::sleep(window);
    assert_eq!(cache.search(&index, "second").len(), 1);
    assert!(cache.search(&index, "first").is_empty());
}

#[test]
fn skips_files_over_5_mib() {
    const LIMIT: usize = 5 * 1024 * 1024;
    let sized = |len: usize| {
        let mut text = b"marker\n".to_vec();
        text.resize(len, b'x');
        text
    };
    let (_tmp, index) = library(&[
        ("at-limit.md", &sized(LIMIT)),
        ("over.md", &sized(LIMIT + 1)),
    ]);
    let results = search(&index, "marker");
    let rels: Vec<&str> = results.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["at-limit.md"]);
}

#[test]
fn unicode_safe_snippets() {
    let text = [
        "é→😀plan😀→é".to_owned(),
        format!("{}plan{}", "é".repeat(100), "→".repeat(100)),
        format!("{}plan{}", "😀".repeat(70), "😀".repeat(70)),
        format!("{}zplan", "e\u{301}".repeat(40)),
    ]
    .join("\n");
    let (_tmp, index) = library(&[("u.md", text.as_bytes())]);

    let results = search(&index, "plan");
    let hits = &results[0].hits;
    assert_eq!(line_numbers(&results[0]), [1, 2, 3, 4]);
    for hit in hits {
        assert_eq!(marked(hit), ["plan"]);
        assert_alternates(hit);
    }
    assert_eq!(snippet(&hits[0]), "é→😀plan😀→é");
    let cut = &hits[1].segments;
    assert_eq!(cut[0].text, format!("…{}", "é".repeat(60)));
    assert_eq!(cut[2].text, format!("{}…", "→".repeat(60)));
    let emoji = &hits[2].segments;
    assert_eq!(emoji[0].text, format!("…{}", "😀".repeat(60)));
    assert_eq!(emoji[2].text, format!("{}…", "😀".repeat(60)));
    // 81 characters before the hit: the cut lands between an `e` and its combining accent.
    let combining = &hits[3].segments[0].text;
    assert!(combining.starts_with("…\u{301}e"), "{combining:?}");
    assert_eq!(combining.chars().count(), 61);
}

#[test]
fn blank_query_finds_nothing() {
    let (_tmp, index) = library(&[("a.md", b"   spaces   and\ttabs\n")]);
    for query in ["", " ", "\t", "  \n"] {
        assert!(search(&index, query).is_empty(), "{query:?}");
    }
}

#[test]
fn skips_binary_and_searches_lossy() {
    let (_tmp, index) = library(&[
        ("bin.md", b"marker\0\x01\x02"),
        ("lossy.md", b"bad \xff\xfe bytes, marker\n"),
        ("not-markdown.txt", b"marker\n"),
    ]);
    let results = search(&index, "marker");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rel, "lossy.md");
    assert_eq!(
        snippet(&results[0].hits[0]),
        "bad \u{fffd}\u{fffd} bytes, marker"
    );
}

#[test]
fn titles_prefer_frontmatter_then_heading_then_stem() {
    let (_tmp, index) = library(&[
        ("a.md", b"---\ntitle: Front Title\n---\n# Heading\nmarker\n"),
        (
            "b.md",
            b"---\nstatus: active\n# a YAML comment\n---\n# Real Heading\nmarker\n",
        ),
        ("c.md", b"```sh\n# a shell comment\n```\nmarker\n"),
    ]);
    let results = search(&index, "marker");
    assert_eq!(result(&results, "a.md").title, "Front Title");
    assert_eq!(result(&results, "b.md").title, "Real Heading");
    assert_eq!(result(&results, "c.md").title, "c");
}

#[test]
fn titles_skip_whole_fences() {
    let (_tmp, index) = library(&[
        // A shorter run of the same character doesn't close a fence.
        (
            "long.md",
            b"````md\n```\n# not this\n```\n````\n# After Long\nmarker\n",
        ),
        // Nor does the other fence character, nor a run with text after it.
        (
            "mixed.md",
            b"~~~\n```\n# not this\n~~~~ info\n# nor this\n~~~\n# After Mixed\nmarker\n",
        ),
    ]);
    let results = search(&index, "marker");
    assert_eq!(result(&results, "long.md").title, "After Long");
    assert_eq!(result(&results, "mixed.md").title, "After Mixed");
}

#[test]
fn titles_come_from_any_yaml_form() {
    let (_tmp, index) = library(&[
        ("quoted.md", b"---\n\"title\": Quoted Key\n---\nmarker\n"),
        (
            "flow.md",
            b"---\n{title: Flow Map, status: done}\n---\nmarker\n",
        ),
    ]);
    let results = search(&index, "marker");
    assert_eq!(result(&results, "quoted.md").title, "Quoted Key");
    assert_eq!(result(&results, "flow.md").title, "Flow Map");
}

#[test]
fn nested_roots_report_a_file_once() {
    let (_tmp, vault) = vault_copy();
    let outer = scan_root(&vault, &ScanOptions::default()).unwrap();
    let inner = scan_root(&vault.join("work"), &ScanOptions::default()).unwrap();
    let index = LibraryIndex {
        roots: vec![outer, inner],
    };
    let results = search(&index, "Blocked on a review");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rel, "alpha/README.md", "the deepest root wins");
}

#[test]
fn file_hits_serialize_in_camel_case() {
    let (_tmp, index) = library(&[("a.md", b"marker\n")]);
    let json = serde_json::to_value(&search(&index, "marker")[0]).unwrap();
    assert_eq!(json["nameMatch"], false);
    assert_eq!(json["total"], 1);
    assert_eq!(json["hits"][0]["segments"][0]["hit"], true);
    assert!(json["path"].as_str().unwrap().ends_with("a.md"));
}
