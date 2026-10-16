// The frame an HTML page or a PDF shows in: it fills the document pane in place of the document
// (`#lx-doc-pane.framed`). The frame is in the page before its `src` is set, as WebView2 gives the
// first document the frame's colour scheme only then. Every show loads a new URL (`v=<n>` in its
// query): only a reload, never a change of hash, takes a PDF to a page or shows a change on disk.
// A PDF opens at its first page, or the one asked for (`#page=<n>`), and goes to another page by
// loading again.
//
// A PDF's frame has no sandbox, which WebView2's viewer won't run in. A page's always has one that
// allows scripts and never the same origin, and every response of the page protocol but a PDF's
// carries a sandboxing CSP of its own. A page's frame follows Lectern's light or dark theme, and
// its bridge talks to Lectern through page-frame.ts, loaded on first use: only messages its own
// window posts are taken, held until that module is there, as are a passage to find, an anchor or a
// saved place to go to once the page is ready (its own saved place unless something else is asked
// for). Above a page, a notice names what it loads from the web.
//
// Only messages signed with the open's key (`bridgeKey`) are taken: Rust puts it in the bridge of
// the open's own page alone, so no page script can forge one, and no page the frame navigates to
// has it. Every `load` of the frame must be paired with a signed `ready` and a signed `loaded`, the
// bridge's own load of that document, each before it or within 2 seconds after it, in either order.
// A load without them is another page in the frame (a script, `<meta http-equiv=refresh>`, a link
// the page kept from its bridge, or a page that left before its own load): its session ends, and
// the page Lectern meant to show is opened again with a fresh token, through the navigation queue,
// so a navigation the reader started wins. A second within 10 seconds stops the loop: the pane
// says so instead, with the page's default app a click away. A page that loads itself again sends
// its `ready` and `loaded` again, and is read again.
//
// The keyboard focus inside a frame is the frame's own, so Rust is told whenever it goes in or out
// (`trackFrameFocus`): only Rust sees the keys pressed there.
import { samePath } from "./dom";
import type { DocPayload } from "./generated/DocPayload";
import type { FrameFocus } from "./generated/FrameFocus";
import type { PageLoads } from "./generated/PageLoads";
import type { PageAim, PageFrame, PageHost } from "./page-frame";

/** Where a PDF opens unless asked otherwise: no reading position is kept in one. */
const PDF_START = 1;
/** A page that navigates its frame again within this long, in milliseconds, is let go. */
const NAVIGATION_LOOP_MS = 10_000;
/** How long a load of the frame waits for its page's signed `ready` and `loaded`, in ms. */
const READY_WITHIN_MS = 2000;

/** What a frame needs from the app. */
export interface FrameHost extends PageHost {
  /** Whether Lectern's theme is a dark one: a page's frame takes that colour scheme. */
  dark(): boolean;
  /** A frame came or went, so the keyboard focus may have moved out of one. */
  focusMoved(): void;
  /** The page at `path` navigated its frame: it is to be opened again, as Lectern meant. */
  navigatedAway(path: string): void;
  /** The page at `path` keeps navigating its frame: the pane says so instead. */
  keepsNavigatingAway(path: string): void;
}

/** `url` with `name=value` added to its query. */
function withParam(url: string, name: string, value: string): string {
  const hash = url.indexOf("#");
  const base = hash < 0 ? url : url.slice(0, hash);
  const rest = hash < 0 ? "" : url.slice(hash);
  return `${base}${base.includes("?") ? "&" : "?"}${name}=${value}${rest}`;
}

export class FrameView {
  private frame: HTMLIFrameElement | null = null;
  private notice: HTMLElement | null = null;
  /** Bumped by every show, and every PDF page gone to, so each loads a new URL. */
  private version = 0;
  /** The URL showing, before its `v` and its page. */
  private url: string | null = null;
  /** The page's session with its bridge, once its module has loaded. */
  private session: PageFrame | null = null;
  /** What arrived for a page before its session: its bridge's messages and reports of loads. */
  private held: { messages: unknown[]; loads: PageLoads[] } = { messages: [], loads: [] };
  /** Where to take a page once it is ready, asked for before its session was there. */
  private start: { fraction: number | null; anchor: string | null; aim?: PageAim } = {
    fraction: null,
    anchor: null,
  };
  /** The page showing: what is opened again should it navigate its frame. */
  private pagePath: string | null = null;
  /** The key its bridge signs its messages with; none for a PDF. */
  private key: string | null = null;
  /** A signed `ready` came that no load of the frame has been paired with yet. */
  private readySeen = false;
  /** A signed `loaded` (the bridge's own load) came, not yet paired with a load of the frame. */
  private loadedSeen = false;
  /** A load of the frame waits for its signed `ready` and `loaded`. */
  private loadWaiting = false;
  /** How long it waits. */
  private readyTimer: ReturnType<typeof setTimeout> | null = null;
  /** The page navigated its frame: nothing more is taken from it. */
  private away = false;
  /** The last page that navigated its frame, and when, for the loop guard. */
  private lastAway: { path: string; at: number } | null = null;

