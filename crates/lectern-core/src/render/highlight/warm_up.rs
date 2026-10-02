//! Compiling the grammars of the languages the vault uses most before a render needs them, at low
//! priority: on one thread of its own, a line at a time, waiting whenever a render is
//! highlighting. A render compiles what it needs anyway, so the warm-up never has to win a race.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::util::LinesWithEndings;

use super::{rendering, syntax_for, syntaxes};

/// The fence tags warmed, in order: the languages the vault uses most. `jsx` shares the `tsx`
/// grammar. Every other grammar (TypeScript alone is about 45 MB compiled) compiles when a
/// document first uses it.
pub const WARM_UP_LANGUAGES: &[&str] = &["ruby", "tsx", "bash", "vim", "js", "json", "yaml"];

/// A short, typical snippet in each warmed language, reaching the grammar states a real block
/// does.
const SAMPLES: &[(&str, &str)] = &[
    (
        "ruby",
        concat!(
            "# note\n",
            "RSpec.describe Report do\n",
            "  let(:record) { described_class.new(name: \"a-#{1}\", tags: %w[x y]) }\n",
            "  it \"keeps its name\" do\n",
            "    expect(record.name).to eq(:a) if record&.valid?\n",
            "  end\n",
            "end\n",
        ),
    ),
    (
        "tsx",
        concat!(
            "import { useState } from \"react\";\n",
            "// note\n",
            "export function Panel({ items, onSelect }: Props) {\n",
            "  const [open, setOpen] = useState<boolean>(false);\n",
            "  if (items.length === 0) {\n",
            "    return <p className=\"empty\">None yet.</p>;\n",
            "  }\n",
            "  return (\n",
            "    <ul>\n",
            "      {items.map((item) => (\n",
            "        <li key={item.id} onClick={() => onSelect(item)}>{item.label}</li>\n",
            "      ))}\n",
            "    </ul>\n",
            "  );\n",
            "}\n",
        ),
    ),
    (
        "bash",
        concat!(
            "# note\n",
            "export A=\"${HOME}/b\"\n",
            "for f in *.md; do echo \"$f\" | grep -q x && ls -la \"$f\"; done\n",
            "bundle exec rspec spec/a_spec.rb --format documentation\n",
        ),
    ),
    (
        "vim",
        concat!(
            "\" note\n",
            "nnoremap <leader>a :call A()<CR>\n",
            "function! A() abort\n",
            "  let l:x = expand('%:t:r')\n",
            "  if l:x =~# '_spec$' | echo \"y\" | endif\n",
            "endfunction\n",
        ),
    ),
    (
        "js",
        concat!(
            "// note\n",
            "const { a } = require(\"b\");\n",
            "export const c = async (d) => {\n",
            "  for (const e of d) { if (e > 1) return `${e}`; }\n",
            "};\n",
        ),
    ),
    ("json", "{\"a\": [1, true, null], \"b\": {\"c\": \"d\"}}\n"),
    ("yaml", "# note\na: 1\nb:\n  - \"c\"\n  - d: [e, f]\n"),
];

/// For a language without a sample, such as one the open document uses: enough to compile the
/// grammar's main context.
const GENERIC_SAMPLE: &str = "a = b(1, \"c\", [d]) # e\n";

/// How often a warm-up waiting for a render looks again.
const RENDER_POLL: Duration = Duration::from_millis(2);

/// Warms `WARM_UP_LANGUAGES` on the calling thread and returns when they are compiled.
pub fn warm_up() {
    for tag in WARM_UP_LANGUAGES {
        warm_language(tag);
    }
}

/// Queues `first` (the open document's languages, say), then `WARM_UP_LANGUAGES`, for the
/// warm-up thread, and returns at once.
pub fn warm_up_in_background(first: &[String]) {
    let tags = first
        .iter()
        .cloned()
        .chain(WARM_UP_LANGUAGES.iter().map(|tag| (*tag).to_owned()));
    queue().push(tags);
}

/// Drops the warm-up still queued; the language being warmed finishes.
pub(super) fn cancel() {
    if let Some(queue) = QUEUE.get() {
        queue.cancel();
    }
}

static QUEUE: OnceLock<WarmUpQueue> = OnceLock::new();

fn queue() -> &'static WarmUpQueue {
    QUEUE.get_or_init(|| WarmUpQueue::spawn(warm_language))
}

/// Compiles the grammar `tag` names by highlighting a sample, yielding to renders between lines.
fn warm_language(tag: &str) {
    let set = syntaxes();
    let Some(syntax) = syntax_for(&set, tag) else {
        return;
    };
    let code = SAMPLES
        .iter()
        .find(|(sample_tag, _)| sample_tag.eq_ignore_ascii_case(tag))
        .map_or(GENERIC_SAMPLE, |(_, code)| code);
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &set,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    for line in LinesWithEndings::from(code) {
        while rendering() {
            thread::sleep(RENDER_POLL);
        }
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return;
        }
    }
    generator.finalize();
}

/// Fence tags waiting to be warmed, one at a time, in order, on a thread of their own.
pub(super) struct WarmUpQueue {
    shared: Arc<Shared>,
}

struct Shared {
    tags: Mutex<VecDeque<String>>,
    wake: Condvar,
}

