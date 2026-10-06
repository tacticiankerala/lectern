//! Anchoring a comment to its quote, and finding the quote again after the note changes.
//!
//! A comment stays where it was while the note is unchanged. Otherwise its quote is looked for in
//! the note's visible text, the context around it settling a quote found more than once. A quote
//! that's gone is looked for word by word, and a passage holding most of its words is taken as
//! the quote reworded. Failing that the comment is detached, its quote and thread kept.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::ops::NewAnchor;
use super::text::{normalize, TextMap};
use super::{AnchorMeta, Comment, Item, Review, CONTEXT_CHARS, QUOTE_CAP};

/// The share of a quote's words a passage must hold to be taken as the quote reworded.
const FUZZY_THRESHOLD_TWENTIETHS: usize = 12; // 0.6
/// Added to a passage's score when it sits under the comment's heading path.
const HEADING_BONUS_TWENTIETHS: usize = 1; // 0.05
/// A quote of fewer words than this is never looked for word by word.
const MIN_FUZZY_TOKENS: usize = 3;

/// How a comment's quote was found in the note.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AnchorState {
    /// The quote is where the comment says, or was found word for word.
    Anchored,
    /// A passage holding most of the quote's words was found: the quote was reworded.
    Moved,
    /// The quote is gone.
    Detached,
}

/// Where a comment's quote is in the note now.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub id: u32,
    pub state: AnchorState,
    /// The quote's lines now when it was found (for a capped quote, the stored range moved to
    /// where the excerpt starts now); the stored lines when it's detached.
    pub start_line: u32,
    pub end_line: u32,
    /// The quote's byte range in [`TextMap::text`] when it was looked for and found.
    pub span: Option<(usize, usize)>,
    /// The passage the quote became, when it moved.
    pub current_text: Option<String>,
    /// When detached: the deepest heading of the comment's path still in the note, and its line.
    pub pinned_heading: Option<(String, u32)>,
}

/// A quote as stored: `normalized`, cut to [`QUOTE_CAP`] characters and an ellipsis when longer.
pub fn cap_quote(normalized: &str) -> String {
    match normalized.char_indices().nth(QUOTE_CAP) {
        Some((cut, _)) => format!("{}…", &normalized[..cut]),
        None => normalized.to_owned(),
    }
}

/// The part of a stored quote that is the note's text: without the ellipsis [`cap_quote`] added.
pub fn match_part(quote: &str) -> &str {
    match quote.strip_suffix('…') {
        Some(head) if head.chars().count() == QUOTE_CAP => head,
        _ => quote,
    }
}

/// What a quote is looked for as: its [`match_part`], normalised.
fn needle(quote: &str) -> String {
    normalize(match_part(quote))
}

/// True if `quote` was cut by [`cap_quote`]: it's only the start of the passage commented on.
fn is_capped(quote: &str) -> bool {
    match_part(quote).len() != quote.len()
}

/// The lines of a comment whose quote was found word for word at `span`. A capped quote is only
/// the start of the passage, so the stored range moves with it, keeping its length, rather than
/// shrinking to the excerpt.
fn anchored_lines(c: &Comment, text: &TextMap, span: (usize, usize)) -> (u32, u32) {
    let (start, end) = text.lines_of(span.0, span.1);
    if !is_capped(&c.quote) {
        return (start, end);
    }
    let length = c.end_line.saturating_sub(c.start_line);
    (start, end.max(start.saturating_add(length)))
}

/// What anchoring a comment produces, for the operation to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorParts {
    pub start_line: u32,
    pub end_line: u32,
    pub quote: String,
    pub heading_path: Vec<String>,
    pub meta: AnchorMeta,
}

/// Anchors a new comment, or a reattached one, to the text the UI sent.
///
/// The whole selection is found in the note's text, preferring an occurrence within the given
/// lines, and gives the lines, prefix and suffix; only its first [`QUOTE_CAP`] characters are
/// stored as the quote. A selection that isn't there whole is looked for by that excerpt. If
/// neither is there (the UI and core disagree, or the note changed under the reader), the given
/// lines are kept with an empty fingerprint, so they are never trusted.
pub fn make_anchor(text: &TextMap, a: &NewAnchor, fp: &str, now: &str) -> AnchorParts {
    let selection = normalize(&a.quote);
    let quote = cap_quote(&selection);
    let excerpt = needle(&quote);
    let (start, end) = (a.start_line, a.end_line.max(a.start_line));
    let found = occurrence_near(text, &selection, start, end).or_else(|| {
        (excerpt != selection)
            .then(|| occurrence_near(text, &excerpt, start, end))
            .flatten()
    });
    let meta = |prefix, suffix, fp: &str| AnchorMeta {
        prefix,
        suffix,
        fp: fp.to_owned(),
        n: 0,
        created: now.to_owned(),
    };
    match found {
        Some((s, e)) => {
            let (start_line, end_line) = text.lines_of(s, e);
            AnchorParts {
                start_line,
                end_line,
                quote,
                heading_path: text.heading_path_at(start_line),
                meta: meta(
                    text.before(s, CONTEXT_CHARS),
                    text.after(e, CONTEXT_CHARS),
                    fp,
                ),
            }
        }
        None => AnchorParts {
            start_line: start,
            end_line: end,
            quote,
            heading_path: text.heading_path_at(start),
            meta: meta(String::new(), String::new(), ""),
        },
    }
}

