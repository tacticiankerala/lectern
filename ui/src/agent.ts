// An agent's requests to this window (`agent-request`), answered with `replyToAgent`:
// - `status`'s part: what the window shows. That is the file, the heading at the top of the view
//   with the headings it sits under, the source line there, and the reader's selection with its
//   lines; with the request the window is asking the reader about and what became of the earlier
//   ones. A note's or text file's are read from the document; an HTML page's from what its bridge
//   last said (page-frame.ts), with no lines; a PDF tells only its file.
// - `show`: a file opens at once in a window that shows none, or that file already (moving to its
//   target), or when the `agentOpens` setting says to switch. Otherwise the window asks the reader
//   first, with a bar over the top of the document: "Claude wants to show plan.md › Rollout",
//   Show and Not now. One request asks at a time: a newer one takes its place, whatever the newer
//   one does. The reader's own moves leave it, unless they open its file themselves, which shows
//   it. Show takes it off the bar and settles it once the file has opened (shown), or hasn't
//   (failed, with why). The window leaving (its page reloading into another workspace, or the
//   window closing once its unsaved comment text is let go) dismisses it. What became of each
//   request asked about (shown, dismissed, replaced or failed) is told to Rust as an `answered`
//   reply to it, and kept for `status`. Agent opens go on the history, so Back returns, and so
//   does a move within the file on screen. A newer request that opens nothing itself (of the file
//   on screen, or asked about) drops an older one's open still on its way, whatever file it
//   names, which answers `replaced`; the reader's own navigation is left be. A request held while
//   the file on screen is opened again (Rust had taken a dropped open) goes nowhere, failed, once
//   the reader moves on meanwhile, as one on its way does.
// - Targets (`landing`): in a note or a text file, Rust's core found the target and sends where
//   (`places`), so the window goes to its line; to its own text instead (a line's, a quote's,
//   found in its block's text alone, never the whole document's) when that is far down a block
//   longer than the view, while a heading lands by its block. In an HTML page the window finds it
//   in what the page's bridge reported (`pageSpan`): a quote by its text, whitespace aside, told
//   apart from others like it by the text before and after it (256 characters of each at most); a
//   heading by its whole text or its path, in the page's blocks (the outline's list shortens
//   them); the `#anchor` after the page's path by a heading's id, or the slug of its text, as
//   Lectern's links find one. A `show`
//   waits for a page still loading as long as Rust waits for its answer, and so does Show on the
//   bar: a passage that isn't there fails, and one the page hadn't loaded in time for is
//   `pending`, gone to once it has, the page aimed at again should it load afresh meanwhile (a
//   change on disk). A newer request meanwhile (`replaced`), or another file on screen (failed),
//   leaves it going nowhere; so does a newer request for a target the window started with, and a
//   request that ends unreached (dismissed, replaced, or its file failing to open), so the reader
//   opening that page later isn't taken there. In a PDF, its page. `show` only moves there;
//   `highlight` paints.
// - `highlight`: the agent's highlights, on a layer of their own (`agent`) over the comments' and
//   under find's, at most 20, the oldest dropped first, a passage painted already not added again
//   (the count is of distinct passages): in a note or a text file where core found
//   them (else over the blocks on their lines), in an HTML page through its bridge. Each keeps its
//   target: when the text changes (the file shown afresh, a page's text reported again), they are
//   found again, a note's by core, and painted where they are now, never at the old places; one
//   that's gone is dropped. They are never saved, and go when the window shows another file, or
//   the agent clears them. A PDF can't be painted: that fails.
// - `read` of an HTML page: its blocks, as its bridge reported them, and whether it stopped them
//   short at its limit (Rust reads any other file, and reads a page cut short as incomplete).
// - The review tools in an HTML page (Rust writes any other file's sidecar itself): `comment` on a
//   passage found in what the page reported, as `highlight` finds one, its anchor the passage and
//   the text before it, as a selection's; `reply` and `resolve` on a thread Rust found. Each is an
//   operation of the window's comments (comments.ts), saved with the page's text after those before
//   it, the agent its author, so its entry is the agent's (`**Claude (question):**`), never yours.
//   The answer is the thread and its status now.
// - A comment thread as `show`'s target: the Comments tab opens on its card, and the document goes
//   to its passage, once the comments of the file have loaded (`{comment}` landing); with comments
//   hidden in the window, the document goes to its passage alone.
// Loaded on the first request.
import {
  blockLines,
  blocksOnLines,
  buildTextIndex,
  charsBefore,
  leafBlockOf,
  normalize,
  quoteFromRange,
  rangeAt,
  textOnly,
  type TextIndex,
} from "./comments-text";
import type { Passage } from "./doc-view";
import { h, HEADINGS, samePath } from "./dom";
import type { Span } from "./find";
import type { AgentCall } from "./generated/AgentCall";
import type { AgentOpens } from "./generated/AgentOpens";
import type { AgentReply } from "./generated/AgentReply";
import type { AskAnswer } from "./generated/AskAnswer";
import type { AskOutcome } from "./generated/AskOutcome";
import type { ClaudeKind } from "./generated/ClaudeKind";
import type { CommentArgs } from "./generated/CommentArgs";
import type { DocPayload } from "./generated/DocPayload";
import type { HighlightArgs } from "./generated/HighlightArgs";
import type { ReadArgs } from "./generated/ReadArgs";
import type { ReviewOp } from "./generated/ReviewOp";
import type { ReviewPayload } from "./generated/ReviewPayload";
import type { ShowArgs } from "./generated/ShowArgs";
import type { ShowAt } from "./generated/ShowAt";
import type { Target } from "./generated/Target";
import type { TextPlace } from "./generated/TextPlace";
import type { ViewSelection } from "./generated/ViewSelection";
import type { WindowView } from "./generated/WindowView";
import { READING_BAND_PERCENT } from "./outline";
import type { PageFrame } from "./page-frame";

