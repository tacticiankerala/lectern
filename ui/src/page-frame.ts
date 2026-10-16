// An HTML page's session with its bridge (crates/lectern-core/assets/page-bridge.js), which runs in
// the page's sandboxed frame: what the page reports (its text, headings, scroll position,
// selection, colour scheme, clicked links and clicks on text a comment marks), and what Lectern
// asks of it (highlights, scrolling and the colours to paint with). Also the notice of what the
// page loads from the web. Loaded on first use, when a page opens. It is the page the review
// comments see (comments-page.ts): its blocks are the text core anchors them against.
//
// The page can post to Lectern itself, so nothing it sends is trusted: frame-view.ts takes only
// messages from the frame's own window, and every field is checked here, by type and size. A
// message that is off in any way is dropped, never thrown on. Values are only ever text, never
// markup. Only messages signed with the open's key reach a session (frame-view.ts checks), and a
// second `ready` is the page loading itself again: it is read again, and taken back where the
// reader was. A link is followed only right after the reader did something in the
// page, and at most one a second, so a page can't send Lectern anywhere by itself, nor many places
// at once; one within the page's open (its own URL's folder, `/o/<token>/`, and below) goes to the
// file it names there. The page's text is taken at most every half second, the latest, so a page
// that keeps changing doesn't rebuild find and the outline all the time. An anchor, or a place to
// restore, waits until the page is ready; an anchor wins.
import type { PageSurface } from "./comments-page";
import { h } from "./dom";
import type { FindPage, Span } from "./find";
import type { FollowTarget } from "./generated/FollowTarget";
import type { OutlineItem } from "./generated/OutlineItem";
import type { PageBlock } from "./generated/PageBlock";
import type { PageLoads } from "./generated/PageLoads";

/** The most text a page reports, in UTF-16 code units, as the bridge caps it. */
export const MAX_TEXT = 2 * 1024 * 1024;
const MAX_HEADINGS = 10_000;
const MAX_HEADING_TEXT = 2000;
const MAX_ID = 1000;
const MAX_HREF = 8192;
/** The hosts the notice names before "and N more". */
const NOTICE_HOSTS = 3;
/** How often the page's text is taken, at most, in milliseconds. */
const TEXT_EVERY_MS = 500;
/** How often a link is followed, at most, in milliseconds. */
const FOLLOW_EVERY_MS = 1000;

/** A heading of the page: its block's text, its element's id, and where its block starts. */
export interface PageHeading {
  level: number;
  text: string;
  id: string;
  offset: number;
}

export interface PageScroll {
  /** How far down the page is scrolled, 0 to 1. */
  fraction: number;
  /** The block at the top of the view. */
  top: number;
}

/**
 * The text selected in a page: its quote, its offsets into the page's text, and the box of its
 * last line in the frame's viewport.
 */
export interface PageSelection {
  quote: string;
  start: number;
  end: number;
  rect: { x: number; y: number; width: number; height: number };
}

export type BridgeMessage =
  | {
      type: "ready";
      headings: PageHeading[];
      blocks: PageBlock[];
      scroll: PageScroll;
      dark: boolean;
    }
  | { type: "blocks"; headings: PageHeading[]; blocks: PageBlock[] }
  | { type: "selection"; selection: PageSelection | null }
  | { type: "scroll"; scroll: PageScroll }
  | { type: "link"; href: string }
  | { type: "scheme"; dark: boolean }
  | { type: "comment-click"; offset: number };

type Fields = Record<string, unknown>;