/// The occurrence of `needle` to anchor on: one whose lines lie within `start..=end`, else one
/// overlapping them, else any; of those, the one starting nearest `start`, then the earliest.
fn occurrence_near(text: &TextMap, needle: &str, start: u32, end: u32) -> Option<(usize, usize)> {
    occurrences(text.text(), needle)
        .map(|s| {
            let span = (s, s + needle.len());
            let (first, last) = text.lines_of(span.0, span.1);
            let fit = if first >= start && last <= end {
                0
            } else if first <= end && last >= start {
                1
            } else {
                2
            };
            ((fit, first.abs_diff(start)), span)
        })
        .min_by_key(|&(key, _)| key)
        .map(|(_, span)| span)
}

/// The start of every occurrence of `needle` in `haystack`; none for an empty needle.
///
/// For a needle of at most [`QUOTE_CAP`] characters, overlapping occurrences are included (`a a`
/// is twice in `a a a`). A longer one is matched end to end: visiting the overlapping
/// occurrences costs the needle's length each, and a needle that long which overlaps itself is
/// repeated text, in which one occurrence is as good as another.
fn occurrences<'h>(haystack: &'h str, needle: &'h str) -> impl Iterator<Item = usize> + 'h {
    let overlapping = needle.chars().nth(QUOTE_CAP).is_none();
    let mut from = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        let at = from + haystack.get(from..)?.find(needle)?;
        from = if overlapping {
            // The next search starts one character on, not after the match.
            at + haystack[at..].chars().next().map_or(1, char::len_utf8)
        } else {
            at + needle.len()
        };
        Some(at)
    })
}

/// Finds every comment's quote in the note: one result per comment, in file order.
pub fn resolve_all(review: &Review, text: &TextMap, fp: &str) -> Vec<Resolved> {
    let mut fuzzy = None;
    review
        .comments()
        .map(|c| resolve(c, text, fp, &mut fuzzy))
        .collect()
}

fn resolve<'t>(
    c: &Comment,
    text: &'t TextMap,
    fp: &str,
    fuzzy: &mut Option<Fuzzy<'t>>,
) -> Resolved {
    let at = |state, (start_line, end_line), span| Resolved {
        id: c.id,
        state,
        start_line,
        end_line,
        span,
        current_text: None,
        pinned_heading: None,
    };
    let stored = (c.start_line, c.end_line);
    let meta = c.anchor.as_ref();
    if meta.is_some_and(|a| !a.fp.is_empty() && a.fp == fp) {
        return at(AnchorState::Anchored, stored, None);
    }

    let needle = needle(&c.quote);
    // A capped quote's suffix follows the whole passage, not the excerpt looked for, so only
    // the prefix can tell its occurrences apart.
    let (prefix, suffix) = match meta {
        Some(a) if is_capped(&c.quote) => (a.prefix.as_str(), ""),
        Some(a) => (a.prefix.as_str(), a.suffix.as_str()),
        None => ("", ""),
    };
    if let Some((s, e)) = best_by_context(text, &needle, prefix, suffix, c.start_line) {
        return at(
            AnchorState::Anchored,
            anchored_lines(c, text, (s, e)),
            Some((s, e)),
        );
    }

    let words = tokens(&needle);
    if words.len() >= MIN_FUZZY_TOKENS {
        let fuzzy = fuzzy.get_or_insert_with(|| Fuzzy::new(text));
        if let Some((s, e)) = fuzzy.find(&words, c) {
            let mut moved = at(AnchorState::Moved, text.lines_of(s, e), Some((s, e)));
            moved.current_text = Some(text.text()[s..e].to_owned());
            return moved;
        }
    }

    let mut detached = at(AnchorState::Detached, stored, None);
    detached.pinned_heading = pinned_heading(c, text);
    detached
}

