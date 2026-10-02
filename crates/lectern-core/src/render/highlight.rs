//! Syntax highlighting with syntect and two-face's grammars, using the pure-Rust regex engine.
//!
//! Grammars compile lazily, regex by regex, the first time a block reaches them, and stay
//! compiled in the syntax set together with each highlighting thread's matching caches: hundreds
//! of MB once the large grammars (TSX, TypeScript) have run. So the set can be released
//! (`release_grammars`), which frees all of it; the next highlight loads it again. `release` drops
//! it while the window is in the background, and `warm_up` compiles the most used grammars ahead
//! of the renders, out of their way.

mod release;
mod warm_up;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use super::escape_html;

pub use release::{BackgroundRelease, Grammars, ReleaseAction, ReleasePolicy};
pub use warm_up::{warm_up, warm_up_in_background, StartupWarmUp, WARM_UP_LANGUAGES};

/// The syntax set while it is loaded, and when a highlight last used it.
struct Loaded {
    set: Arc<SyntaxSet>,
    used: Instant,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

/// Renders highlighting right now. The warm-up waits while there are any.
static RENDERS: AtomicUsize = AtomicUsize::new(0);

/// At most this many threads highlight. Every thread that runs a grammar's regexes builds its own
/// matching caches the first time, and keeps them (about 12 MB a thread for the fixture plan's
/// languages). On a 32-thread PC the first code-heavy render after start-up spent longer building
/// caches on every thread than the threads saved; 8 measured fastest for that render, while a
/// warm render stays within its budget.
const MAX_HIGHLIGHT_THREADS: usize = 8;

/// The threads code blocks are highlighted on. `None` if it couldn't start; highlighting then uses
/// rayon's global pool.
static POOL: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    let threads =
        std::thread::available_parallelism().map_or(1, |n| n.get().min(MAX_HIGHLIGHT_THREADS));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("lectern-highlight-{i}"))
        .build()
        .ok()
});

/// Runs `work` on the highlighting threads, where its parallel iterators run too.
pub(crate) fn on_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    match &*POOL {
        Some(pool) => pool.install(work),
        None => work(),
    }
}

#[cfg(test)]
pub(crate) fn pool() -> Option<&'static rayon::ThreadPool> {
    POOL.as_ref()
}

fn loaded() -> MutexGuard<'static, Option<Loaded>> {
    LOADED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The syntax set, loaded if it isn't, marked as used now.
pub(crate) fn syntaxes() -> Arc<SyntaxSet> {
    let mut slot = loaded();
    let loaded = slot.get_or_insert_with(|| Loaded {
        set: Arc::new(two_face::syntax::extra_newlines()),
        used: Instant::now(),
    });
    loaded.used = Instant::now();
    Arc::clone(&loaded.set)
}

/// A syntax set taken out of use, with every compiled grammar and matching cache in it; freed
/// when dropped, unless a highlight under way still holds it.
pub struct ReleasedGrammars(
    #[expect(dead_code, reason = "held only to be freed when dropped")] Arc<SyntaxSet>,
);

/// Takes the syntax set out of use and drops the warm-up still queued. A highlight under way
/// keeps the set it holds until it finishes; the next one loads the set again. `None` when it
/// wasn't loaded.
pub fn take_grammars() -> Option<ReleasedGrammars> {
    warm_up::cancel();
    loaded().take().map(|loaded| ReleasedGrammars(loaded.set))
}

/// `take_grammars`, freeing the set at once. True when it was loaded.
pub fn release_grammars() -> bool {
    take_grammars().is_some()
}

/// When a highlight last used the syntax set; `None` while it isn't loaded.
pub fn grammars_last_used() -> Option<Instant> {
    loaded().as_ref().map(|loaded| loaded.used)
}

/// Counts a render as highlighting until it is dropped.
pub(crate) struct Rendering;

