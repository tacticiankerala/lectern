//! Reading and writing the sidecar file.
//!
//! The reader is tolerant: a section it can't read as a comment is kept verbatim as
//! [`Item::Raw`], and every comment keeps the text it was read from. So [`serialize`] gives back
//! an unmodified file byte for byte, and regenerates only the comments marked `dirty`.

use std::sync::LazyLock;

use regex::Regex;

use super::{
    sidecar_note, AnchorMeta, ClaudeKind, Comment, CommentStatus, Entry, EntryAuthor, Item, Review,
};

/// A comment's section header. The middle dots and the en dash also accept `-` and `.`.
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^## C(\d+)\s*[·.\-]\s*([A-Za-z]+)\s*[·.\-]\s*L(\d+)(?:\s*[–\-]\s*L?(\d+))?(?:\s*[·.\-]\s*(.*?))?\s*$",
    )
    .expect("the header pattern is valid")
});

/// The line that starts a thread entry, with the colon inside or outside the bold:
/// - `**You:**`, the reader;
/// - `**<Name> (<kind>):**`, an agent: 1 to 32 letters, digits, spaces, `.`, `_` or `-`, starting
///   with a letter, then one of the four kinds. [`entry_head`] turns away the name `You`;
/// - `**<Name>:**` with no kind, an agent's reply, only for the agents named here.
///
/// Names and kinds are matched in any case. Without the kind, bold text such as `**Note:**`
/// would start an entry.
static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\*\*(?:(\p{L}(?:[\p{L}\p{Nd} ._\-]{0,30}[\p{L}\p{Nd}._\-])?)\s*\(((?i:reply|question|pushback|resolved))\)|((?i:you|claude|codex|gemini|copilot|cursor|ai)))(?::\*\*|\*\*:)\s?(.*)$",
    )
    .expect("the entry pattern is valid")
});

/// The entry line v0.2.0 read: also `**You (<word>):**` and `**Claude (<word>):**` for any word.
/// Lines it matches are still escaped, so the backslash v0.2.0 put before them still comes off.
static V020_ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*(?i:you|claude)(?:\s*\([A-Za-z]+\))?(?::\*\*|\*\*:)")
        .expect("the v0.2.0 entry pattern is valid")
});

/// The anchor line under a header.
static ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<!--\s*anchor\s(.*?)-->\s*$").expect("the anchor pattern is valid")
});

/// A line meant as the anchor line, whether or not it can be read.
static ANCHOR_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<!--\s*anchor(?:\s|$)").expect("the anchor-like pattern is valid")
});

/// One `key="json string"` or `key=123` attribute of the anchor line.
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\w+)=("(?:[^"\\]|\\.)*"|\d+)"#).expect("the attribute pattern is valid")
});

/// The instructions comment at the top of every sidecar Lectern creates. No line of it may read as
/// structure, and nothing inside may end the comment early. Lectern owns it: a save puts the
/// current text in place of the block an older version wrote (see [`refresh_instructions`]).
pub const INSTRUCTIONS: &str = "<!-- lectern: review comments on the note, for AI agents and people.
Reply: add a paragraph under the comment starting **Claude (reply):**, with your own name and the kind reply, question, pushback or resolved.
New comment: append \"## C<next number> · open · L<line> · <Heading>\", then \"> exact words from the note\", then **Claude (question):** and your text.
Edit nothing else. The quote outranks line numbers; resolved and dismissed comments need nothing. -->
";

/// An empty sidecar for the note called `note_file_name`.
pub fn new_review(note_file_name: &str) -> Review {
    let preamble = format!(
        "---\nlectern-review: 1\nnote: {}\n---\n# Review: {}\n\n{INSTRUCTIONS}\n",
        note_value(note_file_name),
        one_line(note_file_name),
    );
    Review {
        note: note_file_name.to_owned(),
        preamble,
        items: Vec::new(),
        crlf: false,
        bom: false,
    }
}

