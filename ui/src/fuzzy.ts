// Quick open's fuzzy matcher (spec §6): the query's characters, in order, anywhere in an item's
// file name or relative path, case-insensitively. A match scores bonuses for landing on word starts
// (after `-`, `_`, a space, `/` or `.`, or on a camel hump), for consecutive runs and for starting
// the string, less one point per character skipped between matched ones (capped per gap). A match
// inside the file name counts double and gets a bonus, so file names beat folder names.

export interface Scored<T> {
  item: T;
  score: number;
  /** Indexes of the matched characters in the item's `rel`, which ends with its `name`. */
  positions: number[];
}

const WORD_START = 8;
const CONSECUTIVE = 5;
const FIRST_CHAR = 10;
const FILENAME_HIT = 15;
const NAME_WEIGHT = 2;
/** At most this much is taken off for one gap between matched characters. */
const GAP_CAP = 5;
const DEFAULT_LIMIT = 50;

/** Characters after which a word starts. */
const SEPARATORS = new Set(["-", "_", " ", "/", ".", "\\"].map((c) => c.charCodeAt(0)));

/** Lower-cased char codes, memoised for characters outside ASCII. */
const folded = new Map<number, number>();

function fold(code: number): number {
  if (code < 128) {
    return code >= 65 && code <= 90 ? code + 32 : code;
  }
  let lower = folded.get(code);
  if (lower === undefined) {
    lower = String.fromCharCode(code).toLowerCase().charCodeAt(0);
    folded.set(code, lower);
  }
  return lower;
}

function isUpper(code: number): boolean {
  return code >= 65 && code <= 90;
}

function isLower(code: number): boolean {
  return code >= 97 && code <= 122;
}

/** Whether a word starts at `i` in `s`. */
function wordStart(s: string, i: number): boolean {
  if (i === 0) {
    return true;
  }
  const prev = s.charCodeAt(i - 1);
  return SEPARATORS.has(prev) || (isUpper(s.charCodeAt(i)) && isLower(prev));
}

/** The score of matching at `positions` in `s`. */
function scoreAt(s: string, positions: Int32Array, m: number): number {
  let score = 0;
  for (let k = 0; k < m; k++) {
    const i = positions[k] ?? 0;
    score += 1;
    if (wordStart(s, i)) score += WORD_START;
    if (k === 0) {
      if (i === 0) score += FIRST_CHAR;
    } else {
      const gap = i - (positions[k - 1] ?? 0) - 1;
      score += gap === 0 ? CONSECUTIVE : -Math.min(gap, GAP_CAP);
    }
  }
  return score;
}

/**
 * Matches the folded query `q` in `s` from `start` onwards, taking each query character at its
 * first occurrence; false when the rest of `s` doesn't hold the query.
 */
function greedy(s: string, q: Int32Array, start: number, out: Int32Array): boolean {
  let k = 0;
  for (let i = start; i < s.length && k < q.length; i++) {
    if (fold(s.charCodeAt(i)) === q[k]) {
      out[k++] = i;
    }
  }
  return k === q.length;
}

/**
 * Narrows a greedy match to the shortest window ending where it ends, matching backwards from
 * there: the leftmost-first characters a greedy pass picks can sit far from the rest.
 */
function tighten(s: string, q: Int32Array, out: Int32Array): void {
  let i = out[q.length - 1] ?? 0;
  for (let k = q.length - 1; k >= 0; k--) {
    while (fold(s.charCodeAt(i)) !== q[k]) i--;
    out[k] = i--;
  }
}

/** Scratch buffers, reused across calls: a match being tried, the best one, a copy of it. */
let trial = new Int32Array(16);
let best = new Int32Array(16);
let kept = new Int32Array(16);

/**
 * The best match of `q` in `s` (from `from` on), as a score with its positions left in `best`,
 * or -Infinity. Tries the tightened leftmost match, then a match starting at each word start
 * holding the query's first character.
 */
function bestMatch(s: string, q: Int32Array, from: number): number {
  const m = q.length;
  if (!greedy(s, q, from, trial)) {
    return -Infinity;
  }
  tighten(s, q, trial);
  let top = scoreAt(s, trial, m);
  best.set(trial.subarray(0, m));
  const first = q[0];
  for (let i = from; i < s.length; i++) {
    if (fold(s.charCodeAt(i)) !== first || !wordStart(s, i) || i === best[0]) continue;
    if (!greedy(s, q, i, trial)) break;
    const score = scoreAt(s, trial, m);
    if (score > top) {
      top = score;
      best.set(trial.subarray(0, m));
    }
  }
  return top;
}

/**
 * The items matching `query`, best first (then shorter paths, then in their given order), at most
 * `limit` of them. An empty query keeps every item in order, unscored.
 */
export function fuzzyFilter<T>(
  query: string,
  items: T[],
  key: (t: T) => { name: string; rel: string },
  limit = DEFAULT_LIMIT,
): Scored<T>[] {
  // By code point; a character outside the BMP matches by its first half, which is close enough.
  // eslint-disable-next-line @typescript-eslint/no-misused-spread -- no grapheme clusters needed
  const chars = [...query.replace(/\s+/g, "")];
  if (chars.length === 0) {
    return items.slice(0, limit).map((item) => ({ item, score: 0, positions: [] }));
  }
  const q = Int32Array.from(chars, (c) => fold(c.charCodeAt(0)));
  const m = q.length;
  if (trial.length < m) {
    trial = new Int32Array(m);
    best = new Int32Array(m);
    kept = new Int32Array(m);
  }
  // The best `limit` so far, kept sorted; anything not beating the last is skipped cheaply.
  const top: Ranked<T>[] = [];
  for (let index = 0; index < items.length; index++) {
    const item = items[index] as T;
    const { name, rel } = key(item);
    // The path first: when it can't hold the query, neither can the name at its end.
    let score = bestMatch(rel, q, 0);
    if (score === -Infinity) continue;
    // Where the positions' string starts in `rel`: 0, or the name's start.
    let shift = 0;
    const nameStart = rel.length - name.length;
    if (nameStart >= 0 && rel.endsWith(name)) {
      kept.set(best.subarray(0, m));
      const inName = bestMatch(name, q, 0);
      if (inName !== -Infinity && inName * NAME_WEIGHT + FILENAME_HIT >= score) {
        score = inName * NAME_WEIGHT + FILENAME_HIT;
        shift = nameStart;
      } else {
        best.set(kept.subarray(0, m));
      }
    }
    const len = rel.length;
    const last = top[top.length - 1];
    if (top.length >= limit && last && !ranksAbove(score, len, index, last)) continue;
    const positions = Array.from(best.subarray(0, m), (i) => i + shift);
    const entry: Ranked<T> = { item, score, len, index, positions };
    let at = top.length;
    while (at > 0 && ranksAbove(score, len, index, top[at - 1] as Ranked<T>)) at--;
    top.splice(at, 0, entry);
    if (top.length > limit) top.pop();
  }
  return top.map(({ item, score, positions }) => ({ item, score, positions }));
}

interface Ranked<T> extends Scored<T> {
  len: number;
  index: number;
}

function ranksAbove(
  score: number,
  len: number,
  index: number,
  other: { score: number; len: number; index: number },
): boolean {
  if (score !== other.score) return score > other.score;
  if (len !== other.len) return len < other.len;
  return index < other.index;
}