/** The answer to a request a window never gets: Rust answers it. */
export const NOT_HERE = "That request isn't one a window answers.";

/** How many of the reader's answers the window keeps for `status`, the latest: as Rust keeps. */
const ANSWERS_KEPT = 10;

/** The most highlights an agent has in a window; past it, the oldest goes. */
const HIGHLIGHTS_KEPT = 20;

/** Where an agent's highlights paint over the comments' (0 and 1), under find's (3 and 4). */
const HIGHLIGHT_PRIORITY = 2;

/** How much of a quote's context, before it and after it, is compared, at most. */
const CONTEXT_COMPARED = 256;

/** The answer while an HTML page hasn't reported its text yet. */
const PAGE_LOADING = "The page hasn't loaded yet; try again in a moment.";

/** The answer to a highlight in a PDF, which can't be painted. */
const NO_PDF_HIGHLIGHT = "PDFs can't be highlighted; use show with a page target.";

/** The answer to a review tool for a file whose sidecar Rust writes itself. */
const NOT_A_PAGE = "Only an HTML page's comments go through its window.";

/** The closing note of a `resolve` that gives none, as core writes it. */
const RESOLVED = "Resolved.";

/** How long the window has to answer when Rust doesn't say, in milliseconds: as Rust waits. */
const ANSWER_WITHIN_MS = 5000;

/** What a wait for a page keeps of the time to answer in, for the answer to get back. */
const ANSWER_MARGIN_MS = 500;

/** What an HTML page's session gives an agent: its text as its bridge reported it, and paint. */
export type PageText = Pick<
  PageFrame,
  | "text"
  | "blocks"
  | "starts"
  | "headings"
  | "ready"
  | "truncated"
  | "paint"
  | "clear"
  | "headingPath"
  | "selection"
>;

/**
 * Where an open lands in its file, or a move within it goes: a source line, where Rust's core found
 * the target in a note or a text file, with the target's own text there when core found that, gone
 * to instead of its block when it is far down a long one; a PDF's page; a passage of an HTML page,
 * found once the page has reported its text; or a comment thread, by its number, whose card and
 * passage show once the file's comments have loaded.
 */
export type Landing =
  | { line: number; passage?: Passage }
  | { page: number }
  | { aim: (page: PageText) => number | null }
  | { comment: number };

/**
 * Where `target` takes the reader, found by core at `place` in a note or a text file: its line,
 * and a line's or a quote's own text there (`passageIn`); a heading lands by its block. Null for
 * none.
 */
export function landing(target: Target | undefined, place: TextPlace | undefined): Landing | null {
  if (place !== undefined) {
    const { textStart, textEnd, blockOffset } = place;
    return target !== undefined &&
      !("heading" in target) &&
      textStart !== undefined &&
      textEnd !== undefined &&
      blockOffset !== undefined
      ? {
          line: place.startLine,
          passage: passageIn(place.startLine, blockOffset, textEnd - textStart),
        }
      : { line: place.startLine };
  }
  if (target === undefined) {
    return null;
  }
  if ("comment" in target) {
    return { comment: threadNumber(target.comment) };
  }
  if ("line" in target) {
    return { line: target.line };
  }
  if ("page" in target) {
    return { page: target.page };
  }
  return { aim: (page) => pageSpan(page, target)?.start ?? null };
}

/**
 * The passage `length` long, `offset` into the text of the block holding source line `line`,
 * found in that block's own text alone, never the whole document's (a long passage ends with its
 * block). Null when no one block holds the line (a table's row of cells), or the text isn't there.
 */
function passageIn(line: number, offset: number, length: number): Passage {
  return (doc) => {
    const [block, ...others] = blocksOnLines(doc, line, line);
    if (block === undefined || others.length > 0) {
      return null;
    }
    const index = buildTextIndex([block]);
    return rangeAt(index, offset, Math.min(offset + Math.max(1, length), index.text.length));
  };
}

/**
 * One of an agent's highlights: its target, and where it is in the text on screen: in a note or a
 * text file where core found it (`place`), in an HTML page where the window did (`span`). Null
 * while the text has changed and it hasn't been found again.
 */
interface Mark {
  target: Target;
  place: TextPlace | null;
  span: Span | null;
}

/**
 * A `show` whose HTML page target is found once the page has loaded: the request, and the
 * navigation it opened the page in (null until it has). A newer request, or another navigation,
 * leaves it stale: it goes nowhere.
 */
interface Showing {
  id: number;
  navigation: number | null;
}

/** The id of a target the window started with, before any request: a request's is 1 or more. */
const STARTED_WITH = 0;

/**
 * A page's target not reached yet: the request, its page, where it goes, and the showing of the
 * page it was aimed at (null for none yet). A page shown afresh (a reload) is aimed at again.
 */
interface Pending {
  showing: Showing;
  path: string;
  aim: (page: PageText) => number | null;
  doc: DocPayload | null;
}