/// Puts the current [`INSTRUCTIONS`] in place of the instructions block in the preamble: the first
/// line starting `<!-- lectern:`, through its closing `-->`, the new text taking the file's line
/// ending. The rest of the preamble is kept byte for byte. A preamble without such a block, or
/// whose block is never closed, is left alone, so Lectern refreshes only the block it wrote.
///
/// Only a save calls this: reading a file and writing it back still changes nothing.
pub fn refresh_instructions(review: &mut Review) {
    let mut at = 0;
    let start = review.preamble.split_inclusive('\n').find_map(|line| {
        let here = at;
        at += line.len();
        line.starts_with("<!-- lectern:").then_some(here)
    });
    let Some(start) = start else {
        return;
    };
    let Some(len) = review.preamble[start..].find("-->") else {
        return;
    };
    let end = start + len + "-->".len();
    let block = INSTRUCTIONS.trim_end_matches('\n');
    let block = if review.crlf {
        block.replace('\n', "\r\n")
    } else {
        block.to_owned()
    };
    review.preamble.replace_range(start..end, &block);
}

/// The YAML indicators a plain scalar can't start with.
const YAML_INDICATORS: [char; 19] = [
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
];

/// The note's file name as a frontmatter value: plain when YAML would read it back unchanged,
/// otherwise a JSON string, which YAML reads as a double-quoted one.
fn note_value(name: &str) -> String {
    let plain = !name.is_empty()
        && name.trim() == name
        && !name.contains([':', '#', '\'', '"'])
        && !name.chars().any(char::is_control)
        && !name.starts_with(YAML_INDICATORS);
    if plain {
        name.to_owned()
    } else {
        serde_json::to_string(name).expect("a string always serialises")
    }
}

/// Reads a sidecar. Never fails: whatever can't be read as a comment is kept as it is.
///
/// The preamble, raw sections and each comment's `raw` are the original text, line endings and
/// all, so they're written back byte for byte even when CRLF and LF are mixed. Comments are read
/// from a copy with CRLF turned into LF.
pub fn parse(text: &str) -> Review {
    let bom = text.starts_with('\u{feff}');
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let crlf = text.contains("\r\n");
    let starts = section_starts(text);
    let preamble = text[..starts.first().copied().unwrap_or(text.len())].to_owned();
    let ends = starts.iter().skip(1).copied().chain([text.len()]);
    let items = starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| parse_section(&text[start..end]))
        .collect();
    Review {
        note: sidecar_note(&preamble).unwrap_or_default(),
        preamble,
        items,
        crlf,
        bom,
    }
}

/// Where each section starts: every line beginning `## ` outside fenced code.
fn section_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut fence = Fence::default();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let content = without_line_ending(line);
        if !fence.step(content) && content.starts_with("## ") {
            starts.push(at);
        }
        at += line.len();
    }
    starts
}

/// A line from `split_inclusive('\n')` without its `\n` or `\r\n`.
fn without_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn parse_section(section: &str) -> Item {
    parse_comment(section).map_or_else(|| Item::Raw(section.to_owned()), Item::Comment)
}