impl Shared {
    fn tags(&self) -> MutexGuard<'_, VecDeque<String>> {
        self.tags.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl WarmUpQueue {
    /// A queue whose thread warms each tag with `warm`.
    pub(super) fn spawn(warm: impl Fn(&str) + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            tags: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("lectern-warm-up".to_owned())
            .spawn(move || loop {
                let tag = {
                    let mut tags = worker
                        .wake
                        .wait_while(worker.tags(), |tags| tags.is_empty())
                        .unwrap_or_else(PoisonError::into_inner);
                    tags.pop_front()
                };
                if let Some(tag) = tag {
                    warm(&tag);
                }
            });
        if let Err(e) = spawned {
            log::warn!("couldn't start the highlighter's warm-up: {e}");
        }
        Self { shared }
    }

    /// Adds the tags not already waiting, in order.
    pub(super) fn push(&self, new: impl IntoIterator<Item = String>) {
        let mut tags = self.shared.tags();
        for tag in new {
            if !tags.iter().any(|queued| queued.eq_ignore_ascii_case(&tag)) {
                tags.push_back(tag);
            }
        }
        self.shared.wake.notify_one();
    }

    pub(super) fn cancel(&self) {
        self.shared.tags().clear();
    }
}

/// Starts the warm-up once start-up is done with the processor: when the document rendered at
/// boot has rendered, or, when boot had none, at the first paint. Once.
pub struct StartupWarmUp {
    progress: Mutex<Startup>,
    start: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Startup {
    /// Set when boot is done: whether it rendered a document.
    boot: Option<bool>,
    painted: bool,
    started: bool,
}

impl Default for StartupWarmUp {
    /// Starts `warm_up_in_background`.
    fn default() -> Self {
        Self::new(|| warm_up_in_background(&[]))
    }
}

impl StartupWarmUp {
    /// A trigger that runs `start` when the warm-up is due.
    pub fn new(start: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            progress: Mutex::new(Startup::default()),
            start: Box::new(start),
        }
    }

    /// Boot is done, having rendered a document or not.
    pub fn boot_finished(&self, rendered: bool) {
        self.update(|startup| startup.boot = Some(rendered));
    }

    /// The window painted its first document (or the welcome screen).
    pub fn first_paint(&self) {
        self.update(|startup| startup.painted = true);
    }

    fn update(&self, change: impl FnOnce(&mut Startup)) {
        let due = {
            let mut startup = self.progress.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut startup);
            let due = !startup.started
                && startup
                    .boot
                    .is_some_and(|rendered| rendered || startup.painted);
            startup.started |= due;
            due
        };
        if due {
            (self.start)();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    use super::super::Rendering;
    use super::*;

    #[test]
    fn the_warm_up_list_is_the_vaults_most_used_languages() {
        assert_eq!(
            WARM_UP_LANGUAGES,
            ["ruby", "tsx", "bash", "vim", "js", "json", "yaml"]
        );
        let sampled: Vec<&str> = SAMPLES.iter().map(|(tag, _)| *tag).collect();
        assert_eq!(sampled, WARM_UP_LANGUAGES);
    }

    fn counting_trigger() -> (StartupWarmUp, Arc<AtomicUsize>) {
        let started = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&started);
        let trigger = StartupWarmUp::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        (trigger, started)
    }

    #[test]
    fn the_warm_up_waits_for_the_boot_render_and_starts_once() {
        let (trigger, started) = counting_trigger();
        trigger.first_paint();
        assert_eq!(started.load(Ordering::SeqCst), 0, "not while boot renders");
        trigger.boot_finished(true);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        trigger.first_paint();
        trigger.boot_finished(true);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn after_a_boot_render_the_warm_up_does_not_wait_for_the_paint() {
        let (trigger, started) = counting_trigger();
        trigger.boot_finished(true);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn without_a_boot_document_the_warm_up_waits_for_the_first_paint() {
        let (trigger, started) = counting_trigger();
        trigger.boot_finished(false);
        assert_eq!(started.load(Ordering::SeqCst), 0);
        trigger.first_paint();
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_queue_warms_each_tag_once_in_order() {
        let (tx, rx) = mpsc::channel();
        let queue = WarmUpQueue::spawn(move |tag| tx.send(tag.to_owned()).unwrap());
        queue.push(["ruby".to_owned(), "tsx".to_owned(), "Ruby".to_owned()]);
        let warmed: Vec<String> = (0..2)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(warmed, ["ruby", "tsx"]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn a_cancelled_queue_warms_nothing_more() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (tx, rx) = mpsc::channel();
        let held = Arc::clone(&gate);
        let queue = WarmUpQueue::spawn(move |tag| {
            let (open, cv) = &*held;
            drop(cv.wait_while(open.lock().unwrap(), |open| !*open).unwrap());
            tx.send(tag.to_owned()).unwrap();
        });
        queue.push(["ruby".to_owned(), "tsx".to_owned(), "bash".to_owned()]);
        // The worker holds "ruby" at the gate; the rest is still queued.
        thread::sleep(Duration::from_millis(100));
        queue.cancel();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "ruby");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn warming_waits_while_a_render_highlights() {
        let render = Rendering::begin();
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let warming = thread::spawn(move || {
            warm_language("ruby");
            finished.store(true, Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(300));
        assert!(
            !done.load(Ordering::SeqCst),
            "warmed while a render highlighted"
        );
        drop(render);
        let started = Instant::now();
        warming.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