/** Whether the CSS Custom Highlight API is there to paint with. */
const supported = (): boolean => typeof Highlight === "function" && "highlights" in CSS;

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
  /** The HTML page's session with its bridge, once it has one. */
  page(): PageText | null;
  /** The path on screen: the document's, or the one that failed to open; null for none. */
  shownPath(): string | null;
  /** Whether an agent's open of another file asks the reader first, or switches at once. */
  opens(): AgentOpens;
  /**
   * Opens `path` on the history, at `landing` when given. Once it has landed: null when it is on
   * screen, else why it isn't.
   */
  open(path: string, landing: Landing | null): Promise<string | null>;
  /**
   * Moves within the document on screen to `landing`, marking where the reader was on the history
   * first; false, moving nowhere, for a passage the HTML page on screen hasn't got.
   */
  move(landing: Landing): boolean;
  /**
   * The HTML page on screen once it has reported its text (signed with its open's key), waiting
   * at most `within` milliseconds for it; null when it hasn't by then.
   */
  pageReady(within: number): Promise<PageText | null>;
  /** The current navigation's number: another one opened another document, or this one again. */
  navigation(): number;
  /**
   * Where each of `targets` is now in `path`, the note or text file on screen, as core finds them
   * in the file; null for one that's gone.
   */
  places(path: string, targets: Target[]): Promise<(TextPlace | null)[]>;
  /**
   * Tells Rust what became of the request `id`, which the window asked the reader about, with why
   * when it `failed`; resolves once told.
   */
  answered(id: number, answer: AskAnswer, message?: string): Promise<void>;
  /** Aims the HTML page on screen at a passage, found once it has loaded (a reload's frame too). */
  aimPage(aim: (page: PageText) => number | null): void;
  /**
   * A newer `show` has come: a comment thread an older one asked to show, still waiting to, is
   * dropped, so it never takes the reader back there.
   */
  forgetThreads(): void;
  /**
   * A newer `show` has come that opens nothing itself: an agent's open still on its way is
   * dropped, so it never takes the screen, before this returns (the drop's own navigation is
   * `navigation()` then). The reader's own navigation is left be. Resolves once the file on
   * screen is the one that stays (`recovered`).
   */
  dropOpen(): Promise<void>;
  /**
   * Resolves once the file on screen is the one that stays: opened again in place, should Rust
   * have taken a dropped open already, whichever show dropped it; at once when it isn't being.
   */
  recovered(): Promise<void>;
  /**
   * Saves an agent's `op` to the comments of `path`, the HTML page on screen, with its text, after
   * the operations before it, and shows it: the review as saved, or why it wasn't, as an error.
   */
  review(path: string, op: ReviewOp): Promise<ReviewPayload>;
  /** Where the bar that asks the reader goes. */
  readonly bar: HTMLElement;
}

/** A request to show a file that the window is asking the reader about. */
interface Ask {
  id: number;
  path: string;
  /** The agent's name, as the reader sees it. */
  agent: string;
  /** The heading it is about, as its target names it (or the anchor after a page's path). */
  heading: string | null;
  /** Where the file opens. */
  landing: Landing | null;
  /** An HTML page's passage, found once the page has loaded; null for any other target. */
  inPage: Target | null;
  showing: Showing;
  /** When Rust stops waiting for the reader's answer, in ms since the epoch; 0 when it doesn't. */
  answerBy: number;
}

/** This window's part in answering agents. */
export class Agent {
  /** The request the bar asks about, if any. */
  private asking: Ask | null = null;
  /** What became of the requests asked about, the oldest first. */
  private readonly answers: AskOutcome[] = [];
  /** The agent's highlights in the file `marked`, the oldest first. */
  private marks: Mark[] = [];
  private marked: string | null = null;
  /** The newest `show`'s id: an older one waiting for its page goes nowhere. */
  private lastShow = STARTED_WITH;
  /** A page's target not reached yet, aimed at again should the page load afresh. */
  private pending: Pending | null = null;

  constructor(private readonly host: AgentHost) {}

  /** This window's answer to `call`. */
  async answer(call: AgentCall): Promise<AgentReply> {
    const request = call.request;
    const places = call.places ?? [];
    // When to have answered by, so Rust still waits for it.
    const by = Date.now() + (call.withinMs ?? ANSWER_WITHIN_MS) - ANSWER_MARGIN_MS;
    switch (request.tool) {
      case "status":
        return { kind: "view", ...this.view() };
      case "show":
        return this.show(call.id, call.agent, request, places, by);
      case "highlight":
        return this.highlight(request, places);
      case "read":
        return this.read(request);
      case "comment":
        return this.comment(call.agent, request);
      case "reply":
        return this.onThread(request.path, request.id, {
          text: request.text,
          author: { name: call.agent, kind: request.kind ?? "reply" },
        });
      case "resolve":
        return this.onThread(request.path, request.id, {
          text: request.text ?? RESOLVED,
          author: { name: call.agent, kind: "resolved" },
        });
      default:
        return { kind: "failed", message: NOT_HERE };
    }
  }

  /** What the window shows, with the request it asks about and the answers so far. */
  view(): WindowView {
    return { ...view(this.host), ask: this.asking?.path ?? null, answers: [...this.answers] };
  }

  /**
   * The document on screen changed: the file asked about, opened by the reader, is shown. The
   * agent's highlights go with another file. Shown afresh, the same file's text may have changed:
   * they are taken off and found again, a note's by core (`places`), a page's once it has reported
   * its text (`pageChanged`), and painted where they are now; one that's gone is dropped.
   */
  docShown(): void {
    const doc = this.host.doc();
    if (this.asking && doc && samePath(doc.path, this.asking.path)) {
      void this.settle("shown");
    }
    this.aimAgain(doc);
    if (this.marked === null) {
      return;
    }
    if (!(doc && samePath(doc.path, this.marked))) {
      this.marks = [];
      this.marked = null;
    }
    for (const mark of this.marks) {
      mark.place = null;
      mark.span = null;
    }
    this.paint();
    if (doc && this.marks.length > 0 && (doc.kind === "markdown" || doc.kind === "text")) {
      void this.findAgain(doc);
    }
  }

