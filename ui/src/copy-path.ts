// Copying a path in the form a terminal takes: Windows (`C:\…`, `\\server\…`) or WSL (`/…`).
// Rust works out the forms (`PathMapper::forms`); this decides which path a shortcut or a link's
// menu means, and words the toasts.
import { LOCAL_KINDS } from "./doc-view";

export type PathForm = "windows" | "wsl";

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

/** The most characters of a path a toast shows (about its width); a longer one loses its middle. */
export const TOAST_PATH_CHARS = 60;

/** Whitespace, quotes and backticks around a path, which core trims too. */
const SURROUNDING = /^[\s"'`“”‘’]+|[\s"'`“”‘’]+$/gu;

/** `text` without the whitespace, quotes and backticks around it. */
export function trimPath(text: string): string {
  return text.replace(SURROUNDING, "");
}

/**
 * `text`, trimmed, when it is one absolute path: `/…`, `X:\…` (or `X:/…`) or `\\server\…`, on a
 * single line. Null otherwise.
 */
export function asPath(text: string): string | null {
  const path = trimPath(text);
  return !/[\r\n]/.test(path) && /^(?:\/|\\\\|[A-Za-z]:[\\/])/.test(path) ? path : null;
}

/**
 * A path to copy. `literal`: an exact path on this machine (the open file, a tree row, a resolved
 * link's target), which Rust takes as it is and which can be opened or revealed. Otherwise text
 * written as a path (a selection, a broken link), which Rust cleans up and maps as a link.
 */
export interface PathToCopy {
  path: string;
  literal: boolean;
}

/**
 * The path of a link in a note or a comment: a `doc`, `file` or `path` link's target, exact; or a
 * broken link's text (else its href), as text, when that is written as an absolute path. Null for
 * any other link.
 */
export function linkPath(link: HTMLAnchorElement): PathToCopy | null {
  const kind = link.dataset.kind;
  if (kind !== undefined && LOCAL_KINDS.has(kind)) {
    const target = link.dataset.target;
    return target ? { path: target, literal: true } : null;
  }
  if (kind === "broken") {
    const path = asPath(link.textContent) ?? asPath(link.getAttribute("href") ?? "");
    return path === null ? null : { path, literal: false };
  }
  return null;
}

/** Whether `node` is in the note on screen (`doc`) or in a review comment's card. */
export function inNoteOrComment(node: Node | null, doc: HTMLElement): boolean {
  const el = node instanceof Element ? node : (node?.parentElement ?? null);
  return el !== null && (doc.contains(el) || el.closest(".comment-card") !== null);
}

/**
 * The path a copy shortcut copies: the selection in the note or a comment when it is one absolute
 * path (as text), else the path link focused there, else `file` (the file open, if any, exact).
 */
export function shortcutPath(doc: HTMLElement, file: string | null): PathToCopy | null {
  const selection = document.getSelection();
  if (
    selection &&
    !selection.isCollapsed &&
    inNoteOrComment(selection.anchorNode, doc) &&
    inNoteOrComment(selection.focusNode, doc)
  ) {
    const selected = asPath(selection.toString());
    if (selected !== null) {
      return { path: selected, literal: false };
    }
  }
  const focused = document.activeElement;
  if (focused instanceof HTMLAnchorElement && inNoteOrComment(focused, doc)) {
    const link = linkPath(focused);
    if (link) {
      return link;
    }
  }
  return file === null ? null : { path: file, literal: true };
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
