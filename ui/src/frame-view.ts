// The frame an HTML page or a PDF shows in: it fills the document pane in place of the document
// (`#lx-doc-pane.framed`). The frame is in the page before its `src` is set, as WebView2 gives the
// first document the frame's colour scheme only then. Every show loads a new URL (`&v=<n>`): only
// a reload, never a change of hash, takes a PDF to a page or shows a change on disk.
//
// A PDF's frame has no sandbox, which WebView2's viewer won't run in. A page's always has one that
// allows scripts and never the same origin, and every response of the page protocol but a PDF's
// carries a sandboxing CSP of its own. A page's frame follows Lectern's light or dark theme, and
// its bridge talks to Lectern through page-frame.ts, loaded on first use: only messages its own
// window posts are taken, held until that module is there, as are an anchor or a saved place to go
// to once the page is ready (its own saved place unless something else is asked for). Above a page,
// a notice names what it loads from the web.
//
// The keyboard focus inside a frame is the frame's own, so Rust is told whenever it goes in or out
// (`trackFrameFocus`): only Rust sees the keys pressed there.
import type { DocPayload } from "./generated/DocPayload";
import type { FrameFocus } from "./generated/FrameFocus";
import type { PageLoads } from "./generated/PageLoads";
import type { PageFrame, PageHost } from "./page-frame";

/** Where a PDF opens: no reading position is kept in one. */
const PDF_START = "#page=1";

/** What a frame needs from the app. */
export interface FrameHost extends PageHost {
  /** Whether Lectern's theme is a dark one: a page's frame takes that colour scheme. */
  dark(): boolean;
  /** A frame came or went, so the keyboard focus may have moved out of one. */
  focusMoved(): void;
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
  /** Bumped by every show, so each loads a new URL. */
  private version = 0;
  /** The page's session with its bridge, once its module has loaded. */
  private session: PageFrame | null = null;
  /** What arrived for a page before its session: its bridge's messages and reports of loads. */
  private held: { messages: unknown[]; loads: PageLoads[] } = { messages: [], loads: [] };
  /** Where to take a page once it is ready, asked for before its session was there. */
  private start: { fraction: number | null; anchor: string | null } = {
    fraction: null,
    anchor: null,
  };

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

  /** Shows `doc`, a page or a PDF, from `url` (its `frameUrl`) in a new frame. */
  show(doc: DocPayload, url: string): void {
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
    // Where the reader left it, unless something else is asked for.
    this.start = { fraction: page ? (doc.position?.fraction ?? null) : null, anchor: null };
    this.version += 1;
    const src = withParam(url, "v", String(this.version));
    frame.src = page ? src : src + PDF_START;
    if (page) {
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
    this.pane.classList.remove("framed");
    if (had) {
      this.host.focusMoved();
    }
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

  /** A message from a page's bridge: taken only from the frame's own window. */
  private received(e: MessageEvent): void {
    const win = this.frame?.contentWindow;
    if (!win || e.source !== win || this.frame?.dataset.kind !== "page") {
      return;
    }
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