  /**
   * The page a target is pending in shown afresh (it loaded again, a new frame in the old one's
   * place): it is aimed at again, unless the target has gone stale.
   */
  private aimAgain(doc: DocPayload | null): void {
    const pending = this.pending;
    if (pending === null || doc === null || doc === pending.doc) {
      return;
    }
    if (!this.current(pending.showing) || !samePath(doc.path, pending.path)) {
      this.pending = null;
      return;
    }
    if (doc.kind === "html") {
      pending.doc = doc;
      this.host.aimPage(pending.aim);
    }
  }

  /**
   * Where a window that started for an agent goes in `path`, its first document, at `at`: a note's
   * or a PDF's at once, an HTML page's once it has loaded, unless a newer request, or another
   * navigation than `navigation`, comes first. Null for nowhere.
   */
  startedWith(at: ShowAt, path: string, navigation: number): Landing | null {
    const target = at.target;
    if (at.place !== undefined || !foundInPage(target)) {
      return landing(target, at.place);
    }
    return this.pageLanding(target, { id: STARTED_WITH, navigation }, path);
  }

  /**
   * The HTML page on screen reported its text, maybe changed: the agent's highlights are found in
   * it again and painted where they are now, and one that's gone is dropped.
   */
  pageChanged(): void {
    const page = this.host.page();
    if (this.marks.length === 0 || this.host.doc()?.kind !== "html" || !page?.ready) {
      return;
    }
    this.marks = this.marks.flatMap((mark) => {
      const span = pageSpan(page, mark.target);
      return span === null ? [] : [{ ...mark, span }];
    });
    this.paint();
  }

  /**
   * Finds the highlights in the note `doc`, shown afresh, again through core, and paints them,
   * unless another document, or another showing of it, came meanwhile; one that's gone is dropped.
   */
  private async findAgain(doc: DocPayload): Promise<void> {
    const marks = [...this.marks];
    let places: (TextPlace | null)[];
    try {
      places = await this.host.places(
        doc.path,
        marks.map((m) => m.target),
      );
    } catch {
      places = marks.map(() => null);
    }
    if (this.host.doc() !== doc) {
      return;
    }
    const found = new Map(marks.map((mark, i) => [mark, places[i] ?? null]));
    this.marks = this.marks.flatMap((mark) => {
      const place = found.get(mark);
      if (place === undefined) return [mark];
      return place === null ? [] : [{ ...mark, place }];
    });
    this.paint();
  }

  /**
   * The window is leaving: its page reloads into another workspace, or it closes. The request it
   * asks about, if any, is dismissed; resolves once Rust is told.
   */
  leave(): Promise<void> {
    return this.settle("dismissed");
  }

  /**
   * `show` from the agent `agent`, the request `id`, at its target, which core found at `places`
   * in a note or a text file: on screen at once, or asked about first (see the top of this file).
   * An HTML page's target is found once the page has loaded, waited for until `by`: a passage
   * that isn't there fails, and one the page hadn't loaded in time for is `pending`, gone to once
   * it has.
   */
  private async show(
    id: number,
    agent: string,
    args: ShowArgs,
    places: TextPlace[],
    by: number,
  ): Promise<AgentReply> {
    // A newer request: an older one's page target, or thread, goes nowhere.
    this.lastShow = id;
    this.host.forgetThreads();
    const target = args.target;
    const showing: Showing = { id, navigation: null };
    const inPage =
      target !== undefined && places.length === 0 && foundInPage(target) ? target : null;
    const land = inPage ? this.pageLanding(inPage, showing, args.path) : landing(target, places[0]);
    void this.settle("replaced");
    // The file on screen stays unless this opens another: an older request's open on its way is
    // dropped. Either way, should Rust have taken a dropped open already, the file on screen opens
    // again first, so the target goes to the document that stays.
    const settling =
      this.route(args.path) === "open" ? this.host.recovered() : this.host.dropOpen();
    // The drop has made its own navigation by now: any other from here on is the reader's.
    const navigation = this.host.navigation();
    await settling;
    if (this.lastShow !== id) {
      this.ended(showing);
      return { kind: "replaced" };
    }
    if (this.host.navigation() !== navigation) {
      // The reader moved on meanwhile: this goes nowhere, as a show on its way does.
      this.ended(showing);
      return { kind: "failed", message: notOnScreen(args.path) };
    }
    const route = this.route(args.path);
    const doc = this.host.doc();
    if (route === "here" && doc) {
      if (inPage && land && doc.kind === "html") {
        showing.navigation = this.host.navigation();
        return this.reach(showing, args.path, inPage, land, by, true);
      }
      if (land !== null && !this.host.move(land)) {
        return { kind: "failed", message: notThere(doc.path) };
      }
      return { kind: "shown" };
    }
    if (route === "open") {
      const opened = await this.open(args.path, land);
      if (!inPage || !land || opened.kind !== "shown" || !this.pageShown()) {
        // Not on its way to a page's passage: a failed open's target goes nowhere, later either.
        this.ended(showing);
        // One a newer request dropped on its way was replaced by it.
        return opened.kind === "failed" && this.lastShow !== id ? { kind: "replaced" } : opened;
      }
      showing.navigation = this.host.navigation();
      this.aimed(showing, land);
      return this.reach(showing, args.path, inPage, land, by, false);
    }
    const heading =
      target === undefined
        ? null
        : "heading" in target
          ? target.heading
          : "anchor" in target
            ? target.anchor
            : null;
    // Rust waits for the reader's answer `wait` seconds, when it waits.
    const answerBy = args.wait > 0 ? Date.now() + args.wait * 1000 : 0;
    this.asking = { id, path: args.path, agent, heading, landing: land, inPage, showing, answerBy };
    this.renderBar();
    return { kind: "asked" };
  }

