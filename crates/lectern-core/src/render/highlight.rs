//! Syntax highlighting with syntect and two-face's grammars, using the pure-Rust regex engine.

use std::sync::LazyLock;

use rayon::prelude::*;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use super::escape_html;

/// Loaded on first use, or ahead of time by `warm_up`. Each grammar's regexes compile lazily the
/// first time a block in that language is highlighted.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

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

/// A short, typical snippet in each language the vault uses most, reaching the grammar states a
/// real block does. `jsx` shares the `tsx` grammar.
const WARM_UP_SAMPLES: &[(&str, &str)] = &[
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
        "ts",
        concat!(
            "import type { A } from \"./a\";\n",
            "// note\n",
            "export interface B<T> { c?: T; readonly d: string[] }\n",
            "export async function e(f: number): Promise<string | null> {\n",
            "  const g = await fetch(`/x/${f}`);\n",
            "  return g.ok ? \"y\" : null;\n",
            "}\n",
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
    ("json", "{\"a\": [1, true, null], \"b\": {\"c\": \"d\"}}\n"),
    ("yaml", "# note\na: 1\nb:\n  - \"c\"\n  - d: [e, f]\n"),
    (
        "scss",
        "// note\n$a: 1px;\n.b { &:hover { color: darken($c, 10%); } }\n",
    ),
    (
        "sql",
        "-- note\nSELECT a, COUNT(*) FROM b WHERE c = 'd' GROUP BY a;\n",
    ),
    ("diff", "--- a\n+++ b\n@@ -1,2 +1,2 @@\n-x\n+y\n z\n"),
    (
        "python",
        "# note\nimport os\n\ndef a(b: int) -> str:\n    return f\"{b}\" if b else os.sep\n",
    ),
    (
        "html",
        "<!-- note -->\n<div class=\"a\"><script>let b = 1;</script><a href=\"c\">d</a></div>\n",
    ),
    (
        "markdown",
        "# A\n\n- b `c` **d** [e](f)\n\n> g\n\n```sh\nh\n```\n",
    ),
];

/// Loads the syntax set and compiles the grammars of the most used languages, so the first render
/// doesn't pay for either. Call it on a background thread at app start.
pub fn warm_up() {
    LazyLock::force(&SYNTAXES);
    WARM_UP_SAMPLES.par_iter().for_each(|(tag, code)| {
        highlight_to_html(Some(tag), code);
    });
}

/// The name of the grammar a fence tag is highlighted with, or `None` when it is shown as plain
/// text.
pub fn canonical_lang(tag: &str) -> Option<&'static str> {
    syntax_for(tag).map(|syntax| syntax.name.as_str())
}

/// The inner HTML of a `<code>` element: spans classed `hl-…` for a known language, else the code
/// escaped. Blocks over the size limits are escaped too.
pub fn highlight_to_html(lang_tag: Option<&str>, code: &str) -> String {
    let syntax = match lang_tag.and_then(syntax_for) {
        Some(syntax) if within_limits(code) => syntax,
        _ => return escape_html(code),
    };
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &SYNTAXES,
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

fn syntax_for(tag: &str) -> Option<&'static SyntaxReference> {
    if tag.is_empty() {
        return None;
    }
    let syntaxes: &'static SyntaxSet = &SYNTAXES;
    let lower = tag.to_ascii_lowercase();
    let syntax = match ALIASES.iter().find(|(alias, _)| *alias == lower) {
        Some((_, name)) => syntaxes.find_syntax_by_name(name),
        None => syntaxes
            .find_syntax_by_name(tag)
            .or_else(|| syntaxes.find_syntax_by_token(&lower)),
    }?;
    (syntax.name != PLAIN_TEXT).then_some(syntax)
}