  constructor(
    private readonly pane: HTMLElement,
    private readonly host: FrameHost,
  ) {
    window.addEventListener("message", (e) => {
      this.received(e);
    });
  }

  /** Whether a frame is showing. */
  get shown(): boolean {
    return this.frame !== null;
  }

  /** The HTML page showing, once its session has started; null for anything else. */
  get page(): PageFrame | null {
    return this.session;
  }

  /** Shows `doc`, a page or a PDF (at `pdfPage`), from `url` (its `frameUrl`) in a new frame. */
  show(doc: DocPayload, url: string, pdfPage = PDF_START): void {
    this.close();
    const page = doc.kind !== "pdf";
    const frame = document.createElement("iframe");
    frame.className = "lx-frame";
    frame.title = doc.title;
    frame.dataset.kind = page ? "page" : "pdf";
    this.pane.classList.add("framed");
    if (page) {
      frame.setAttribute("sandbox", "allow-scripts");
      // Before it loads, so its first document gets it too.
      frame.style.colorScheme = this.host.dark() ? "dark" : "light";
      const notice = document.createElement("div");
      notice.className = "page-notice";
      notice.setAttribute("role", "status");
      notice.hidden = true;
      this.pane.append(notice);
      this.notice = notice;
    }
    this.pane.append(frame);
    this.frame = frame;
    this.pagePath = page ? doc.path : null;
    this.key = page ? doc.bridgeKey : null;
    // Where the reader left it, unless something else is asked for.
    this.start = { fraction: page ? (doc.position?.fraction ?? null) : null, anchor: null };
    this.url = url;
    this.version += 1;
    const src = withParam(url, "v", String(this.version));
    frame.src = page ? src : `${src}#page=${String(pdfPage)}`;
    if (page) {
      frame.addEventListener("load", () => {
        this.loaded(frame);
      });
      this.startPage(doc, frame);
    }
  }

  /** Takes the frame away, so the document shows again. */
  close(): void {
    const had = this.frame !== null;
    this.frame?.remove();
    this.notice?.remove();
    this.session?.dispose();
    this.frame = null;
    this.notice = null;
    this.session = null;
    this.held = { messages: [], loads: [] };
    this.start = { fraction: null, anchor: null };
    this.url = null;
    this.pagePath = null;
    this.key = null;
    this.away = false;
    this.readySeen = false;
    this.loadedSeen = false;
    this.loadWaiting = false;
    if (this.readyTimer !== null) {
      clearTimeout(this.readyTimer);
      this.readyTimer = null;
    }
    this.pane.classList.remove("framed");
    if (had) {
      this.host.focusMoved();
    }
  }

  /** Takes the PDF showing to its page `page`, loading it again; nothing for anything else. */
  toPdfPage(page: number): void {
    const frame = this.frame;
    if (frame?.dataset.kind !== "pdf" || this.url === null) {
      return;
    }
    this.version += 1;
    frame.src = `${withParam(this.url, "v", String(this.version))}#page=${String(page)}`;
  }

  /**
   * Scrolls a page to the passage `aim` finds once it is ready, over its anchor or saved place;
   * false when the ready page hasn't got it, or nothing showing is a page.
   */
  aim(aim: PageAim): boolean {
    if (this.session) {
      return this.session.aim(aim);
    }
    if (this.frame?.dataset.kind !== "page") {
      return false;
    }
    this.start = { ...this.start, aim };
    return true;
  }

  /** Scrolls a page to `fraction` of its height once it is ready. */
  restore(fraction: number): void {
    if (this.session) {
      this.session.restore(fraction);
    } else if (this.frame?.dataset.kind === "page") {
      this.start = { fraction, anchor: null };
    }
  }

  /**
   * Scrolls a page to its heading `id` once it is ready, over its saved place; false when the
   * ready page has no such heading, or nothing showing is a page.
   */
  scrollToAnchor(id: string): boolean {
    if (this.session) {
      return this.session.scrollToAnchor(id);
    }
    if (this.frame?.dataset.kind !== "page") {
      return false;
    }
    this.start = { ...this.start, anchor: id };
    return true;
  }

  /** What a page has loaded from the web, as Rust heard from the browser (`page-loads`). */
  loads(loads: PageLoads): void {
    if (this.session) {
      this.session.loads(loads);
    } else if (this.frame?.dataset.kind === "page") {
      this.held.loads.push(loads);
    }
  }