function isObject(value: unknown): value is Fields {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isText(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length <= max;
}

function isLevel(value: unknown): value is number {
  return isCount(value) && value >= 1 && value <= 6;
}

function headings(value: unknown): PageHeading[] | null {
  if (!Array.isArray(value) || value.length > MAX_HEADINGS) return null;
  const out: PageHeading[] = [];
  for (const item of value as unknown[]) {
    if (
      !isObject(item) ||
      !isLevel(item.level) ||
      !isText(item.text, MAX_HEADING_TEXT) ||
      !isText(item.id, MAX_ID) ||
      !isCount(item.offset)
    ) {
      return null;
    }
    out.push({ level: item.level, text: item.text, id: item.id, offset: item.offset });
  }
  return out;
}

function blocks(value: unknown): PageBlock[] | null {
  if (!Array.isArray(value)) return null;
  const out: PageBlock[] = [];
  // The text and a space between each two blocks.
  let length = -1;
  for (const item of value as unknown[]) {
    if (!isObject(item) || typeof item.text !== "string") return null;
    length += item.text.length + 1;
    if (length > MAX_TEXT) return null;
    const level = item.headingLevel;
    if (level === undefined || level === null) {
      out.push({ text: item.text });
    } else if (isLevel(level)) {
      out.push({ text: item.text, headingLevel: level });
    } else {
      return null;
    }
  }
  return out;
}

function scroll(value: unknown): PageScroll | null {
  if (!isObject(value)) return null;
  const { fraction, top } = value;
  if (typeof fraction !== "number" || !(fraction >= 0 && fraction <= 1) || !isCount(top)) {
    return null;
  }
  return { fraction, top };
}

function selection(value: unknown): PageSelection | null | undefined {
  if (value === null) return null;
  if (!isObject(value) || !isText(value.quote, MAX_TEXT)) return undefined;
  const { start, end, rect } = value;
  if (!isCount(start) || !isCount(end) || end < start || !isObject(rect)) return undefined;
  const box = [rect.x, rect.y, rect.width, rect.height];
  if (!box.every((n) => typeof n === "number" && Number.isFinite(n))) return undefined;
  const [x, y, width, height] = box as number[];
  return {
    quote: value.quote,
    start,
    end,
    rect: { x: x ?? 0, y: y ?? 0, width: width ?? 0, height: height ?? 0 },
  };
}

/** A message from a page's bridge, every field checked; null for anything else. */
export function parseBridgeMessage(data: unknown): BridgeMessage | null {
  try {
    if (!isObject(data) || data.lx !== 1) return null;
    switch (data.type) {
      case "ready": {
        const h = headings(data.headings);
        const b = blocks(data.blocks);
        const s = scroll(data.scroll);
        if (!h || !b || !s || typeof data.dark !== "boolean") {
          return null;
        }
        return {
          type: "ready",
          headings: h,
          blocks: b,
          scroll: s,
          dark: data.dark,
        };
      }
      case "blocks": {
        const h = headings(data.headings);
        const b = blocks(data.blocks);
        return h && b ? { type: "blocks", headings: h, blocks: b } : null;
      }
      case "selection": {
        const s = selection(data.selection);
        return s === undefined ? null : { type: "selection", selection: s };
      }
      case "scroll": {
        const s = scroll(data.scroll);
        return s ? { type: "scroll", scroll: s } : null;
      }
      case "link":
        return isText(data.href, MAX_HREF) ? { type: "link", href: data.href } : null;
      case "scheme":
        return typeof data.dark === "boolean" ? { type: "scheme", dark: data.dark } : null;
      case "comment-click":
        return isCount(data.offset) && data.offset <= MAX_TEXT
          ? { type: "comment-click", offset: data.offset }
          : null;
      default:
        return null;
    }
  } catch {
    return null;
  }
}

/** Up to three of the `hosts` Rust names, then how many more of the `count` it has seen in all. */
function hostNames(hosts: string[], count: number): string {
  const shown = hosts.slice(0, NOTICE_HOSTS);
  const more = Math.max(hosts.length, count) - shown.length;
  return more > 0
    ? `${shown.join(", ")} and ${String(more)} more`
    : shown.length > 1
      ? `${shown.slice(0, -1).join(", ")} and ${shown[shown.length - 1] ?? ""}`
      : (shown[0] ?? "");
}

/**
 * The notice of what a page loads from the web, or was blocked from (`blocked`): the hosts named
 * as `hostNames` names them.
 */
export function noticeText(hosts: string[], count: number, blocked: boolean): string {
  const named = hostNames(hosts, count);
  return blocked ? `Blocked loads from ${named}` : `This page loads from ${named}`;
}

/**
 * Where a page's open serves from: the token its URL names, that URL's folder (`…/o/<token>/`,
 * absolute), and the folder on disk it stands for, the page's own, with the separator its path
 * uses.
 */
export interface PageOpen {
  token: string;
  base: string;
  folder: string;
  separator: string;
}

/** The open of the page at `docPath`, from its frame URL (`…/o/<token>/<name>`), or null. */
export function pageOpen(frameUrl: string | null, docPath: string): PageOpen | null {
  if (frameUrl === null) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(frameUrl, location.href);
  } catch {
    return null;
  }
  const found = /^(.*\/o\/([^/]+)\/)[^/]+$/.exec(url.origin + url.pathname);
  // Either separator, as a path may be written with either.
  const cut = Math.max(docPath.lastIndexOf("\\"), docPath.lastIndexOf("/"));
  if (!found || cut < 0) {
    return null;
  }
  return {
    token: found[2] ?? "",
    base: found[1] ?? "",
    folder: docPath.slice(0, cut),
    separator: docPath.charAt(cut),
  };
}