  /**
   * How a `show` of `path` takes the reader there: `here`, the document on screen already; `open`,
   * opening it at once (the window shows nothing, or that path failed to open, or the setting is
   * to switch); else `ask`, the reader first.
   */
  private route(path: string): "here" | "open" | "ask" {
    const doc = this.host.doc();
    if (doc && samePath(doc.path, path)) {
      return "here";
    }
    const shown = this.host.shownPath();
    return shown === null || samePath(shown, path) || this.host.opens() === "switch"
      ? "open"
      : "ask";
  }

  /**
   * Where `showing` goes in its HTML page `path`: `target`, found once the page has loaded, unless
   * stale; it is pending until then (`pending`), and reached once found.
   */
  private pageLanding(target: Target, showing: Showing, path: string): Landing {
    const aim = (page: PageText): number | null => {
      const at = this.current(showing) ? (pageSpan(page, target)?.start ?? null) : null;
      if (at !== null && this.pending?.aim === aim) {
        this.pending = null;
      }
      return at;
    };
    this.pending = { showing, path, aim, doc: null };
    return { aim };
  }

  /** `showing`'s page, on screen now, was aimed at `land`: aimed again should it load afresh. */
  private aimed(showing: Showing, land: Landing): void {
    const pending = this.pending;
    if (pending?.showing === showing && "aim" in land && pending.aim === land.aim) {
      pending.doc = this.host.doc();
    }
  }

  /**
   * `showing` came to an end: reached, failed, replaced or dismissed. Nothing is aimed at for it
   * again.
   */
  private ended(showing: Showing): void {
    if (this.pending?.showing === showing) {
      this.pending = null;
    }
  }

  /** Whether the document on screen is an HTML page, whose passages the window finds itself. */
  private pageShown(): boolean {
    return this.host.doc()?.kind === "html";
  }

  /** Whether `showing` is still the request to go to: no newer one came, nor another navigation. */
  private current(showing: Showing): boolean {
    return (
      this.lastShow === showing.id &&
      (showing.navigation === null || this.host.navigation() === showing.navigation)
    );
  }

  /**
   * Whether `target` is in the HTML page `path` on screen, once it has loaded, waited for until
   * `by`, going there (`land`, by a move when `move`; an open's page goes there itself as it
   * loads): shown, failed when it isn't there, or pending when the page hadn't loaded by then,
   * going there once it has. A newer request meanwhile replaces it, and another file on screen
   * fails it: either way it goes nowhere.
   */
  private async reach(
    showing: Showing,
    path: string,
    target: Target,
    land: Landing,
    by: number,
    move: boolean,
  ): Promise<AgentReply> {
    const page = await this.host.pageReady(Math.max(0, by - Date.now()));
    if (this.lastShow !== showing.id) {
      this.ended(showing);
      return { kind: "replaced" };
    }
    const doc = this.host.doc();
    if (!this.current(showing) || !doc || !samePath(doc.path, path)) {
      this.ended(showing);
      return { kind: "failed", message: notOnScreen(path) };
    }
    if (page !== null && pageSpan(page, target) === null) {
      this.ended(showing);
      return { kind: "failed", message: notThere(path) };
    }
    if (move) {
      this.host.move(land);
    }
    if (page === null) {
      // Not loaded yet: it goes there once it has, the page aimed at again should it reload.
      this.aimed(showing, land);
      return { kind: "shown", target: "pending" };
    }
    this.ended(showing);
    return { kind: "shown" };
  }

  /** Opens `path` at `land`: shown, or failed with why. */
  private async open(path: string, land: Landing | null): Promise<AgentReply> {
    const failure = await this.host.open(path, land);
    return failure === null
      ? { kind: "shown" }
      : { kind: "failed", message: didNotOpen(path, failure) };
  }

  /**
   * `highlight`: the agent's highlights in the file on screen, its earlier ones cleared first when
   * `clear`, each target where core found it (`places`) in a note or a text file, or found in an
   * HTML page's text, all of them or none; one on a passage painted already isn't added again. A
   * PDF can't be painted. Answers how many the window shows now.
   */
  private highlight(args: HighlightArgs, places: TextPlace[]): AgentReply {
    const doc = this.host.doc();
    const path = args.path ?? doc?.path ?? "";
    if (!doc || !samePath(doc.path, path)) {
      return { kind: "failed", message: notShown(path) };
    }
    let added: Mark[] = [];
    switch (doc.kind) {
      case "markdown":
      case "text":
        added = args.targets.flatMap((target, i) => {
          const place = places[i];
          return place ? [{ target, place, span: null }] : [];
        });
        break;
      case "html": {
        if (args.targets.length === 0) break;
        const page = this.host.page();
        if (!page?.ready) {
          return { kind: "failed", message: PAGE_LOADING };
        }
        for (const target of args.targets) {
          const span = pageSpan(page, target);
          if (span === null) {
            return { kind: "failed", message: notThere(doc.path) };
          }
          added.push({ target, place: null, span });
        }
        break;
      }
      case "pdf":
        return { kind: "failed", message: NO_PDF_HIGHLIGHT };
    }
    if (args.clear || !samePath(this.marked ?? "", doc.path)) {
      this.marks = [];
    }
    this.marked = doc.path;
    // A passage painted already isn't painted twice, so a retry changes nothing.
    for (const mark of added) {
      if (!this.marks.some((m) => samePlace(m, mark))) this.marks.push(mark);
    }
    this.marks.splice(0, Math.max(0, this.marks.length - HIGHLIGHTS_KEPT));
    this.paint();
    return { kind: "highlighted", count: this.marks.length };
  }