impl Rendering {
    pub(crate) fn begin() -> Self {
        RENDERS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Rendering {
    fn drop(&mut self) {
        RENDERS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a render is highlighting.
pub(crate) fn rendering() -> bool {
    RENDERS.load(Ordering::SeqCst) > 0
}

const BASH: &str = "Bourne Again Shell (bash)";

/// Fence tags, lowercase, and the grammar each one names. Any other tag is looked up as a grammar
/// name, then as a file extension.
const ALIASES: &[(&str, &str)] = &[
    // two-face leaves "JavaScript (Babel)" out of its pure-Rust set; the TSX grammar reads JSX.
    ("jsx", "TypeScriptReact"),
    ("tsx", "TypeScriptReact"),
    ("ts", "TypeScript"),
    ("typescript", "TypeScript"),
    ("js", "JavaScript"),
    ("javascript", "JavaScript"),
    ("sh", BASH),
    ("bash", BASH),
    ("shell", BASH),
    ("zsh", BASH),
    ("vim", "VimL"),
    ("rb", "Ruby"),
    ("yml", "YAML"),
    ("jsonc", "JSON"),
    ("diff", "Diff"),
    ("sql", "SQL"),
    ("scss", "SCSS"),
    ("python", "Python"),
    ("html", "HTML"),
    ("markdown", "Markdown"),
];

/// Highlighting plain text only adds spans, so it is shown escaped instead.
const PLAIN_TEXT: &str = "Plain Text";

/// Larger blocks, and blocks with a longer line, are shown escaped: some grammars take quadratic
/// time over one long token, and on minified code the spans outweigh the code many times over.
const MAX_BLOCK_BYTES: usize = 100 * 1024;
const MAX_LINE_CHARS: usize = 2_000;

/// The name of the grammar a fence tag is highlighted with, or `None` when it is shown as plain
/// text.
pub fn canonical_lang(tag: &str) -> Option<&'static str> {
    // The set comes and goes, so its grammar names are kept apart, once.
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    let set = syntaxes();
    let name = syntax_for(&set, tag)?.name.as_str();
    let names = NAMES.get_or_init(|| {
        set.syntaxes()
            .iter()
            .map(|syntax| &*Box::leak(syntax.name.clone().into_boxed_str()))
            .collect()
    });
    names.iter().copied().find(|known| *known == name)
}

/// The inner HTML of a `<code>` element: spans classed `hl-…` for a known language, else the code
/// escaped. Blocks over the size limits are escaped too.
pub fn highlight_to_html(lang_tag: Option<&str>, code: &str) -> String {
    match lang_tag.filter(|tag| !tag.is_empty()) {
        Some(tag) => highlight_with(&syntaxes(), tag, code),
        None => escape_html(code),
    }
}

/// `highlight_to_html` with the syntax set in hand.
pub(crate) fn highlight_with(set: &SyntaxSet, tag: &str, code: &str) -> String {
    let syntax = match syntax_for(set, tag) {
        Some(syntax) if within_limits(code) => syntax,
        _ => return escape_html(code),
    };
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        set,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    for line in LinesWithEndings::from(code) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return escape_html(code);
        }
    }
    generator.finalize()
}

fn within_limits(code: &str) -> bool {
    code.len() <= MAX_BLOCK_BYTES
        && code
            .lines()
            .all(|line| line.len() <= MAX_LINE_CHARS || line.chars().count() <= MAX_LINE_CHARS)
}

fn syntax_for<'a>(syntaxes: &'a SyntaxSet, tag: &str) -> Option<&'a SyntaxReference> {
    if tag.is_empty() {
        return None;
    }
    let lower = tag.to_ascii_lowercase();
    let syntax = match ALIASES.iter().find(|(alias, _)| *alias == lower) {
        Some((_, name)) => syntaxes.find_syntax_by_name(name),
        None => syntaxes
            .find_syntax_by_name(tag)
            .or_else(|| syntaxes.find_syntax_by_token(&lower)),
    }?;
    (syntax.name != PLAIN_TEXT).then_some(syntax)
}
