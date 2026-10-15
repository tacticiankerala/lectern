// Copying a path in the form a terminal takes: Windows (`C:\…`, `\\server\…`) or WSL (`/…`).
// Rust works out the forms (`PathMapper::forms`) and converts the paths in a selection
// (`PathMapper::convert_paths`); this reads a selection or a link as paths, decides what a
// shortcut or a menu means, and words the toasts.
import { LOCAL_KINDS } from "./doc-view";
import type { PathForm } from "./generated/PathForm";
import { CONVERT_LIMIT } from "./generated/path-tokens";

export type { PathForm };

/** The toast once a path is copied; the path follows it on a line of its own. */
export const COPIED: Record<PathForm, string> = {
  windows: "Copied the Windows path",
  wsl: "Copied the WSL path",
};

/** The toast when a path has no such form, and nothing is copied. */
export const NO_FORM: Record<PathForm, string> = {
  windows: "No Windows path for this location.",
  wsl: "No WSL path for this location. A path mapping in Preferences can add one.",
};

/** Each form's name, in the menus and toasts. */
export const FORM_NAME: Record<PathForm, string> = { windows: "Windows", wsl: "WSL" };

/** The shortcut that copies each form. */
export const COPY_KEYS: Record<PathForm, string> = {
  windows: "Ctrl+Shift+W",
  wsl: "Ctrl+Shift+C",
};

/** The toast for a shortcut on a selection past `CONVERT_LIMIT`, which isn't read at all. */
export const SELECTION_TOO_LONG = "The selection is too long to convert.";

/** The toast once a selection is copied with its paths converted: how many were, and weren't. */
export function copiedWith(form: PathForm, converted: number, unconverted: number): string {
  const name = FORM_NAME[form];
  const copied = `Copied with ${String(converted)} ${name} path${converted === 1 ? "" : "s"}`;
  return unconverted > 0 ? `${copied}; ${String(unconverted)} had no ${name} form` : copied;
}

/** The most characters of a path a toast shows (about its width); a longer one loses its middle. */
export const TOAST_PATH_CHARS = 60;