  /**
   * `comment` from the agent `agent` in the HTML page on screen: a thread on its passage, found in
   * what the page reported, anchored as a selection of it is, the agent's entry first.
   */
  private comment(agent: string, args: CommentArgs): Promise<AgentReply> {
    const shown = this.pageOnScreen(args.path);
    if ("kind" in shown) {
      return Promise.resolve(shown);
    }
    const { path, page } = shown;
    const span = pageSpan(page, args.target);
    if (span === null) {
      return Promise.resolve({ kind: "failed", message: notThere(path) });
    }
    // Where it was found too, so core anchors it on that occurrence, whatever the text before it
    // shares with another.
    const anchor = {
      startLine: 0,
      endLine: 0,
      quote: normalize(page.text.slice(span.start, span.end)),
      prefix: charsBefore(page.text, span.start),
      textStart: span.start,
    };
    const author = { name: agent, kind: args.kind ?? "question" };
    return this.saved(path, { op: "add", anchor, text: args.text, author });
  }

  /** `reply` or `resolve` in the HTML page on screen: `entry`, the agent's, on thread `id` (`C3`). */
  private onThread(
    path: string,
    id: string,
    entry: { text: string; author: { name: string; kind: ClaudeKind } },
  ): Promise<AgentReply> {
    const shown = this.pageOnScreen(path);
    if ("kind" in shown) {
      return Promise.resolve(shown);
    }
    return this.saved(shown.path, { op: "reply", id: threadNumber(id), ...entry });
  }

  /**
   * The HTML page `path` on screen, once it has reported its text, with its path as the window
   * opened it; else why an agent's review tool can't work on it.
   */
  private pageOnScreen(path: string): { path: string; page: PageText } | AgentReply {
    const doc = this.host.doc();
    if (!doc || !samePath(doc.path, path)) {
      return { kind: "failed", message: notShown(path) };
    }
    if (doc.kind !== "html") {
      return { kind: "failed", message: NOT_A_PAGE };
    }
    const page = this.host.page();
    if (!page?.ready) {
      return { kind: "failed", message: PAGE_LOADING };
    }
    return { path: doc.path, page };
  }

  /** Saves the agent's `op` to the comments of the page `path`: the thread, and its status now. */
  private async saved(path: string, op: ReviewOp): Promise<AgentReply> {
    let review: ReviewPayload;
    try {
      review = await this.host.review(path, op);
    } catch (e) {
      return { kind: "failed", message: e instanceof Error ? e.message : String(e) };
    }
    const thread = review.comments.find((c) => c.id === review.changed);
    return thread
      ? { kind: "commented", id: thread.id, status: thread.status }
      : { kind: "failed", message: "The comment wasn't saved." };
  }

  /** `read` of the HTML page on screen: its blocks, once it has reported them. */
  private read(args: ReadArgs): AgentReply {
    const doc = this.host.doc();
    const path = args.path ?? doc?.path ?? "";
    if (!doc || !samePath(doc.path, path)) {
      return { kind: "failed", message: notShown(path) };
    }
    if (doc.kind !== "html") {
      return { kind: "failed", message: "Only an HTML page's text comes from its window." };
    }
    const page = this.host.page();
    if (!page?.ready) {
      return { kind: "failed", message: PAGE_LOADING };
    }
    // Cut short at the limit on the text the page reports: Rust reads it as incomplete.
    return page.truncated
      ? { kind: "pageText", blocks: page.blocks, truncated: true }
      : { kind: "pageText", blocks: page.blocks };
  }

  /**
   * Paints the agent's highlights in the document on screen: a note's or a text file's on the
   * `agent` layer, an HTML page's through its bridge. None clears the layer.
   */
  private paint(): void {
    const doc = this.host.doc();
    if (doc?.kind === "html") {
      const page = this.host.page();
      const spans = this.marks.flatMap((m) =>
        m.span !== null && m.span.end <= (page?.text.length ?? 0) ? [m.span] : [],
      );
      if (spans.length > 0) {
        page?.paint("agent", spans);
      } else {
        page?.clear("agent");
      }
    }
    if (!supported()) {
      return;
    }
    const el = this.host.docElement();
    const notes = doc?.kind === "markdown" || doc?.kind === "text";
    const places = notes ? this.marks.flatMap((m) => (m.place ? [m.place] : [])) : [];
    const index = places.some((p) => p.textStart !== undefined) ? buildTextIndex([el]) : null;
    const ranges = places.flatMap((place) => rangesOf(place, el, index));
    if (ranges.length === 0) {
      CSS.highlights.delete("agent");
      return;
    }
    const highlight = new Highlight(...ranges);
    highlight.priority = HIGHLIGHT_PRIORITY;
    CSS.highlights.set("agent", highlight);
  }

  /**
   * Show, on the bar: the request comes off it, and settles once its file has opened (shown), or
   * hasn't (failed, with why, which the window shows too). An HTML page's passage settles as an
   * immediate show's does, once the page has loaded, waited for while Rust still waits for the
   * answer, else as long as an immediate show waits: failed when it isn't there, replaced when a
   * newer request came meanwhile.
   */
  private async showAsked(): Promise<void> {
    const ask = this.take();
    if (ask === null) {
      return;
    }
    const failure = await this.host.open(ask.path, ask.landing);
    if (failure !== null) {
      // Dropped on its way by a newer request, it was replaced.
      await (this.lastShow === ask.id
        ? this.record(ask, "failed", didNotOpen(ask.path, failure))
        : this.record(ask, "replaced"));
      return;
    }
    if (ask.inPage === null || ask.landing === null || !this.pageShown()) {
      await this.record(ask, "shown");
      return;
    }
    ask.showing.navigation = this.host.navigation();
    this.aimed(ask.showing, ask.landing);
    const by = Math.max(ask.answerBy, Date.now() + ANSWER_WITHIN_MS) - ANSWER_MARGIN_MS;
    const reply = await this.reach(ask.showing, ask.path, ask.inPage, ask.landing, by, false);
    // `reach` ended the target, unless it is still on its way to the page once it loads.
    await (reply.kind === "failed"
      ? this.record(ask, "failed", reply.message, true)
      : this.record(ask, reply.kind === "replaced" ? "replaced" : "shown", undefined, true));
  }