/** Whether `host` is a machine-local name: `localhost`, or one of Lectern's own `*.localhost`. */
function isLocalHost(host: string): boolean {
  return host === "localhost" || host.endsWith(".localhost");
}

/**
 * Where a link clicked in a page leads, from the URL the browser resolved it to (`href`, `<base>`
 * and all): one within the page's open (`open`) goes as a path, its open's folder and below on
 * disk, to Lectern's link rules; a web or mail link goes out as an external link; anything else,
 * any other URL of the page's origin among them, goes nowhere.
 */
export function pageLinkTarget(open: PageOpen | null, href: string): FollowTarget | null {
  let url: URL;
  try {
    url = new URL(href.trim());
  } catch {
    return null;
  }
  const at = url.origin + url.pathname;
  if (open !== null && at.startsWith(open.base)) {
    const segments: string[] = [];
    for (const segment of at.slice(open.base.length).split("/")) {
      let decoded: string;
      try {
        decoded = decodeURIComponent(segment);
      } catch {
        return null;
      }
      // eslint-disable-next-line no-control-regex -- control characters are what it refuses
      if (/^\.{0,2}$|[\\/:\u0000-\u001f]/.test(decoded)) {
        return null;
      }
      segments.push(decoded);
    }
    let anchor: string | null = null;
    if (url.hash.length > 1) {
      try {
        anchor = decodeURIComponent(url.hash.slice(1));
      } catch {
        return null;
      }
    }
    return {
      kind: "path",
      target: [open.folder, ...segments].join(open.separator),
      line: null,
      anchor,
    };
  }
  if (open !== null && url.origin === new URL(open.base).origin) {
    return null;
  }
  if (
    url.protocol === "mailto:" ||
    (/^https?:$/.test(url.protocol) && !isLocalHost(url.hostname))
  ) {
    return { kind: "external", target: url.href, line: null, anchor: null };
  }
  return null;
}

/** What a page's session needs from the app. */
export interface PageHost {
  /** The page's headings, for the outline. */
  headings(items: OutlineItem[]): void;
  /** The page's text changed: find searches it again. */
  text(): void;
  /** The page scrolled: its reading position may be saved. */
  scrolled(): void;
  /** Follows a link clicked in the page. */
  follow(target: FollowTarget): void;
  /** Opens Preferences, for the notice's link. */
  preferences(): void;
  /** The text selected in the page changed (`PageFrame.selection`). */
  selection(): void;
  /** The reader clicked text a comment marks, at `offset` into the page's text. */
  commentClicked(offset: number): void;
}

/** Colours to paint a highlight layer with. */
interface Colours {
  background: string;
  color?: string;
}

/** Lectern's highlight colours, from the theme in effect, for the bridge to paint with. */
function pageTheme(): Record<string, Colours> {
  const style = getComputedStyle(document.documentElement);
  const value = (name: string): string => style.getPropertyValue(name).trim();
  return {
    find: { background: value("--mark") },
    "find-current": { background: value("--accent"), color: value("--bg") },
    comment: { background: value("--comment") },
    "comment-focus": { background: value("--comment-focus") },
  };
}

/** Whether the reader has just done something (clicked, pressed a key), in the page or here. */
function readerActed(): boolean {
  const activation = (navigator as { userActivation?: { isActive: boolean } }).userActivation;
  return activation === undefined || activation.isActive;
}

export class PageFrame implements FindPage, PageSurface {
  /** The page's text: its blocks joined by single spaces. */
  text = "";
  /** The blocks, as the page reported them. */
  blocks: PageBlock[] = [];
  /** Where each block starts in `text`. */
  starts: number[] = [];
  /** How far down the page is scrolled, 0 to 1, as it last said. */
  fraction = 0;
  /** The page has reported its text (`ready`): it may be scrolled and painted. */
  ready = false;
  /** Whether the page sees the dark colour scheme, as it last said. */
  dark = false;
  /** The text selected in the page, as it last said. */
  selection: PageSelection | null = null;
  private headings: PageHeading[] = [];
  /** The block at the top of the view. */
  private topBlock = 0;
  /** This page's open: its token, which its reports of what it loads carry, and what it reaches. */
  private readonly open: PageOpen | null;
  /** The reader dismissed the notice. */
  private dismissed = false;
  /** When a link was last followed; links within `FOLLOW_EVERY_MS` of it are dropped. */
  private followedAt = -Infinity;
  /** When the page's text was last taken, and the latest that came since, waiting its turn. */
  private textTaken = 0;
  private waiting: { headings: PageHeading[]; blocks: PageBlock[] } | null = null;
  private textTimer: ReturnType<typeof setTimeout> | null = null;
  /** Where to scroll once the page is ready: a saved fraction, and an anchor, which wins. */
  private restoreTo: number | null;
  private anchorTo: string | null;