fn parse_comment(section: &str) -> Option<Comment> {
    let text = section.replace("\r\n", "\n");
    let (header, body) = text.split_once('\n').unwrap_or((&text, ""));
    let caps = HEADER.captures(header)?;
    let id = caps[1].parse().ok()?;
    let status = CommentStatus::parse(&caps[2])?;
    let start_line = caps[3].parse().ok()?;
    let end_line = match caps.get(4) {
        Some(end) => end.as_str().parse().ok()?,
        None => start_line,
    };
    // Only ` › ` separates headings: ` > ` is common inside real headings (`Latency > 500 ms`).
    // Known limit: a heading that itself holds ` › ` splits in two when read back.
    let heading_path = caps
        .get(5)
        .map(|path| {
            path.as_str()
                .split(" › ")
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let mut lines = body.split('\n').peekable();
    let mut notes = Vec::new();
    skip_blank(&mut lines);
    let anchor = match lines.next_if(|line| ANCHOR_LIKE.is_match(line)) {
        Some(line) => {
            let anchor = parse_anchor(line);
            if anchor.is_none() {
                notes.push(line.to_owned());
            }
            anchor
        }
        None => None,
    };
    skip_blank(&mut lines);
    let mut quote = Vec::new();
    while let Some(line) = lines.next_if(|line| line.starts_with('>')) {
        quote.push(line.strip_prefix("> ").unwrap_or(&line[1..]));
    }
    let entries = parse_thread(lines, &mut notes);

    Some(Comment {
        id,
        status,
        start_line,
        end_line,
        heading_path,
        anchor,
        quote: quote.join("\n"),
        notes,
        entries,
        raw: Some(section.to_owned()),
        dirty: false,
    })
}

fn skip_blank<'a>(lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) {
    while lines.next_if(|line| line.trim().is_empty()).is_some() {}
}

/// The anchor line's attributes, or `None` if the line can't be read in full: anything between
/// the attributes other than whitespace (`prefix = "…"`, a stray word, an unclosed quote) would
/// otherwise drop that attribute silently, and a rewrite would lose it.
fn parse_anchor(line: &str) -> Option<AnchorMeta> {
    let attrs = &ANCHOR.captures(line)?[1];
    let mut meta = AnchorMeta::default();
    let mut read_to = 0;
    for attr in ATTR.captures_iter(attrs) {
        let whole = attr.get(0).expect("group 0 is the whole match");
        if !attrs[read_to..whole.start()].trim().is_empty() {
            return None;
        }
        read_to = whole.end();
        let field = match &attr[1] {
            "prefix" => &mut meta.prefix,
            "suffix" => &mut meta.suffix,
            "fp" => &mut meta.fp,
            "created" => &mut meta.created,
            "n" => {
                meta.n = attr_value(&attr[2])?.parse().ok()?;
                continue;
            }
            _ => continue,
        };
        *field = attr_value(&attr[2])?;
    }
    attrs[read_to..].trim().is_empty().then_some(meta)
}

/// A quoted attribute decoded as JSON, or a bare number as written.
fn attr_value(raw: &str) -> Option<String> {
    if raw.starts_with('"') {
        serde_json::from_str(raw).ok()
    } else {
        Some(raw.to_owned())
    }
}

/// The lines after the quote: paragraphs before the first entry go to `notes`, the rest are
/// entries, each running to the next entry line outside fenced code.
fn parse_thread<'a>(lines: impl Iterator<Item = &'a str>, notes: &mut Vec<String>) -> Vec<Entry> {
    let mut fence = Fence::default();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut entries: Vec<(EntryHead, Vec<&str>)> = Vec::new();
    for line in lines {
        let code = fence.step(line);
        if !code {
            if let Some((head, first)) = entry_head(line) {
                entries.push((head, vec![first]));
                continue;
            }
        }
        let line = if code { line } else { unescape(line) };
        match entries.last_mut() {
            Some((_, text)) => text.push(line),
            None if !code && line.trim().is_empty() => flush(&mut paragraph, notes),
            None => paragraph.push(line),
        }
    }
    flush(&mut paragraph, notes);
    entries
        .into_iter()
        .map(|(head, text)| Entry {
            author: head.author,
            name: head.name.to_owned(),
            kind: head.kind,
            text: trim_blank_lines(&text).join("\n"),
        })
        .collect()
}

/// Who an entry line says wrote the entry.
struct EntryHead<'a> {
    author: EntryAuthor,
    /// `You`, or the agent's name as written.
    name: &'a str,
    kind: Option<ClaudeKind>,
}

/// If `line` starts an entry (see [`ENTRY`]): who wrote it, and the rest of the line.
fn entry_head(line: &str) -> Option<(EntryHead<'_>, &str)> {
    let caps = ENTRY.captures(line)?;
    let is_you = |name: &str| name.eq_ignore_ascii_case("you");
    let head = if let (Some(name), Some(kind)) = (caps.get(1), caps.get(2)) {
        // The reader's entries have no kind.
        if is_you(name.as_str()) {
            return None;
        }
        EntryHead {
            author: EntryAuthor::Agent,
            name: name.as_str(),
            kind: ClaudeKind::parse(kind.as_str()),
        }
    } else {
        let name = caps.get(3)?.as_str();
        EntryHead {
            author: if is_you(name) {
                EntryAuthor::You
            } else {
                EntryAuthor::Agent
            },
            name: if is_you(name) { "You" } else { name },
            kind: None,
        }
    };
    Some((head, caps.get(4).map_or("", |rest| rest.as_str())))
}