  /** The request being asked about, if any, settles as `answer` (see `take` and `record`). */
  private settle(answer: AskAnswer): Promise<void> {
    const ask = this.take();
    return ask === null ? Promise.resolve() : this.record(ask, answer);
  }

  /** The request being asked about, taken off the bar, which goes; null for none. */
  private take(): Ask | null {
    const ask = this.asking;
    if (ask !== null) {
      this.asking = null;
      this.renderBar();
    }
    return ask;
  }

  /**
   * What became of `ask`, with why when it `failed`: `status` keeps it, and Rust is told; resolves
   * once it is. Its page's passage, if any, ends with it, so the reader opening that page later
   * isn't taken there; unless Show `reached` for it, which has settled it already, leaving it on
   * its way to a page still loading.
   */
  private record(ask: Ask, answer: AskAnswer, message?: string, reached = false): Promise<void> {
    if (!reached) {
      this.ended(ask.showing);
    }
    const outcome: AskOutcome = { path: ask.path, answer };
    if (message !== undefined) outcome.message = message;
    this.answers.push(outcome);
    this.answers.splice(0, Math.max(0, this.answers.length - ANSWERS_KEPT));
    return this.host.answered(ask.id, answer, message);
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
      void this.showAsked();
    });
    later.addEventListener("click", () => {
      void this.settle("dismissed");
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

/** Whether two highlights cover the same passage: where core found them, or the window did. */
function samePlace(a: Mark, b: Mark): boolean {
  if (a.span !== null && b.span !== null) {
    return a.span.start === b.span.start && a.span.end === b.span.end;
  }
  return (
    a.place !== null &&
    b.place !== null &&
    a.place.startLine === b.place.startLine &&
    a.place.endLine === b.place.endLine &&
    a.place.textStart === b.place.textStart &&
    a.place.textEnd === b.place.textEnd
  );
}

/** The number of the comment thread `id` names: `C3` is 3. */
function threadNumber(id: string): number {
  return Number(id.slice(1));
}

/** A path's file name. */
function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

/** Why `path` didn't open, `failure`, as the agent hears it. */
function didNotOpen(path: string, failure: string): string {
  return `${fileName(path)} didn't open: ${failure}`;
}

/** The answer when `path` isn't on screen, in the words Rust uses. */
function notShown(path: string): string {
  return `${fileName(path)} isn't shown in Lectern. Show it first.`;
}

/** The answer when another file took the place of `path` before its page loaded. */
function notOnScreen(path: string): string {
  return `${fileName(path)} isn't on screen any more: another file took its place.`;
}

/** The answer when a target isn't in `path`, in the words Rust uses. */
function notThere(path: string): string {
  return `That passage isn't in ${fileName(path)}.`;
}

/**
 * The ranges of the document `el` that `place` covers: its text, where core found it (`index`, the
 * text of `el`); else the blocks on its lines, each whole.
 */
function rangesOf(place: TextPlace, el: HTMLElement, index: TextIndex | null): Range[] {
  if (index !== null && place.textStart !== undefined && place.textEnd !== undefined) {
    const range = rangeAt(index, place.textStart, place.textEnd);
    if (range !== null) return [range];
  }
  return blocksOnLines(el, place.startLine, place.endLine).map((block) => {
    const range = document.createRange();
    range.selectNodeContents(block);
    return range;
  });
}

/** Whether `target` is one an HTML page's window finds itself: a quote, a heading or an anchor. */
function foundInPage(target: Target): boolean {
  return "quote" in target || "heading" in target || "anchor" in target;
}

/**
 * Where `target` is in an HTML page's text, as its bridge reported it; null when it isn't there.
 * A quote by its text, whitespace aside, and of two or more, the one the text just before and after
 * it agrees with most, then the first. A heading by its text, or a path of headings each under the
 * one before (`Report › Results`, with headings between left out), in any letter case: of those
 * that match, the one whose own path is nearest the target's length, then the first. An anchor by
 * a heading's id, or the slug of its text (`anchorSpan`). Nothing else is in a page.
 */
export function pageSpan(
  page: Pick<PageText, "text" | "blocks" | "starts" | "headings">,
  target: Target,
): Span | null {
  if ("quote" in target) {
    return quoteSpan(page.text, target.quote, target.prefix ?? "", target.suffix ?? "");
  }
  if ("heading" in target) {
    return headingSpan(page, target.heading);
  }
  if ("anchor" in target) {
    return anchorSpan(page, target.anchor);
  }
  return null;
}

/**
 * The occurrence of `quote` in `text` that agrees most with `prefix` and `suffix`, then the first.
 * Only the `CONTEXT_COMPARED` characters of each next to the quote count, and the first occurrence
 * that agrees with all of them is taken at once, so a long context on a page of repeated text
 * costs little.
 */
function quoteSpan(text: string, quote: string, prefix: string, suffix: string): Span | null {
  const needle = normalize(quote);
  if (needle === "") {
    return null;
  }
  const before = normalize(prefix).slice(-CONTEXT_COMPARED);
  const after = normalize(suffix).slice(0, CONTEXT_COMPARED);
  const whole = before.length + after.length;
  let best: { span: Span; agreement: number } | null = null;
  for (let at = text.indexOf(needle); at !== -1; at = text.indexOf(needle, at + 1)) {
    const end = at + needle.length;
    const agreement = agreesBefore(text, at, before) + agreesAfter(text, end, after);
    if (best === null || agreement > best.agreement) {
      best = { span: { start: at, end }, agreement };
      if (agreement === whole) break;
    }
  }
  return best?.span ?? null;
}

/** How many characters of `text` just before `at`, spaces next to it aside, end as `wanted` does. */
function agreesBefore(text: string, at: number, wanted: string): number {
  let i = at;
  while (i > 0 && text[i - 1] === " ") i--;
  let n = 0;
  while (n < wanted.length && i - n > 0 && text[i - n - 1] === wanted[wanted.length - n - 1]) n++;
  return n;
}

/** How many characters of `text` just after `at`, spaces next to it aside, start as `wanted` does. */
function agreesAfter(text: string, at: number, wanted: string): number {
  let i = at;
  while (i < text.length && text[i] === " ") i++;
  let n = 0;
  while (n < wanted.length && i + n < text.length && text[i + n] === wanted[n]) n++;
  return n;
}

/** A heading's text as a target is matched against it: as core matches one. */
function headingKey(heading: string): string {
  return normalize(heading.trim().replace(/^#+/, "").replace(/[*_`]/g, "")).toLowerCase();
}

/**
 * The span of the block of the heading `wanted` names in a page (see `pageSpan`): matched on the
 * page's blocks, whole headings at their levels, not the outline's list, which shortens them.
 */
function headingSpan(page: Pick<PageText, "blocks" | "starts">, wanted: string): Span | null {
  const segments = wanted
    .split("›")
    .map(headingKey)
    .filter((s) => s !== "");
  const last = segments.pop();
  if (last === undefined) {
    return null;
  }
  let best: { block: number; extra: number } | null = null;
  const path: { level: number; key: string }[] = [];
  for (const [i, block] of page.blocks.entries()) {
    const level = block.headingLevel;
    if (level === undefined) continue;
    while (path.length > 0 && (path[path.length - 1]?.level ?? 0) >= level) path.pop();
    const key = headingKey(block.text);
    if (
      key === last &&
      inOrder(
        segments,
        path.map((p) => p.key),
      )
    ) {
      const extra = path.length - segments.length;
      if (best === null || extra < best.extra) best = { block: i, extra };
    }
    path.push({ level, key });
  }
  if (best === null) {
    return null;
  }
  const { block } = best;
  const start = page.starts[block] ?? 0;
  return { start, end: start + (page.blocks[block]?.text.length ?? 0) };
}

/**
 * The span of the heading block an `#anchor` (after the page's path in a `show`) names in a page,
 * as Lectern's links find one: the anchor as written, then percent-decoded, then its slug, each
 * tried against the headings' ids (the page's own, else the bridge's made-up ones, for those the
 * outline lists) and then the slugs of their text, GitHub's, a repeat numbered `-1`, `-2`, … as
 * a note's heading ids are. Null when no heading has it.
 */
function anchorSpan(
  page: Pick<PageText, "blocks" | "starts" | "headings">,
  anchor: string,
): Span | null {
  const ids = new Map(page.headings.map((h) => [h.offset, h.id]));
  const slugs = new Slugs();
  const named = page.blocks.flatMap((block, i) => {
    if (block.headingLevel === undefined) return [];
    const start = page.starts[i] ?? 0;
    const span = { start, end: start + block.text.length };
    return [{ span, id: ids.get(start) ?? "", slug: slugs.next(block.text) }];
  });
  const decoded = decodeAnchor(anchor);
  for (const wanted of [anchor, decoded, slugify(decoded)]) {
    const found =
      named.find((h) => h.id !== "" && h.id === wanted) ??
      named.find((h) => h.slug !== "" && h.slug === wanted);
    if (found) return found.span;
  }
  return null;
}

/** `anchor` percent-decoded; as written when it isn't valid encoding. */
function decodeAnchor(anchor: string): string {
  try {
    return decodeURIComponent(anchor);
  } catch {
    return anchor;
  }
}

/**
 * A heading's text as a GitHub-style id, as core's `slugify` makes one: lower case, its letters,
 * marks, numbers and connector punctuation (`_`) kept, each space and `-` a `-`, all else dropped.
 */
function slugify(text: string): string {
  let slug = "";
  for (const c of text.toLowerCase()) {
    if (c === " " || c === "-") slug += "-";
    else if (/[\p{L}\p{M}\p{N}\p{Pc}]/u.test(c)) slug += c;
  }
  return slug;
}

/** Hands out headings' slugs in order, a repeat with `-1`, `-2`, … as core's `Slugger` does. */
class Slugs {
  private readonly seen = new Map<string, number>();

  next(text: string): string {
    const base = slugify(text);
    let slug = base;
    while (this.seen.has(slug)) {
      const n = (this.seen.get(base) ?? 0) + 1;
      this.seen.set(base, n);
      slug = `${base}-${String(n)}`;
    }
    this.seen.set(slug, 0);
    return slug;
  }
}

/** Whether each of `wanted` is in `path`, in the same order, others between them or not. */
function inOrder(wanted: string[], path: string[]): boolean {
  let at = 0;
  return wanted.every((w) => {
    while (at < path.length && path[at] !== w) at++;
    return at++ < path.length;
  });
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
