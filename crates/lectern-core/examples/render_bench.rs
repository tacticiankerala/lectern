//! Times `render` on the 3,000-line fixture plan and on a synthetic 5 MB note, with an index of
//! `fixtures/vault` so links, wikilinks and inline-code paths resolve as they do in the app. Then
//! times re-anchoring 100 review comments on the plan after it changed.
//!
//! `cargo run -p lectern-core --release --example render_bench`. The spec's budget for the plan is
//! a median of 30 ms or less in a release build, and 15 ms for the review comments.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::scan::{read_heads, scan_root, ScanOptions};
use lectern_core::library::LibraryIndex;
use lectern_core::render::{highlight, render, RenderContext};
use lectern_core::review::anchor::{self, AnchorState};
use lectern_core::review::ops::{self, NewAnchor, OpContext, ReviewOp};
use lectern_core::review::text::TextMap;
use lectern_core::review::{fingerprint, format, Item, Review};

const VAULT: &str = "../../fixtures/vault";
const BIG_PLAN: &str = "work/alpha/plans/2026-01-01-big-plan.md";
const RUNS: usize = 20;
const SYNTHETIC_BYTES: usize = 5 * 1024 * 1024;
const REVIEW_COMMENTS: usize = 100;
/// Of those, how many quotes are reworded so they're found only word by word.
const REWORDED: usize = 20;

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
        asset_base: "http://lxasset.localhost/",
        trusted_unc_hosts: &[],
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

    review_bench(&source);
}

/// Comments on 100 passages of the plan: times building the text map, re-anchoring every comment
/// and finding where each is in the text for the UI, as opening a note with a sidecar does. First
/// on the plan unchanged, then with 3 lines added at the top of its body, then with 20 of the
/// quotes reworded too, so those take the fuzzy search.
fn review_bench(source: &str) {
    let mut review = review_of(source);
    time_review("note unchanged", &review, source);
    let body = frontmatter_end(source);
    let shifted = format!(
        "{}Three lines\nadded at\nthe top.\n{}",
        &source[..body],
        &source[body..]
    );
    time_review("note shifted 3 lines", &review, &shifted);

    for item in &mut review.items {
        if let Item::Comment(c) = item {
            if (c.id as usize).is_multiple_of(REVIEW_COMMENTS / REWORDED) {
                c.quote = reword(&c.quote);
            }
        }
    }
    time_review(&format!("{REWORDED} quotes reworded"), &review, &shifted);
}

/// A review with a comment on every so many blocks of ten words or more: on the whole block, or
/// on eight words from inside it, in turn.
fn review_of(source: &str) -> Review {
    let map = TextMap::build(source);
    let fp = fingerprint(source);
    let ctx = OpContext {
        text: &map,
        fingerprint: &fp,
        now: "2026-10-06T10:00:00Z",
    };
    let prose: Vec<_> = map
        .blocks()
        .iter()
        .filter(|b| map.block_text(b).split(' ').count() >= 10)
        .collect();
    let step = (prose.len() / REVIEW_COMMENTS).max(1);
    let mut review = format::new_review("2026-01-01-big-plan.md");
    for (i, block) in prose.iter().step_by(step).take(REVIEW_COMMENTS).enumerate() {
        let text = map.block_text(block);
        let quote = match i % 2 {
            0 => text.to_owned(),
            _ => text
                .split(' ')
                .skip(1)
                .take(8)
                .collect::<Vec<_>>()
                .join(" "),
        };
        let anchor = NewAnchor {
            start_line: block.start_line,
            end_line: block.end_line,
            quote,
            prefix: String::new(),
        };
        let op = ReviewOp::Add {
            anchor,
            text: format!("Comment {i}"),
        };
        ops::apply(&mut review, &op, &ctx).expect("a comment on the plan is added");
    }
    review
}

/// The byte where the note's body starts, after its frontmatter.
fn frontmatter_end(source: &str) -> usize {
    source
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---\n"))
        .map_or(0, |at| "---\n".len() + at + "\n---\n".len())
}

/// `quote` with its middle word replaced.
fn reword(quote: &str) -> String {
    let mut words: Vec<&str> = quote.split(' ').collect();
    let middle = words.len() / 2;
    words[middle] = "reworded";
    words.join(" ")
}

fn time_review(label: &str, review: &Review, source: &str) {
    let fp = fingerprint(source);
    // As the payload does: a comment left where it was is looked for in its lines.
    let resolve = || {
        let map = TextMap::build(source);
        let mut resolved = anchor::resolve_all(review, &map, &fp);
        for (r, c) in resolved.iter_mut().zip(review.comments()) {
            if r.state == AnchorState::Anchored && r.span.is_none() {
                r.span = anchor::stored_span(c, &map);
            }
        }
        resolved
    };
    let states = resolve();
    let count = |state| states.iter().filter(|r| r.state == state).count();
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(resolve());
            start.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "review: {} comments, {label}, text map + re-anchoring + places, {RUNS} runs: \
         min {:.2} ms, median {:.2} ms, max {:.2} ms \
         ({} anchored, {} moved, {} detached)",
        states.len(),
        ms(times[0]),
        ms(median(&times)),
        ms(times[RUNS - 1]),
        count(AnchorState::Anchored),
        count(AnchorState::Moved),
        count(AnchorState::Detached),
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