/// The occurrence of `needle` whose surroundings agree most with the stored `prefix` and
/// `suffix`; on a tie the one starting nearest `line`, then the earliest.
fn best_by_context(
    text: &TextMap,
    needle: &str,
    prefix: &str,
    suffix: &str,
    line: u32,
) -> Option<(usize, usize)> {
    let t = text.text();
    // (score, distance to `line` once worked out, span)
    let mut best: Option<(usize, Option<u32>, (usize, usize))> = None;
    for s in occurrences(t, needle) {
        let e = s + needle.len();
        let score = common_suffix(&t[..s], prefix) + common_prefix(&t[e..], suffix);
        let distance = |span: (usize, usize)| text.lines_of(span.0, span.1).0.abs_diff(line);
        best = match best {
            Some((top, _, _)) if score < top => best,
            Some((top, top_distance, top_span)) if score == top => {
                let top_distance = top_distance.unwrap_or_else(|| distance(top_span));
                let here = distance((s, e));
                if here < top_distance {
                    Some((score, Some(here), (s, e)))
                } else {
                    Some((top, Some(top_distance), top_span))
                }
            }
            _ => Some((score, None, (s, e))),
        };
    }
    best.map(|(_, _, span)| span)
}

/// How many characters `a` and `b` share at their ends.
fn common_suffix(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// How many characters `a` and `b` share at their starts.
fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// The word tokens of `s`: maximal runs of alphanumeric characters, lowercased.
fn tokens(s: &str) -> Vec<String> {
    token_spans(s)
        .map(|(start, end)| {
            let mut word = String::new();
            push_lowercase(&mut word, &s[start..end]);
            word
        })
        .collect()
}

/// Appends a token lowercased, the same way for the quote and the note: as a whole word, so a
/// final capital sigma becomes `ς` as it's written in lowercase text.
fn push_lowercase(out: &mut String, word: &str) {
    if word.is_ascii() {
        out.extend(word.bytes().map(|b| char::from(b.to_ascii_lowercase())));
    } else {
        out.push_str(&word.to_lowercase());
    }
}

/// The byte ranges of the word tokens of `s`.
fn token_spans(s: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut chars = s.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, _) = chars.find(|(_, c)| c.is_alphanumeric())?;
        let mut end = s.len();
        while let Some(&(i, c)) = chars.peek() {
            if !c.is_alphanumeric() {
                end = i;
                break;
            }
            chars.next();
        }
        Some((start, end))
    })
}

/// The note's word tokens, built once per [`resolve_all`] and only when a quote needs looking for
/// word by word.
struct Fuzzy<'t> {
    text: &'t TextMap,
    vocab: HashMap<String, u32>,
    /// Per token: its id in `vocab`, its byte range in the text, and its block.
    ids: Vec<u32>,
    spans: Vec<(usize, usize)>,
    blocks: Vec<usize>,
    /// The heading path of each heading, by index.
    heading_paths: Vec<Vec<String>>,
    /// Counts by token id, kept all zero between searches: the quote's and the window's.
    need: Vec<u16>,
    have: Vec<u16>,
    /// Per heading: whether it has the comment's heading path. Reused across comments.
    same_path: Vec<bool>,
}

impl<'t> Fuzzy<'t> {
    fn new(text: &'t TextMap) -> Self {
        let mut vocab: HashMap<String, u32> = HashMap::new();
        let mut ids = Vec::new();
        let mut spans = Vec::new();
        let mut blocks = Vec::new();
        let mut word = String::new();
        let all = text.text();
        for (b, block) in text.blocks().iter().enumerate() {
            for (s, e) in token_spans(text.block_text(block)) {
                let (s, e) = (block.start + s, block.start + e);
                word.clear();
                push_lowercase(&mut word, &all[s..e]);
                let next = u32::try_from(vocab.len()).unwrap_or(u32::MAX);
                let id = match vocab.get(word.as_str()) {
                    Some(&id) => id,
                    None => *vocab.entry(word.clone()).or_insert(next),
                };
                ids.push(id);
                spans.push((s, e));
                blocks.push(b);
            }
        }
        Self {
            text,
            need: vec![0; vocab.len()],
            have: vec![0; vocab.len()],
            vocab,
            ids,
            spans,
            blocks,
            heading_paths: text.heading_paths(),
            same_path: vec![false; text.headings().len()],
        }
    }