fn flush(paragraph: &mut Vec<&str>, notes: &mut Vec<String>) {
    if !paragraph.is_empty() {
        notes.push(paragraph.join("\n"));
        paragraph.clear();
    }
}

fn trim_blank_lines<'a, 'b>(lines: &'b [&'a str]) -> &'b [&'a str] {
    let blank = |line: &&str| line.trim().is_empty();
    let start = lines.iter().position(|l| !blank(l)).unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !blank(l))
        .map_or(start, |i| i + 1);
    &lines[start..end]
}

/// True if a line of comment text would read as structure (a section header or an entry line, or
/// one v0.2.0 escaped), counting one that is already escaped, so it needs a leading backslash.
fn needs_escape(line: &str) -> bool {
    let bare = line.trim_start_matches('\\');
    // Every header starts with `## `, so this covers HEADER too.
    bare.starts_with("## ") || ENTRY.is_match(bare) || V020_ENTRY.is_match(bare)
}

/// Strips the backslash the writer put before a line that would read as structure.
fn unescape(line: &str) -> &str {
    match line.strip_prefix('\\') {
        Some(rest) if needs_escape(rest) => rest,
        _ => line,
    }
}

/// Writes a sidecar: the preamble, raw sections and unchanged comments byte for byte as they were
/// read, and the comments marked `dirty` (or never read) freshly rendered, each set apart by a
/// blank line. Only the rendered text and the blank lines added around it take the file's line
/// ending.
pub fn serialize(review: &Review) -> String {
    let eol = if review.crlf { "\r\n" } else { "\n" };
    let mut out = String::new();
    // Follows the fences in the text written verbatim: one left open would swallow what follows.
    let mut fence = Fence::default();
    push_verbatim(&mut out, &review.preamble, &mut fence);
    let mut after_rendered = false;
    for item in &review.items {
        let verbatim = match item {
            Item::Raw(section) => Ok(section),
            Item::Comment(c) => c.raw.as_ref().filter(|_| !c.dirty).ok_or(c),
        };
        match verbatim {
            Ok(text) => {
                if after_rendered {
                    end_with_blank_line(&mut out, eol);
                }
                push_verbatim(&mut out, text, &mut fence);
                after_rendered = false;
            }
            Err(c) => {
                fence.close(&mut out, eol);
                end_with_blank_line(&mut out, eol);
                let mut section = String::new();
                render_comment(c, &mut section);
                // Comment text may hold CRLF of its own; the file's line ending goes on afterwards.
                section = section.replace("\r\n", "\n");
                if review.crlf {
                    section = section.replace('\n', "\r\n");
                }
                out.push_str(&section);
                after_rendered = true;
            }
        }
    }
    if review.bom {
        out.insert(0, '\u{feff}');
    }
    out
}

fn push_verbatim(out: &mut String, text: &str, fence: &mut Fence) {
    for line in text.split_inclusive('\n') {
        fence.step(without_line_ending(line));
    }
    out.push_str(text);
}

/// Adds line endings until `out` ends in a blank line, whichever endings it already has.
fn end_with_blank_line(out: &mut String, eol: &str) {
    if out.is_empty() {
        return;
    }
    while !(out.ends_with("\n\n") || out.ends_with("\n\r\n")) {
        out.push_str(eol);
    }
}

/// A comment's section: header, anchor line, quote, notes and entries, ending in one newline.
fn render_comment(c: &Comment, out: &mut String) {
    let mut head = format!("## C{} · {} · L{}", c.id, c.status.as_str(), c.start_line);
    if c.end_line != c.start_line {
        head.push_str(&format!("–L{}", c.end_line));
    }
    let path: Vec<String> = c
        .heading_path
        .iter()
        .map(|heading| one_line(heading))
        .filter(|heading| !heading.is_empty())
        .collect();
    if !path.is_empty() {
        head.push_str(" · ");
        head.push_str(&path.join(" › "));
    }
    head.push('\n');
    if let Some(a) = &c.anchor {
        head.push_str(&format!(
            "<!-- anchor prefix={} suffix={} fp={} n={} created={} -->\n",
            attr(&a.prefix),
            attr(&a.suffix),
            attr(&a.fp),
            a.n,
            attr(&a.created),
        ));
    }
    let quote: Vec<String> = c
        .quote
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect();
    head.push_str(&quote.join("\n"));

    let mut blocks = vec![head];
    blocks.extend(c.notes.iter().map(|note| render_note(note)));
    blocks.extend(c.entries.iter().map(render_entry));
    out.push_str(&blocks.join("\n\n"));
    out.push('\n');
}

