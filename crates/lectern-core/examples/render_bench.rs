//! Times `render` on the 3,000-line fixture plan and on a synthetic 5 MB note, with an index of
//! `fixtures/vault` so links, wikilinks and inline-code paths resolve as they do in the app.
//!
//! `cargo run -p lectern-core --release --example render_bench`. The spec's budget for the plan is
//! a median of 30 ms or less in a release build.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::LibraryIndex;
use lectern_core::render::{highlight, render, RenderContext};

const VAULT: &str = "../../fixtures/vault";
const BIG_PLAN: &str = "work/alpha/plans/2026-01-01-big-plan.md";
const RUNS: usize = 20;
const SYNTHETIC_BYTES: usize = 5 * 1024 * 1024;

fn main() {
    let vault = Path::new(env!("CARGO_MANIFEST_DIR")).join(VAULT);
    let path = vault.join(BIG_PLAN);
    let source = std::fs::read_to_string(&path).expect("the big plan fixture is readable");

    let start = Instant::now();
    let mut root = scan_root(&vault, &ScanOptions::default()).expect("the fixture vault scans");
    read_heads(&mut root);
    let index = LibraryIndex { roots: vec![root] };
    println!(
        "index of fixtures/vault ({} files): {:.2} ms",
        index.roots[0].files.len(),
        ms(start.elapsed())
    );

    let mapper = PathMapper::default();
    let ctx = RenderContext {
        doc_path: &path,
        index: Some(&index),
        mapper: &mapper,
        asset_base: "http://asset.localhost/",
    };

    let start = Instant::now();
    highlight::warm_up();
    println!(
        "warm-up (syntax set + common grammars): {:.2} ms",
        ms(start.elapsed())
    );

    // Whatever the warm-up didn't reach (other grammar states, per-thread regex caches) is paid
    // here.
    let start = Instant::now();
    std::hint::black_box(render(&source, &ctx));
    println!("first render after warm-up: {:.2} ms", ms(start.elapsed()));

    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(render(&source, &ctx));
            start.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "big plan ({} lines, {} KB), {RUNS} runs: min {:.2} ms, median {:.2} ms, max {:.2} ms",
        source.lines().count(),
        source.len() / 1024,
        ms(times[0]),
        ms(median(&times)),
        ms(times[RUNS - 1]),
    );

    let synthetic = synthetic_note(SYNTHETIC_BYTES);
    let start = Instant::now();
    std::hint::black_box(render(&synthetic, &ctx));
    println!(
        "synthetic note ({:.1} MB, {} lines), 1 run: {:.2} ms",
        synthetic.len() as f64 / (1024.0 * 1024.0),
        synthetic.lines().count(),
        ms(start.elapsed()),
    );
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The middle of a sorted, even-length list: the mean of the two middle values.
fn median(sorted: &[Duration]) -> Duration {
    let mid = sorted.len() / 2;
    (sorted[mid - 1] + sorted[mid]) / 2
}

/// Repeats a section with a heading, prose, tags, raw HTML, a table, tasks and a code fence until
/// the note reaches `bytes`.
fn synthetic_note(bytes: usize) -> String {
    let mut src = String::with_capacity(bytes + 1024);
    let mut i = 0;
    while src.len() < bytes {
        i += 1;
        write!(
            src,
            "## Section {i}\n\n\
             Paragraph {i} with *emphasis*, `src/lib_{i}.rs`, a [link](https://example.com/{i}), \
             a bare https://example.com/bare/{i} URL, #tag-{i} and <kbd>Ctrl</kbd> + <slug>.\n\n\
             | Name | Path | Notes |\n| --- | --- | --- |\n| row {i} | `a/b/{i}.md` | x<br>y |\n\n\
             - [ ] open task {i}\n- [x] done task {i}\n\n\
             ```ruby\ndef step_{i}(record)\n  record.update!(name: \"step-{i}\") if record.valid?\nend\n```\n\n"
        )
        .expect("writing to a String cannot fail");
    }
    src
}
