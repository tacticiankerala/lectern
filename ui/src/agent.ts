// An agent's requests to this window (`agent-request`), answered with `replyToAgent`:
// - `status`'s part: what the window shows. That is the file, the heading at the top of the view
//   with the headings it sits under, the source line there, and the reader's selection with its
//   lines; with the request the window is asking the reader about and what became of the earlier
//   ones. A note's or text file's are read from the document; an HTML page's from what its bridge
//   last said (page-frame.ts), with no lines; a PDF tells only its file.
// - `show`: a file opens at once in a window that shows none, or that file already (moving to its
//   line), or when the `agentOpens` setting says to switch. Otherwise the window asks the reader
//   first, with a bar over the top of the document: "Claude wants to show plan.md › Rollout",
//   Show and Not now. One request asks at a time: a newer one takes its place, whatever the newer
//   one does. It is never dropped otherwise; the reader's own moves leave it. What became of each
//   request asked about (shown, dismissed or replaced) is told to Rust as an `answered` reply to
//   it, and kept for `status`. Agent opens go on the history, so Back returns.
// The other tools aren't offered yet. Loaded on the first request.
import { blockLines, leafBlockOf, quoteFromRange, textOnly } from "./comments-text";
import { h, HEADINGS, samePath } from "./dom";
import type { AgentCall } from "./generated/AgentCall";
import type { AgentOpens } from "./generated/AgentOpens";
import type { AgentReply } from "./generated/AgentReply";
import type { AskAnswer } from "./generated/AskAnswer";
import type { AskOutcome } from "./generated/AskOutcome";
import type { DocPayload } from "./generated/DocPayload";
import type { ShowArgs } from "./generated/ShowArgs";
import type { ViewSelection } from "./generated/ViewSelection";
import type { WindowView } from "./generated/WindowView";
import { READING_BAND_PERCENT } from "./outline";
import type { PageFrame } from "./page-frame";

/** The answer to a tool this window doesn't take part in yet. */
export const NOT_YET = "Not yet available.";

/** How many of the reader's answers the window keeps for `status`, the latest: as Rust keeps. */
const ANSWERS_KEPT = 10;

/** What reading the window's document needs from the app. */
export interface DocHost {
  /** The document shown, if any. */
  doc(): DocPayload | null;
  /** The element a note or text file renders in. */
  docElement(): HTMLElement;
  /** The element the document scrolls in. */
  scroller(): HTMLElement;
  /** The HTML page's session with its bridge, once it has one. */
  page(): Pick<PageFrame, "headingPath" | "selection"> | null;
}

/** What answering agents needs from the app. */
export interface AgentHost extends DocHost {
  /** The path on screen: the document's, or the one that failed to open; null for none. */
  shownPath(): string | null;
  /** Whether an agent's open of another file asks the reader first, or switches at once. */
  opens(): AgentOpens;
  /**
   * Opens `path` on the history, at the source line `line` when given. Once it has landed: null
   * when it is on screen, else why it isn't.
   */
  open(path: string, line: number | null): Promise<string | null>;
  /** Scrolls the document on screen to its source line `line`. */
  scrollToLine(line: number): void;
  /** Tells Rust what became of the request `id`, which the window asked the reader about. */
  answered(id: number, answer: AskAnswer): void;
  /** Where the bar that asks the reader goes. */
  readonly bar: HTMLElement;
}

/** A request to show a file that the window is asking the reader about. */
interface Ask {
  id: number;
  path: string;
  /** The agent's name, as the reader sees it. */
  agent: string;
  /** The heading it is about, as its target names it. */
  heading: string | null;
  line: number | null;
}

/** This window's part in answering agents. */
export class Agent {
  /** The request the bar asks about, if any. */
  private asking: Ask | null = null;
  /** What became of the requests asked about, the oldest first. */
  private readonly answers: AskOutcome[] = [];

  constructor(private readonly host: AgentHost) {}

  /** This window's answer to `call`. */
  async answer(call: AgentCall): Promise<AgentReply> {
    const request = call.request;
    switch (request.tool) {
      case "status":
        return { kind: "view", ...this.view() };
      case "show":
        return this.show(call.id, call.agent, request);
      default:
        return { kind: "failed", message: NOT_YET };
    }
  }

  /** What the window shows, with the request it asks about and the answers so far. */
  view(): WindowView {
    return { ...view(this.host), ask: this.asking?.path ?? null, answers: [...this.answers] };
  }

  /**
   * `show` from the agent `agent`, the request `id`: on screen at once, or asked about first (see
   * the top of this file).
   */
  private async show(id: number, agent: string, args: ShowArgs): Promise<AgentReply> {
    const target = args.target;
    const line = target && "line" in target ? target.line : null;
    this.settle("replaced");
    const doc = this.host.doc();
    if (doc && samePath(doc.path, args.path)) {
      if (line !== null) this.host.scrollToLine(line);
      return { kind: "shown" };
    }
    const shown = this.host.shownPath();
    if (shown === null || samePath(shown, args.path) || this.host.opens() === "switch") {
      return this.open(args.path, line);
    }
    const heading = target && "heading" in target ? target.heading : null;
    this.asking = { id, path: args.path, agent, heading, line };
    this.renderBar();
    return { kind: "asked" };
  }

  /** Opens `path` at `line`: shown, or failed with why. */
  private async open(path: string, line: number | null): Promise<AgentReply> {
    const failure = await this.host.open(path, line);
    return failure === null
      ? { kind: "shown" }
      : { kind: "failed", message: `${fileName(path)} didn't open: ${failure}` };
  }

  /**
   * The request being asked about, if any, settles as `answer`: the bar goes, Rust is told, and
   * `status` keeps it.
   */
  private settle(answer: AskAnswer): Ask | null {
    const ask = this.asking;
    if (ask === null) {
      return null;
    }
    this.asking = null;
    this.renderBar();
    this.answers.push({ path: ask.path, answer });
    this.answers.splice(0, Math.max(0, this.answers.length - ANSWERS_KEPT));
    this.host.answered(ask.id, answer);
    return ask;
  }

  /** The bar, for the request being asked about; hidden when there is none. */
  private renderBar(): void {
    const bar = this.host.bar;
    const ask = this.asking;
    bar.hidden = ask === null;
    if (ask === null) {
      bar.replaceChildren();
      return;
    }
    const show = h("button", { type: "button", class: "btn primary" }, "Show");
    const later = h("button", { type: "button", class: "btn" }, "Not now");
    show.addEventListener("click", () => {
      const shown = this.settle("shown");
      // A file that fails to open now shows why in the window.
      if (shown) void this.host.open(shown.path, shown.line);
    });
    later.addEventListener("click", () => {
      this.settle("dismissed");
    });
    bar.replaceChildren(
      h(
        "div",
        { class: "agent-ask", role: "status" },
        h(
          "span",
          { class: "agent-ask-text" },
          `${ask.agent} wants to show `,
          h("span", { class: "agent-ask-file", title: ask.path }, fileName(ask.path)),
          ...(ask.heading === null ? [] : [` › ${ask.heading}`]),
        ),
        h("span", { class: "agent-ask-actions" }, show, later),
      ),
    );
  }
}

/** A path's file name. */
function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

/** What the document on screen shows: its file, the heading and line at the top, the selection. */
export type ShownView = Omit<WindowView, "ask" | "answers">;

/** What the window's document shows. */
export function view(host: DocHost): ShownView {
  const doc = host.doc();
  const none: ShownView = { path: null, heading: [], line: null, selection: null };
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
): Omit<ShownView, "path"> {
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
