// An HTML page's session with its bridge (crates/lectern-core/assets/page-bridge.js), which runs in
// the page's sandboxed frame: what the page reports (its text, headings, scroll position,
// selection, colour scheme and clicked links), and what Lectern asks of it (highlights, scrolling
// and the colours to paint with). Also the notice of what the page loads from the web. Loaded on
// first use, when a page opens.
//
// The page can post to Lectern itself, so nothing it sends is trusted: frame-view.ts takes only
// messages from the frame's own window, and every field is checked here, by type and size. A
// message that is off in any way is dropped, never thrown on. Values are only ever text, never
// markup. A link is followed only right after the reader did something in the page, and at most
// one a second, so a page can't send Lectern anywhere by itself, nor many places at once. The page's text is taken at most
// every half second, the latest, so a page that keeps changing doesn't rebuild find and the
// outline all the time. An anchor, or a place to restore, waits until the page is ready; an anchor
// wins.
import { h } from "./dom";
import type { FindPage, Span } from "./find";
import type { FollowTarget } from "./generated/FollowTarget";
import type { OutlineItem } from "./generated/OutlineItem";
import type { PageLoads } from "./generated/PageLoads";

/** The most text a page reports, in UTF-16 code units, as the bridge caps it. */
export const MAX_TEXT = 2 * 1024 * 1024;
const MAX_TITLE = 2000;
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

/** A block of the page's text, as the bridge reports it. */
export interface PageBlock {
  text: string;
  headingLevel?: number;
}

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

export interface PageSelection {
  quote: string;
  start: number;
  end: number;
  rect: { x: number; y: number; width: number; height: number };
}

export type BridgeMessage =
  | {
      type: "ready";
      title: string;
      headings: PageHeading[];
      blocks: PageBlock[];
      scroll: PageScroll;
      dark: boolean;
    }
  | { type: "blocks"; headings: PageHeading[]; blocks: PageBlock[] }
  | { type: "selection"; selection: PageSelection | null }
  | { type: "scroll"; scroll: PageScroll }
  | { type: "link"; href: string }
  | { type: "scheme"; dark: boolean };

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
        if (!isText(data.title, MAX_TITLE) || !h || !b || !s || typeof data.dark !== "boolean") {
          return null;
        }
        return {
          type: "ready",
          title: data.title,
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
      default:
        return null;
    }
  } catch {
    return null;
  }
}

/**
 * The notice of what a page loads from the web: up to three of the `hosts` Rust names, then how
 * many more of the `count` it has seen in all.
 */
export function noticeText(hosts: string[], count: number, blocked: boolean): string {
  const shown = hosts.slice(0, NOTICE_HOSTS);
  const more = Math.max(hosts.length, count) - shown.length;
  const named =
    more > 0
      ? `${shown.join(", ")} and ${String(more)} more`
      : shown.length > 1
        ? `${shown.slice(0, -1).join(", ")} and ${shown[shown.length - 1] ?? ""}`
        : (shown[0] ?? "");
  return blocked ? `Blocked loads from ${named}` : `This page loads from ${named}`;
}

/**
 * Where a link clicked in the page at `docPath` leads: a web or mail link goes out as an external
 * link; a relative one resolves against the page's folder, never above its drive or share, and
 * goes as a path (Lectern's link rules decide what opens it); anything else goes nowhere.
 */
export function pageLinkTarget(docPath: string, href: string): FollowTarget | null {
  const link = href.trim();
  if (/^(https?|mailto):/i.test(link)) {
    return { kind: "external", target: link, line: null, anchor: null };
  }
  // A scheme (a drive letter looks like one too), a host, or the root: not beside the page.
  if (link === "" || /^[a-z][a-z0-9+.-]*:/i.test(link) || /^[\\/]/.test(link)) {
    return null;
  }
  const hash = link.indexOf("#");
  const beforeHash = hash < 0 ? link : link.slice(0, hash);
  const query = beforeHash.indexOf("?");
  const rel = query < 0 ? beforeHash : beforeHash.slice(0, query);
  if (rel === "") {
    return null;
  }
  let decoded: string;
  let anchor: string | null = null;
  try {
    decoded = decodeURIComponent(rel);
    if (hash >= 0 && hash < link.length - 1) {
      anchor = decodeURIComponent(link.slice(hash + 1));
    }
  } catch {
    return null;
  }
  const unc = /^(\\\\[^\\]+\\[^\\]+)\\(.*)$/.exec(docPath);
  const drive = /^([A-Za-z]:)\\(.*)$/.exec(docPath);
  const parts = unc ?? drive;
  if (!parts) {
    return null;
  }
  const root = parts[1] ?? "";
  const folder = (parts[2] ?? "")
    .split("\\")
    .slice(0, -1)
    .filter((s) => s !== "");
  for (const segment of decoded.split(/[\\/]/)) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") folder.pop();
    else folder.push(segment);
  }
  if (folder.length === 0) {
    return null;
  }
  return { kind: "path", target: `${root}\\${folder.join("\\")}`, line: null, anchor };
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

export class PageFrame implements FindPage {
  /** The page's text: its blocks joined by single spaces. */
  text = "";
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
  /** This open's token, which its reports of what it loads carry. */
  private readonly token: string | null;
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

  constructor(
    private readonly frame: HTMLIFrameElement,
    private readonly notice: HTMLElement,
    private readonly doc: { path: string; frameUrl: string | null },
    private readonly host: PageHost,
    start: { fraction: number | null; anchor: string | null },
  ) {
    this.token = tokenOf(doc.frameUrl);
    this.restoreTo = start.fraction;
    this.anchorTo = start.anchor;
  }

  /** Where the view's top is in the text: the start of the block there. */
  get top(): number {
    return this.starts[this.topBlock] ?? 0;
  }

  /** A message the frame's own window posted (frame-view.ts checked where it came from). */
  receive(data: unknown): void {
    const message = parseBridgeMessage(data);
    if (message === null) {
      return;
    }
    switch (message.type) {
      case "ready":
        this.ready = true;
        this.dark = message.dark;
        this.takeScroll(message.scroll);
        this.sendTheme();
        this.takeText(message.headings, message.blocks);
        if (this.anchorTo !== null && this.scrollToHeading(this.anchorTo)) {
          // At the anchor, over any saved place.
        } else if (this.restoreTo !== null) {
          this.post({ type: "scroll-to", fraction: this.restoreTo });
        }
        this.anchorTo = null;
        this.restoreTo = null;
        break;
      case "blocks":
        this.textLater(message.headings, message.blocks);
        break;
      case "scroll":
        this.takeScroll(message.scroll);
        this.host.scrolled();
        break;
      case "selection":
        this.selection = message.selection;
        break;
      case "scheme":
        this.dark = message.dark;
        break;
      case "link":
        this.link(message.href);
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

  /** What the page has loaded from the web (or was blocked from), for this open's notice. */
  loads(loads: PageLoads): void {
    if (loads.token !== this.token || this.dismissed || loads.hosts.length === 0) {
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
    const parts: HTMLElement[] = [
      h("span", { class: "page-notice-text" }, noticeText(loads.hosts, loads.count, loads.blocked)),
    ];
    if (loads.blocked) {
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
    const target = pageLinkTarget(this.doc.path, href);
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

/** The token of a page's open, from its frame URL (`?t=`). */
function tokenOf(frameUrl: string | null): string | null {
  if (frameUrl === null) {
    return null;
  }
  try {
    return new URL(frameUrl, location.href).searchParams.get("t");
  } catch {
    return null;
  }
}