  /** The theme changed: a page's frame takes its colour scheme, and the page its colours. */
  syncTheme(): void {
    if (this.frame?.dataset.kind === "page") {
      this.frame.style.colorScheme = this.host.dark() ? "dark" : "light";
      this.session?.sendTheme();
    }
  }

  /** Starts `doc`'s session with its bridge once page-frame.ts has loaded, unless it has gone. */
  private startPage(doc: DocPayload, frame: HTMLIFrameElement): void {
    const notice = this.notice;
    void import("./page-frame.js").then(
      ({ PageFrame }) => {
        if (this.frame !== frame || notice === null) {
          return;
        }
        const session = new PageFrame(frame, notice, doc, this.host, this.start);
        const { messages, loads } = this.held;
        this.session = session;
        this.held = { messages: [], loads: [] };
        this.start = { fraction: null, anchor: null };
        for (const message of messages) session.receive(message);
        for (const report of loads) session.loads(report);
      },
      (e: unknown) => {
        console.warn(e);
      },
    );
  }

  /**
   * `frame` loaded a document: the open's page when its signed `ready` and `loaded` came first, or
   * come within `READY_WITHIN_MS`; another page otherwise.
   */
  private loaded(frame: HTMLIFrameElement): void {
    if (frame !== this.frame || this.away) {
      return;
    }
    this.loadWaiting = true;
    if (!this.pair() && this.readyTimer === null) {
      this.readyTimer = setTimeout(() => {
        this.readyTimer = null;
        this.navigated(frame);
      }, READY_WITHIN_MS);
    }
  }

  /** A signed message `type` came: a `ready` or a `loaded` counts toward a load of the frame. */
  private signed(type: unknown): void {
    if (type === "ready") {
      this.readySeen = true;
    } else if (type === "loaded") {
      this.loadedSeen = true;
    } else {
      return;
    }
    this.pair();
  }

  /**
   * Pairs the load of the frame waiting with its page's `ready` and `loaded`, once both came;
   * true when it did.
   */
  private pair(): boolean {
    if (!this.loadWaiting || !this.readySeen || !this.loadedSeen) {
      return false;
    }
    this.loadWaiting = false;
    this.readySeen = false;
    this.loadedSeen = false;
    if (this.readyTimer !== null) {
      clearTimeout(this.readyTimer);
      this.readyTimer = null;
    }
    return true;
  }

  /**
   * Another page fills `frame`, its session no longer the page's: the session ends, and the page
   * is opened again, unless the same happened within `NAVIGATION_LOOP_MS`, when it is let go.
   */
  private navigated(frame: HTMLIFrameElement): void {
    const path = this.pagePath;
    if (frame !== this.frame || this.away || path === null) {
      return;
    }
    this.away = true;
    this.session?.dispose();
    this.session = null;
    this.held = { messages: [], loads: [] };
    const now = Date.now();
    const last = this.lastAway;
    if (last !== null && samePath(last.path, path) && now - last.at < NAVIGATION_LOOP_MS) {
      this.lastAway = null;
      this.close();
      this.host.keepsNavigatingAway(path);
      return;
    }
    this.lastAway = { path, at: now };
    this.host.navigatedAway(path);
  }

  /** A message from a page's bridge: taken only from the frame's own window, with the key. */
  private received(e: MessageEvent): void {
    const win = this.frame?.contentWindow;
    if (!win || e.source !== win || this.frame?.dataset.kind !== "page" || this.away) {
      return;
    }
    const data: unknown = e.data;
    if (
      this.key === null ||
      typeof data !== "object" ||
      data === null ||
      (data as { key?: unknown }).key !== this.key
    ) {
      return;
    }
    this.signed((data as { type?: unknown }).type);
    if (this.session) {
      this.session.receive(e.data);
    } else {
      this.held.messages.push(e.data);
    }
  }
}

/** Where the keyboard focus is in `doc`: in it, or inside a page's or a PDF's frame. */
export function frameFocus(doc: Document): FrameFocus {
  const active = doc.activeElement;
  if (active instanceof HTMLIFrameElement && active.classList.contains("lx-frame")) {
    return active.dataset.kind === "pdf" ? "pdf" : "page";
  }
  return "none";
}

/**
 * Calls `tell` whenever the keyboard focus goes into a frame of `doc` or out of one: the window
 * blurs as the focus goes into a frame. Returns a check to run when a frame comes or goes.
 */
export function trackFrameFocus(doc: Document, tell: (focus: FrameFocus) => void): () => void {
  let told: FrameFocus = "none";
  const check = (): void => {
    // The focus has moved by the time the event's tasks are done.
    setTimeout(() => {
      const focus = frameFocus(doc);
      if (focus !== told) {
        told = focus;
        tell(focus);
      }
    }, 0);
  };
  const win = doc.defaultView;
  win?.addEventListener("blur", check);
  win?.addEventListener("focus", check);
  doc.addEventListener("focusin", check);
  return check;
}