fn render_note(note: &str) -> String {
    let mut out = String::new();
    let mut fence = Fence::default();
    for (i, line) in note.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_escaped(&mut out, line, &mut fence);
    }
    fence.close(&mut out, "\n");
    out
}

/// An entry: its marker, then the text. The first line shares the marker's line, where it can't
/// read as structure, unless it opens a fence, which only counts at the start of a line.
fn render_entry(e: &Entry) -> String {
    let mut out = match (e.author, e.kind) {
        (EntryAuthor::You, _) => "**You:**".to_owned(),
        (EntryAuthor::Agent, None) => format!("**{}:**", e.name),
        (EntryAuthor::Agent, Some(kind)) => format!("**{} ({}):**", e.name, kind.as_str()),
    };
    let mut fence = Fence::default();
    let mut lines = e.text.split('\n');
    let first = lines.next().unwrap_or_default();
    if fence_opening(first).is_some() {
        out.push('\n');
        push_escaped(&mut out, first, &mut fence);
    } else if !first.is_empty() {
        out.push(' ');
        out.push_str(first);
    }
    for line in lines {
        out.push('\n');
        push_escaped(&mut out, line, &mut fence);
    }
    fence.close(&mut out, "\n");
    out
}

fn push_escaped(out: &mut String, line: &str, fence: &mut Fence) {
    if !fence.step(line) && needs_escape(line) {
        out.push('\\');
    }
    out.push_str(line);
}

/// A JSON string literal that can't end the HTML comment it sits in: `<` and `>` are escaped, and
/// so is every `-` right after another, so `--` never appears.
fn attr(value: &str) -> String {
    let json = serde_json::to_string(value).expect("a string always serialises");
    let mut out = String::with_capacity(json.len());
    let mut prev = '\0';
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '-' if prev == '-' => out.push_str("\\u002d"),
            _ => out.push(ch),
        }
        prev = ch;
    }
    out
}

/// Text for a single line: control characters (line breaks included) become spaces.
fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().to_owned()
}

/// Tracks fenced code blocks line by line, so nothing inside code is read or escaped as structure.
#[derive(Default)]
struct Fence {
    /// The fence character and how many of it opened the block.
    open: Option<(u8, usize)>,
}

impl Fence {
    /// Feeds the next line, without its line ending. True if it's part of a fenced block,
    /// delimiters included.
    fn step(&mut self, line: &str) -> bool {
        match self.open {
            Some((ch, len)) => {
                if fence_closes(line, ch, len) {
                    self.open = None;
                }
                true
            }
            None => {
                self.open = fence_opening(line);
                self.open.is_some()
            }
        }
    }

    /// Closes a block left open at the end of `out`, so it can't swallow what follows. The closing
    /// fence starts a new line if `out` doesn't end in one.
    fn close(&mut self, out: &mut String, eol: &str) {
        if let Some((ch, len)) = self.open.take() {
            if !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.extend(std::iter::repeat_n(char::from(ch), len));
        }
    }
}

/// The fence character and length if `line` opens a fenced code block: up to three spaces, then
/// three or more backticks or tildes (and no backtick in a backtick fence's info string).
fn fence_opening(line: &str) -> Option<(u8, usize)> {
    let rest = strip_indent(line)?;
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b == ch).count();
    let info = &rest[len..];
    (len >= 3 && !(ch == b'`' && info.contains('`'))).then_some((ch, len))
}

/// True if `line` closes a block opened by `len` of `ch`: at least as many, then only spaces.
fn fence_closes(line: &str, ch: u8, len: usize) -> bool {
    let Some(rest) = strip_indent(line) else {
        return false;
    };
    let run = rest.bytes().take_while(|&b| b == ch).count();
    run >= len && rest[run..].trim().is_empty()
}

/// The line without up to three leading spaces, or `None` if it's indented further.
fn strip_indent(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    (line.len() - rest.len() <= 3).then_some(rest)
}
