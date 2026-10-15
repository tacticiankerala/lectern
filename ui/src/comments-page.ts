// An HTML page as the review comments see it, in place of the rendered note (page-frame.ts gives
// it): the text its bridge reported, which core anchors the comments against
// (`TextMap::from_blocks`), the text selected in it, and highlights and scrolling, which the
// bridge does inside the page's frame. Part of the comments module (comments.ts).
//
// Contracts:
// - A page's text has no source lines on this side: a comment on a selection is anchored by its
//   quote and the text before it (lines 0, which core reads as "anywhere"), and core gives back
//   the lines of the page's source where it found it.
// - A comment's highlight goes where core found it in the page's text (`textStart` to `textEnd`)
//   when the text there is its quote, else on the quote's first place in the page.
import type { Box } from "./comment-adding";
import { matchPart } from "./comments-model";
import { charsBefore, normalize } from "./comments-text";
import type { Span } from "./find";
import type { CommentView } from "./generated/CommentView";
import type { NewAnchor } from "./generated/NewAnchor";
import type { PageBlock } from "./generated/PageBlock";
import type { PageSelection } from "./page-frame";

/** The layers a page paints the comments' highlights in. */
export type CommentLayer = "comment" | "comment-focus";

/** An HTML page on screen, once its session with its bridge has started. */
export interface PageSurface {
  /** The page has reported its text: it may be painted and scrolled, and its comments loaded. */
  readonly ready: boolean;
  /** The page's text: its blocks joined by single spaces. */
  readonly text: string;
  /** The blocks, as the page reported them: what its comments are anchored against. */
  readonly blocks: readonly PageBlock[];
  /** Where each block starts in `text`. */
  readonly starts: readonly number[];
  /** Where the view's top is in the text: the start of the block there. */
  readonly top: number;
  /** The text selected in the page, as it last said. */
  readonly selection: PageSelection | null;
  /** The frame's box in Lectern's viewport. */
  frameBox(): DOMRect;
  paint(layer: CommentLayer, spans: Span[]): void;
  clear(layer: CommentLayer): void;
  /** Scrolls the text at `offset` to the middle, if it is out of sight. */
  reveal(offset: number): void;
  /** Scrolls the text at `offset` to the top. */
  scrollTo(offset: number): void;
}

/**
 * Where a comment's text is in the page's `text`: its quote, or the passage it became when it
 * moved. At the place core found it when the text holds it there, else its first place in the
 * text; null when it isn't there.
 */
export function pageSpan(text: string, c: CommentView): Span | null {
  const needle = normalize(
    c.state === "moved" && c.currentText !== null ? c.currentText : matchPart(c.quote),
  );
  if (needle === "") {
    return null;
  }
  if (c.textStart !== null && c.textEnd !== null && text.slice(c.textStart, c.textEnd) === needle) {
    return { start: c.textStart, end: c.textEnd };
  }
  const at = text.indexOf(needle);
  return at === -1 ? null : { start: at, end: at + needle.length };
}

/**
 * A comment on the text selected in the page: what it is anchored by (its quote, and the text
 * before it when the page's text is the one the selection was made in), and the box of the
 * selection's last line in Lectern's viewport. Null when nothing is selected.
 */
export function pageSelected(page: PageSurface): { anchor: NewAnchor; box: Box } | null {
  const selection = page.selection;
  const quote = selection === null ? "" : normalize(selection.quote);
  if (selection === null || quote === "") {
    return null;
  }
  const { start, end, rect } = selection;
  let first = start;
  while (first < end && page.text[first] === " ") first++;
  const same = page.text.slice(start, end) === selection.quote;
  const frame = page.frameBox();
  return {
    anchor: { startLine: 0, endLine: 0, quote, prefix: same ? charsBefore(page.text, first) : "" },
    box: {
      left: frame.left + rect.x,
      right: frame.left + rect.x + rect.width,
      top: frame.top + rect.y,
      bottom: frame.top + rect.y + rect.height,
    },
  };
}

/**
 * A comment on the block at the top of the page's view: its text, and the text before it, which
 * tells it apart from the same text elsewhere. Null without one.
 */
export function pageTopAnchor(page: PageSurface): NewAnchor | null {
  const i = Math.max(0, page.starts.lastIndexOf(page.top));
  const quote = normalize(page.blocks[i]?.text ?? "");
  if (quote === "") {
    return null;
  }
  return { startLine: 0, endLine: 0, quote, prefix: charsBefore(page.text, page.starts[i] ?? 0) };
}
