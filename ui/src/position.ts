// The reading position: where the reader is in a document, captured to come back to it (history,
// reopening, live reload) and restored once the document renders again.
//
// The `SavedPosition` contract: the nearest heading above the pane's top and the distance below
// it, then the top block's source line, then the scroll fraction. Restoring tries them in that
// order, so a position outlives edits that move or drop the parts it falls back from.
import { HEADINGS, findById } from "./dom";
import type { SavedPosition } from "./generated/SavedPosition";
import { placeAt } from "./reflow";

/** How many screens below a heading `layoutBelow` lays out at most. */
const LAYOUT_SCREENS = 4;

/**
 * Where the reader is in `doc`, scrolled in `scroller`: the nearest heading above the pane's top
 * and the distance below it, the top block's source line, and the scroll fraction. Above the
 * text (at the top, or in the properties strip) only the fraction is kept, so coming back doesn't
 * scroll the strip away.
 */
export function capture(doc: HTMLElement, scroller: HTMLElement): SavedPosition {
  const top = scroller.getBoundingClientRect().top;
  const max = scroller.scrollHeight - scroller.clientHeight;
  const fraction = max > 0 ? scroller.scrollTop / max : 0;
  if (doc.getBoundingClientRect().top >= top) {
    return { headingId: null, offset: 0, line: null, fraction };
  }
  let headingId: string | null = null;
  let offset = 0;
  let above: HTMLElement | null = null;
  for (const heading of doc.querySelectorAll<HTMLElement>(HEADINGS)) {
    if (heading.getBoundingClientRect().top > top + 1) {
      break;
    }
    above = heading;
  }
  if (above) {
    // Measured with the blocks between at their real height, as `restore` places it.
    layoutBelow(doc, scroller, above, top - above.getBoundingClientRect().top);
    headingId = above.id;
    offset = top - above.getBoundingClientRect().top;
  }
  let line: number | null = null;
  for (const block of doc.children) {
    if (block.getBoundingClientRect().bottom > top) {
      line = sourceLine(block);
      break;
    }
  }
  return { headingId, offset, line, fraction };
}

/**
 * Scrolls back to a captured position: by heading id, else by line (the block whose source starts
 * last at or before it), else by fraction. Held in place while lazy layout settles.
 */
export function restore(doc: HTMLElement, scroller: HTMLElement, p: SavedPosition): void {
  const heading = p.headingId === null ? null : findById(doc, p.headingId);
  if (heading) {
    layoutBelow(doc, scroller, heading, p.offset);
    placeAt(scroller, heading, -p.offset);
    return;
  }
  const block = p.line === null ? null : blockAt(doc, p.line);
  if (block) {
    placeAt(scroller, block, 0);
    return;
  }
  scroller.scrollTop = p.fraction * (scroller.scrollHeight - scroller.clientHeight);
}

/** The element of `doc` whose source starts last at or before `line`. */
export function blockAt(doc: HTMLElement, line: number): HTMLElement | null {
  let best: HTMLElement | null = null;
  let bestLine = 0;
  for (const el of doc.querySelectorAll<HTMLElement>("[data-sourcepos]")) {
    const start = sourceLine(el);
    if (start !== null && start <= line && start > bestLine) {
      best = el;
      bestLine = start;
    }
  }
  return best;
}

/** The start line of an element's `data-sourcepos` (`12:1-14:3`). */
function sourceLine(el: Element): number | null {
  const match = /^(\d+):/.exec(el.getAttribute("data-sourcepos") ?? "");
  return match ? Number(match[1]) : null;
}

/**
 * Lays out for real the blocks from `heading` to a screen below `offset`. Off screen they keep
 * content-visibility's estimated height, so with a long stretch between the heading and the
 * saved spot (a long section), placing the heading alone would land the reader far off it.
 * Only within a few screens of the heading, so a long document with few headings never lays
 * out whole.
 */
function layoutBelow(
  doc: HTMLElement,
  scroller: HTMLElement,
  heading: HTMLElement,
  offset: number,
): void {
  const screen = scroller.clientHeight;
  if (offset > LAYOUT_SCREENS * screen) {
    return;
  }
  let block: Element | null = heading;
  while (block && block.parentElement !== doc) {
    block = block.parentElement;
  }
  const end = heading.getBoundingClientRect().top + offset + screen;
  for (; block instanceof HTMLElement; block = block.nextElementSibling) {
    block.style.contentVisibility = "visible";
    if (block.getBoundingClientRect().bottom >= end) {
      break;
    }
  }
}
