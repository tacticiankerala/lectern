// An agent's requests to this window (`agent-request`), answered with `replyToAgent`. For now that
// is `status`'s part: what the window shows. That is the file, the heading at the top of the view
// with the headings it sits under, the source line there, and the reader's selection with its
// lines. A note's or text file's are read from the document; an HTML page's from what its bridge
// last said (page-frame.ts), with no lines; a PDF tells only its file. The other tools aren't
// offered yet. Loaded on the first request.
import { blockLines, leafBlockOf, quoteFromRange, textOnly } from "./comments-text";
import { HEADINGS } from "./dom";
import type { AgentReply } from "./generated/AgentReply";
import type { AgentRequest } from "./generated/AgentRequest";
import type { DocPayload } from "./generated/DocPayload";
import type { ViewSelection } from "./generated/ViewSelection";
import type { WindowView } from "./generated/WindowView";
import { READING_BAND_PERCENT } from "./outline";
import type { PageFrame } from "./page-frame";

/** The answer to a tool this window doesn't take part in yet. */
export const NOT_YET = "Not yet available.";

/** What answering needs from the app. */
export interface AgentHost {
  /** The document shown, if any. */
  doc(): DocPayload | null;
  /** The element a note or text file renders in. */
  docElement(): HTMLElement;
  /** The element the document scrolls in. */
  scroller(): HTMLElement;
  /** The HTML page's session with its bridge, once it has one. */
  page(): Pick<PageFrame, "headingPath" | "selection"> | null;
}

/** This window's answer to `request`. */
export function answer(host: AgentHost, request: AgentRequest): AgentReply {
  if (request.tool === "status") {
    return { kind: "view", ...view(host) };
  }
  return { kind: "failed", message: NOT_YET };
}

/** What the window shows. */
export function view(host: AgentHost): WindowView {
  const doc = host.doc();
  const none: WindowView = { path: null, heading: [], line: null, selection: null };
  if (doc === null) {
    return none;
  }
  switch (doc.kind) {
    case "markdown":
    case "text":
      return { path: doc.path, ...noteView(host.docElement(), host.scroller(), doc) };
    case "html": {
      const page = host.page();
      const quote = page?.selection?.quote ?? "";
      return {
        ...none,
        path: doc.path,
        heading: page?.headingPath() ?? [],
        selection: quote === "" ? null : { quote, startLine: null, endLine: null },
      };
    }
    case "pdf":
      return { ...none, path: doc.path };
  }
}

/** A note's or text file's view, from the document `el`, scrolled in `scroller`. */
function noteView(
  el: HTMLElement,
  scroller: HTMLElement,
  doc: DocPayload,
): Omit<WindowView, "path"> {
  const pane = scroller.getBoundingClientRect();
  return {
    heading: headingPath(el, pane, doc),
    line: topLine(el, pane.top),
    selection: selection(el),
  };
}

/**
 * The heading being read in the pane `pane`, after the headings it sits under, outermost first,
 * as the outline marks it (outline.ts): the first heading showing in the top part of the pane,
 * where a navigation lands one, else the last heading scrolled past; above every heading, the
 * first when it's in view. Each by its text in the outline, as core has it, else as shown. A
 * heading that isn't shown, folded in a closed details or not rendered at all, counts for nothing.
 */
function headingPath(el: HTMLElement, pane: DOMRect, doc: DocPayload): string[] {
  const bandBottom = pane.top + (pane.height * READING_BAND_PERCENT) / 100;
  const path: { level: number; text: string }[] = [];
  let first: { level: number; text: string } | null = null;
  for (const heading of el.querySelectorAll<HTMLElement>(HEADINGS)) {
    if (!shown(heading)) continue;
    const box = heading.getBoundingClientRect();
    const item = {
      level: Number(heading.tagName.slice(1)),
      text:
        doc.outline.find((o) => o.id !== "" && o.id === heading.id)?.text ??
        heading.textContent.trim(),
    };
    if (box.top > bandBottom) {
      if (path.length === 0 && box.top < pane.bottom) first = item;
      break;
    }
    while (path.length > 0 && (path[path.length - 1]?.level ?? 0) >= item.level) path.pop();
    path.push(item);
    // Any of it showing in the top part: the one being read. Else scrolled past; the next may be.
    if (box.bottom >= pane.top) break;
  }
  return (path.length > 0 ? path : first ? [first] : []).map((h) => h.text);
}

/** Whether `heading` is on screen: not folded in a closed details (outside its summary), and rendered. */
function shown(heading: HTMLElement): boolean {
  const closed = heading.closest("details:not([open])");
  if (closed && closed.querySelector(":scope > summary")?.contains(heading) !== true) {
    return false;
  }
  return typeof heading.checkVisibility !== "function" || heading.checkVisibility();
}

/**
 * The source line the block at the pane's top, `top`, starts on; for a wrapper the renderer adds
 * without lines of its own (a table's `.table-wrap`), the line of the first block inside it.
 */
function topLine(el: HTMLElement, top: number): number | null {
  for (const block of el.children) {
    if (block.getBoundingClientRect().bottom > top) {
      const lined = block.hasAttribute("data-sourcepos")
        ? block
        : block.querySelector("[data-sourcepos]");
      return lined ? (blockLines(lined)?.[0] ?? null) : null;
    }
  }
  return null;
}

/** The reader's selection in the document `el`, as a comment on it would quote it. */
function selection(el: HTMLElement): ViewSelection | null {
  const selected = document.getSelection();
  if (!selected || selected.rangeCount === 0 || selected.isCollapsed) {
    return null;
  }
  const whole = selected.getRangeAt(0);
  if (!el.contains(whole.startContainer) || !el.contains(whole.endContainer)) {
    return null;
  }
  const range = textOnly(whole, el);
  const quote = range ? quoteFromRange(range) : "";
  if (!range || quote === "") {
    return null;
  }
  const startBlock = leafBlockOf(range.startContainer, el);
  const endBlock = leafBlockOf(range.endContainer, el);
  const start = startBlock ? blockLines(startBlock) : null;
  const end = endBlock ? blockLines(endBlock) : null;
  return {
    quote,
    startLine: start?.[0] ?? null,
    endLine: end ? Math.max(start?.[0] ?? 0, end[1]) : (start?.[1] ?? null),
  };
}