  /** The session of the page `doc` in `frame`, its notice in `notice`. */
  constructor(
    private readonly frame: HTMLIFrameElement,
    private readonly notice: HTMLElement,
    doc: { path: string; frameUrl: string | null },
    private readonly host: PageHost,
    start: { fraction: number | null; anchor: string | null },
  ) {
    this.open = pageOpen(doc.frameUrl, doc.path);
    this.restoreTo = start.fraction;
    this.anchorTo = start.anchor;
  }

  /** Where the view's top is in the text: the start of the block there. */
  get top(): number {
    return this.starts[this.topBlock] ?? 0;
  }

  /**
   * The heading the view's top is under, after the headings it sits under, outermost first; empty
   * above the first heading.
   */
  headingPath(): string[] {
    const top = this.top;
    const path: PageHeading[] = [];
    for (const heading of this.headings) {
      if (heading.offset > top) break;
      while (path.length > 0 && (path[path.length - 1]?.level ?? 0) >= heading.level) path.pop();
      path.push(heading);
    }
    return path.map((h) => h.text);
  }

  /** A message the frame's own window posted (frame-view.ts checked where it came from). */
  receive(data: unknown): void {
    const message = parseBridgeMessage(data);
    if (message === null) {
      return;
    }
    switch (message.type) {
      case "ready": {
        // A second is the page loading itself again: back where the reader was.
        const again = this.ready ? this.fraction : null;
        this.ready = true;
        this.dark = message.dark;
        this.takeScroll(message.scroll);
        this.sendTheme();
        this.takeText(message.headings, message.blocks);
        if (again !== null) {
          this.post({ type: "scroll-to", fraction: again });
          break;
        }
        if (this.anchorTo !== null && this.scrollToHeading(this.anchorTo)) {
          // At the anchor, over any saved place.
        } else if (this.restoreTo !== null) {
          this.post({ type: "scroll-to", fraction: this.restoreTo });
        }
        this.anchorTo = null;
        this.restoreTo = null;
        break;
      }
      case "blocks":
        this.textLater(message.headings, message.blocks);
        break;
      case "scroll":
        this.takeScroll(message.scroll);
        this.host.scrolled();
        break;
      case "selection":
        this.selection = message.selection;
        this.host.selection();
        break;
      case "scheme":
        this.dark = message.dark;
        break;
      case "link":
        this.link(message.href);
        break;
      case "comment-click":
        this.host.commentClicked(message.offset);
        break;
    }
  }

  /** Sends Lectern's highlight colours, as the theme now has them. */
  sendTheme(): void {
    this.post({ type: "theme", theme: pageTheme() });
  }

  paint(layer: string, spans: Span[]): void {
    this.post({
      type: "paint",
      layer,
      ranges: spans.map(({ start, end }) => ({ start, end })),
    });
  }

  clear(layer: string): void {
    this.post({ type: "clear", layer });
  }

  /** Scrolls the text at `offset` to the middle, if it is out of sight. */
  reveal(offset: number): void {
    this.post({ type: "scroll-to", offset, reveal: true });
  }

  /** Scrolls the text at `offset` to the top. */
  scrollTo(offset: number): void {
    this.post({ type: "scroll-to", offset });
  }

  /** The frame's box in Lectern's viewport. */
  frameBox(): DOMRect {
    return this.frame.getBoundingClientRect();
  }

  /**
   * Scrolls the heading the outline lists `index`th to the top, by where its block starts, which
   * tells apart two headings that share an id; false when it lists no such heading.
   */
  scrollToHeadingAt(index: number): boolean {
    const heading = this.headings[index];
    if (!heading) {
      return false;
    }
    this.post({ type: "scroll-to", offset: heading.offset });
    return true;
  }

  /** Scrolls the heading `id` to the top; false when the page has no such heading. */
  scrollToHeading(id: string): boolean {
    const heading = this.headings.find((h) => h.id === id);
    if (!heading) {
      return false;
    }
    this.post({ type: "scroll-to", offset: heading.offset });
    return true;
  }

  /**
   * Scrolls the heading `id` to the top once the page is ready, over any saved place; true unless
   * the ready page has no such heading.
   */
  scrollToAnchor(id: string): boolean {
    if (this.ready) {
      return this.scrollToHeading(id);
    }
    this.anchorTo = id;
    return true;
  }