/** Nothing but the whitespace, quotes and backticks that core trims from around a path. */
const ONLY_SURROUNDING = /^[\p{White_Space}"'`“”‘’]*$/u;

/** Where a path token is in a text, in UTF-16 code units. */
export interface TokenSpan {
  start: number;
  end: number;
}

const NON_SPACE = /[^\p{White_Space}]/gu;
const SPACE = /\p{White_Space}/gu;
/** The end of a flag's name: its `=`, or the end of the word. */
const FLAG_END = /[=\p{White_Space}]/gu;
/** Where a quoted run stops: its closing quote, or a line break. */
const QUOTED_END: Record<string, RegExp> = { '"': /["\n\r]/g, "'": /['\n\r]/g };

/** Where `pattern` (global) next matches in `text` from `from`, or -1. */
function next(pattern: RegExp, text: string, from: number): number {
  pattern.lastIndex = from;
  return pattern.exec(text)?.index ?? -1;
}

/**
 * The paths in `text`, as core's `path_tokens` finds them: a token is a word (a run of anything
 * but whitespace), or the inside of a `"…"` or `'…'` run that opens a word and closes on the same
 * line. Of a flag's word (`--name=value`, `-n=value`) only the value counts. A token is a path
 * when it holds `/` or `\` and isn't a URL (`scheme://…` or `mailto:…`). Linear in `text`: every
 * scan stops where the next one starts, so nothing is read twice.
 */
export function pathTokens(text: string): TokenSpan[] {
  const tokens: TokenSpan[] = [];
  let at = 0;
  for (;;) {
    const word = next(NON_SPACE, text, at);
    if (word < 0) {
      return tokens;
    }
    const start = flagValue(text, word) ?? word;
    // A quoted run may end before its word does, or run on past spaces; either way, what follows
    // its closing quote is read afresh.
    let token = quotedRun(text, start);
    if (token) {
      at = token.end + 1;
    } else {
      const end = next(SPACE, text, start);
      at = end < 0 ? text.length : end;
      token = { start, end: at };
    }
    if (isPathToken(text.slice(token.start, token.end))) {
      tokens.push(token);
    }
  }
}

/**
 * Where the value starts in a flag's word at `word` (`--name=value`, `-n=value`): just after its
 * first `=`, when the name before it holds no separator or quote.
 */
function flagValue(text: string, word: number): number | null {
  if (text.charAt(word) !== "-") {
    return null;
  }
  const end = next(FLAG_END, text, word);
  return end >= 0 && text.charAt(end) === "=" && !/[\\/"']/.test(text.slice(word, end))
    ? end + 1
    : null;
}

/**
 * The inside of the `"…"` or `'…'` run that opens at `start`, if it closes before a line break.
 * The scan stops at whichever comes first.
 */
function quotedRun(text: string, start: number): TokenSpan | null {
  const end = QUOTED_END[text.charAt(start)];
  if (end === undefined) {
    return null;
  }
  const stop = next(end, text, start + 1);
  return stop >= 0 && text.charAt(stop) === text.charAt(start)
    ? { start: start + 1, end: stop }
    : null;
}

/** Whether a token is a path: it holds a separator and isn't a URL. */
function isPathToken(token: string): boolean {
  return /[\\/]/.test(token) && !token.includes("://") && !/mailto:/i.test(token);
}

/**
 * What a copy converts:
 * - `exact`: a path on this machine (the open file, a tree row, where a wikilink leads), which Rust
 *   takes as it is;
 * - `written`: the path a link was written with, which the renderer has already decoded and
 *   stripped of its fragment and line, so Rust takes it as it is (`written_forms`): an absolute one
 *   mapped as the link is, any other only changing its separators (a bare file name has none);
 * - `text`: one path in a selection or a broken link's text, which Rust maps by itself (an
 *   absolute one as a link is, a relative one only changing its separators), keeping the quotes,
 *   backticks, `:line` or `#anchor` around it;
 * - `paths`: text holding paths (a selected command or block), each converted where it is.
 */
export type PathToCopy =
  | { kind: "exact"; path: string }
  | { kind: "written"; path: string }
  | { kind: "text"; path: string }
  | { kind: "paths"; text: string };

/** Where an absolute path starts: `X:\`, `X:/`, `\\` or `/`. */
const ABSOLUTE = /^(?:[A-Za-z]:[\\/]|\\\\|\/)/;

/**
 * What makes a line that starts as an absolute path read as a command: another absolute path or a
 * flag (`-x`, `--name`) after whitespace, or a shell operator. A dash on its own, as in
 * `Report - final.md`, is part of a name.
 */
const COMMAND = /\s(?:[A-Za-z]:[\\/]|\\\\|\/|--?[A-Za-z])|[|;&<>]/;

/** One character of the whitespace, quotes and backticks that core trims from around a path. */
const AROUND = /[\p{White_Space}"'`“”‘’]/u;

/** `text` without the whitespace, quotes and backticks around it. */
function trimmed(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && AROUND.test(text.charAt(start))) start++;
  while (end > start && AROUND.test(text.charAt(end - 1))) end--;
  return text.slice(start, end);
}

/**
 * How `text` reads as paths: null when it holds none; else one path (`text`) or text holding
 * paths (`paths`). It is one path when, trimmed, it is a single line that starts as an absolute
 * path and doesn't read as a command (spaces and all, so a mapping reaches the whole of it), or
 * when nothing but whitespace, quotes and backticks surround its only path token.
 */
export function pathsIn(text: string): PathToCopy | null {
  const line = trimmed(text);
  if (ABSOLUTE.test(line) && !/[\n\r]/.test(line) && !COMMAND.test(line)) {
    return { kind: "text", path: text };
  }
  const tokens = pathTokens(text);
  const [first] = tokens;
  if (first === undefined) {
    return null;
  }
  const one =
    tokens.length === 1 &&
    ONLY_SURROUNDING.test(text.slice(0, first.start)) &&
    ONLY_SURROUNDING.test(text.slice(first.end));
  return one ? { kind: "text", path: text } : { kind: "paths", text };
}

/** What a link in a note or a comment offers to copy and where it leads. */
export interface LinkPaths {
  /** What Copy copies: the link's text, as shown. */
  text: string;
  /** What "Copy as … path" converts. */
  path: PathToCopy;
  /** The file it opens and reveals; null for a broken link. */
  target: string | null;
}

/**
 * The paths of a link in a note or a comment. A `doc`, `file` or `path` link converts the path it
 * was written with (`data-written`) literally, so a relative one stays relative and a bare file
 * name is the same in both forms. Only a link written with no path (a wikilink) converts its exact
 * target. A broken link converts its text, when that holds a path. Null for any other link.
 */
export function linkPaths(link: HTMLAnchorElement): LinkPaths | null {
  const kind = link.dataset.kind;
  const text = link.textContent;
  if (kind !== undefined && LOCAL_KINDS.has(kind)) {
    const target = link.dataset.target;
    if (!target) {
      return null;
    }
    const written = link.dataset.written;
    const path: PathToCopy = written
      ? { kind: "written", path: written }
      : { kind: "exact", path: target };
    return { text, path, target };
  }
  if (kind === "broken") {
    const path = pathsIn(text);
    return path === null ? null : { text, path, target: null };
  }
  return null;
}

/** Whether `node` is in the note on screen (`doc`) or in a review comment's card. */
export function inNoteOrComment(node: Node | null, doc: HTMLElement): boolean {
  const el = node instanceof Element ? node : (node?.parentElement ?? null);
  return el !== null && (doc.contains(el) || el.closest(".comment-card") !== null);
}

/** A selection holding paths: its text as selected, what its copies convert, and its range. */
export interface SelectedPaths {
  text: string;
  path: PathToCopy;
  range: Range;
}

/** The selection in the note or a comment, if any: its text and its range. */
function selectionIn(doc: HTMLElement): { text: string; range: Range } | null {
  const selection = document.getSelection();
  if (
    !selection ||
    selection.isCollapsed ||
    selection.rangeCount === 0 ||
    !inNoteOrComment(selection.anchorNode, doc) ||
    !inNoteOrComment(selection.focusNode, doc)
  ) {
    return null;
  }
  return { text: selection.toString(), range: selection.getRangeAt(0) };
}

/** Whether `text` is more than a conversion takes (`CONVERT_LIMIT`, in UTF-8 bytes). */
function tooLong(text: string): boolean {
  return text.length > CONVERT_LIMIT || new TextEncoder().encode(text).length > CONVERT_LIMIT;
}

/** One line break (`\r\n`, `\n` or `\r`) at the very end of a text. */
const TRAILING_BREAK = /\r?\n$|\r$/;

/**
 * What a selection's copies convert: how it reads as paths (`pathsIn`), less the one line break
 * at its very end. A triple-click selects a line's break with it, and a converted command pasted
 * into a terminal with one would run at once. The breaks between a block's lines stay, and Copy
 * keeps them all.
 */
function pathsToConvert(selection: string): PathToCopy | null {
  return pathsIn(selection.replace(TRAILING_BREAK, ""));
}

/**
 * The selection in the note or a comment, when it holds a path; else null, and for one too long
 * to convert, which isn't read at all. Its copies convert it less the line break at its end.
 */
export function selectedPaths(doc: HTMLElement): SelectedPaths | null {
  const selected = selectionIn(doc);
  if (!selected || tooLong(selected.text)) {
    return null;
  }
  const path = pathsToConvert(selected.text);
  return path && { ...selected, path };
}

/**
 * What a copy shortcut converts: the selection in the note or a comment when it holds a path, else
 * the link focused there, else `file` (the file open, if any, exact). A selection too long to
 * convert is `too-long`, unread; neither ever falls back to the file.
 */
export function shortcutPath(
  doc: HTMLElement,
  file: string | null,
): PathToCopy | "too-long" | null {
  const selected = selectionIn(doc);
  if (selected && tooLong(selected.text)) {
    return "too-long";
  }
  const path = selected && pathsToConvert(selected.text);
  if (path) {
    return path;
  }
  const focused = document.activeElement;
  if (focused instanceof HTMLAnchorElement && inNoteOrComment(focused, doc)) {
    const link = linkPaths(focused);
    if (link) {
      return link.path;
    }
  }
  return file === null ? null : { kind: "exact", path: file };
}

/** `text` cut to `max` characters by dropping its middle, which an ellipsis stands for. */
export function middleTruncate(text: string, max: number): string {
  const chars = Array.from(new Intl.Segmenter().segment(text), (s) => s.segment);
  if (chars.length <= max) {
    return text;
  }
  const head = Math.floor((max - 1) / 2);
  const tail = max - 1 - head;
  return `${chars.slice(0, head).join("")}…${chars.slice(chars.length - tail).join("")}`;
}