    /// The span of the run of tokens, as many as the quote has, that holds the largest share of
    /// the quote's `words`, if that share (plus the heading bonus) reaches the threshold. On a tie
    /// the run starting nearest the comment's stored line wins, then the earliest.
    fn find(&mut self, words: &[String], c: &Comment) -> Option<(usize, usize)> {
        let m = words.len();
        let n = self.ids.len();
        // The counts are u16: a quote can't have more words than that.
        if n == 0 || m > usize::from(u16::MAX) {
            return None;
        }
        let mut touched = Vec::with_capacity(m);
        for word in words {
            if let Some(&id) = self.vocab.get(word.as_str()) {
                self.need[id as usize] += 1;
                touched.push(id);
            }
        }
        for (same, path) in self.same_path.iter_mut().zip(&self.heading_paths) {
            *same = *path == c.heading_path;
        }
        let top_level = c.heading_path.is_empty();

        let w = m.min(n);
        let (ids, have, need) = (&self.ids, &mut self.have, &self.need);
        let mut overlap = 0usize;
        for &id in &ids[..w] {
            overlap += usize::from(add(have, need, id));
        }
        // Scores are in twentieths of a word, so the threshold and the bonus compare exactly.
        let mut best: Option<(usize, u32, usize)> = None; // (score, distance, first token)
        for first in 0..=n - w {
            if first > 0 {
                overlap -= usize::from(remove(have, need, ids[first - 1]));
                overlap += usize::from(add(have, need, ids[first + w - 1]));
            }
            let block = &self.text.blocks()[self.blocks[first]];
            let under_path = match block.heading {
                Some(h) => self.same_path[h],
                None => top_level,
            };
            let bonus = usize::from(under_path) * HEADING_BONUS_TWENTIETHS * m;
            let score = 20 * overlap + bonus;
            if score < FUZZY_THRESHOLD_TWENTIETHS * m {
                continue;
            }
            let distance = block.start_line.abs_diff(c.start_line);
            let better = match best {
                None => true,
                Some((top, top_distance, _)) => {
                    score > top || (score == top && distance < top_distance)
                }
            };
            if better {
                best = Some((score, distance, first));
            }
        }

        for &id in &ids[n - w..] {
            have[id as usize] = 0;
        }
        for &id in &touched {
            self.need[id as usize] = 0;
        }
        best.map(|(_, _, first)| (self.spans[first].0, self.spans[first + w - 1].1))
    }
}

/// Adds a token to the window's counts; true if it matched one of the quote's.
fn add(have: &mut [u16], need: &[u16], id: u32) -> bool {
    let id = id as usize;
    have[id] += 1;
    have[id] <= need[id]
}

/// Takes a token out of the window's counts; true if it had matched one of the quote's.
fn remove(have: &mut [u16], need: &[u16], id: u32) -> bool {
    let id = id as usize;
    let matched = have[id] <= need[id];
    have[id] -= 1;
    matched
}

/// The deepest heading of the comment's path that's still in the note, and its line: of the
/// headings with that text, the one nearest the comment's stored line.
fn pinned_heading(c: &Comment, text: &TextMap) -> Option<(String, u32)> {
    c.heading_path.iter().rev().find_map(|wanted| {
        text.headings()
            .iter()
            .filter(|h| h.text == *wanted)
            .min_by_key(|h| h.line.abs_diff(c.start_line))
            .map(|h| (h.text.clone(), h.line))
    })
}

/// After an operation, before saving: brings the stored anchor of every comment found word for
/// word up to date with the note (lines, heading path, prefix, suffix and fingerprint). Comments
/// that moved or were detached keep theirs until the reader reattaches, resolves or dismisses
/// them. Only comments that changed are marked for rewriting.
///
/// A capped quote's range keeps its length, shifted to where the excerpt starts now (see
/// [`resolve_all`]), and keeps its suffix: that is the text after the whole passage, which the
/// excerpt alone doesn't locate.
pub fn refresh_anchored(review: &mut Review, resolved: &[Resolved], text: &TextMap, fp: &str) {
    let comments = review.items.iter_mut().filter_map(|item| match item {
        Item::Comment(c) => Some(c),
        Item::Raw(_) => None,
    });
    for (c, r) in comments.zip(resolved) {
        let Some((s, e)) = r
            .span
            .filter(|_| r.state == AnchorState::Anchored && r.id == c.id)
        else {
            continue;
        };
        let heading_path = text.heading_path_at(r.start_line);
        let prefix = text.before(s, CONTEXT_CHARS);
        let suffix = match (is_capped(&c.quote), &c.anchor) {
            (true, Some(a)) => a.suffix.clone(),
            (true, None) => String::new(),
            (false, _) => text.after(e, CONTEXT_CHARS),
        };
        let same_anchor = c
            .anchor
            .as_ref()
            .is_some_and(|a| a.prefix == prefix && a.suffix == suffix && a.fp == fp);
        if same_anchor
            && c.start_line == r.start_line
            && c.end_line == r.end_line
            && c.heading_path == heading_path
        {
            continue;
        }
        c.start_line = r.start_line;
        c.end_line = r.end_line;
        c.heading_path = heading_path;
        let meta = c.anchor.get_or_insert_with(AnchorMeta::default);
        meta.prefix = prefix;
        meta.suffix = suffix;
        meta.fp = fp.to_owned();
        c.dirty = true;
    }
}