  /** Stops taking what the page sends: it has gone. */
  dispose(): void {
    if (this.textTimer !== null) {
      clearTimeout(this.textTimer);
      this.textTimer = null;
    }
    this.waiting = null;
  }

  /** Scrolls to `fraction` of the page, once it is ready. */
  restore(fraction: number): void {
    if (this.ready) {
      this.post({ type: "scroll-to", fraction });
    } else {
      this.restoreTo = fraction;
    }
  }

  /**
   * What the page has loaded from the web, and what it was blocked from, for this open's notice:
   * Preferences a click away when it was served without the web.
   */
  loads(loads: PageLoads): void {
    if (
      loads.token !== this.open?.token ||
      this.dismissed ||
      loads.hosts.length + loads.blockedHosts.length === 0
    ) {
      return;
    }
    const dismiss = h(
      "button",
      { type: "button", class: "page-notice-dismiss", "aria-label": "Dismiss" },
      "×",
    );
    dismiss.addEventListener("click", () => {
      this.dismissed = true;
      this.notice.hidden = true;
    });
    // What it loaded, and apart from it what was blocked; else only what was blocked.
    const parts: HTMLElement[] =
      loads.hosts.length > 0
        ? [h("span", { class: "page-notice-text" }, noticeText(loads.hosts, loads.count, false))]
        : [
            h(
              "span",
              { class: "page-notice-text" },
              noticeText(loads.blockedHosts, loads.blockedCount, true),
            ),
          ];
    if (loads.hosts.length > 0 && loads.blockedHosts.length > 0) {
      parts.push(
        h(
          "span",
          { class: "page-notice-blocked" },
          `Blocked: ${hostNames(loads.blockedHosts, loads.blockedCount)}`,
        ),
      );
    }
    // Served without the web, the page may be let at it.
    if (loads.offline) {
      const change = h(
        "button",
        { type: "button", class: "page-notice-prefs" },
        "Change in Preferences",
      );
      change.addEventListener("click", () => {
        this.host.preferences();
      });
      parts.push(change);
    }
    this.notice.replaceChildren(...parts, dismiss);
    this.notice.hidden = false;
  }

  /**
   * Follows a link the page says the reader clicked: only while the reader has just done
   * something (the browser's user activation), and at most one a second, a link within the second
   * dropped, not kept. The parent can't see clicks inside the frame, so a rate is what it can hold
   * a page to.
   */
  private link(href: string): void {
    const now = Date.now();
    if (!readerActed() || now - this.followedAt < FOLLOW_EVERY_MS) {
      return;
    }
    const target = pageLinkTarget(this.open, href);
    if (target) {
      this.followedAt = now;
      this.host.follow(target);
    }
  }

  /** Takes the page's changed text now, or, when it was taken lately, the latest a moment later. */
  private textLater(headings: PageHeading[], blocks: PageBlock[]): void {
    this.waiting = { headings, blocks };
    if (this.textTimer !== null) {
      return;
    }
    const wait = this.textTaken + TEXT_EVERY_MS - Date.now();
    if (wait <= 0) {
      this.takeWaiting();
      return;
    }
    this.textTimer = setTimeout(() => {
      this.textTimer = null;
      this.takeWaiting();
    }, wait);
  }

  private takeWaiting(): void {
    const waiting = this.waiting;
    this.waiting = null;
    if (waiting) {
      this.takeText(waiting.headings, waiting.blocks);
    }
  }

  private takeText(headings: PageHeading[], blocks: PageBlock[]): void {
    this.textTaken = Date.now();
    const starts: number[] = [];
    let at = 0;
    for (const block of blocks) {
      starts.push(at);
      at += block.text.length + 1;
    }
    this.starts = starts;
    this.blocks = blocks;
    this.text = blocks.map((b) => b.text).join(" ");
    // The outline is listed again only when the headings changed, not with every change of text.
    const listed = JSON.stringify(this.headings) === JSON.stringify(headings);
    this.headings = headings;
    if (!listed) {
      this.host.headings(headings.map(({ level, text, id }) => ({ level, text, id })));
    }
    this.host.text();
  }

  private takeScroll(scroll: PageScroll): void {
    this.fraction = scroll.fraction;
    this.topBlock = scroll.top;
  }

  private post(message: Record<string, unknown>): void {
    // The frame's origin is opaque, so no other target origin reaches it.
    this.frame.contentWindow?.postMessage({ lx: 1, ...message }, "*");
  }
}
