// The document: inserted with one innerHTML assignment, with delegated handlers for links, copy
// buttons, tags and images, and scrolling to anchors, lines and saved positions.
//
// Anchor contract (ruling R16): links carry the fragment as written (`href="#Frag"` in-page,
// `data-anchor` for doc links and wikilinks) plus `data-slug`. The exact id is tried first, then
// the slug, always inside the document, never through `document.getElementById`.
import type { App, OpenOptions } from "./app";
import { HEADINGS, findById, samePath } from "./dom";
import type { DocPayload } from "./generated/DocPayload";
import type { FollowKind } from "./generated/FollowKind";
import type { FollowResult } from "./generated/FollowResult";
import type { FollowTarget } from "./generated/FollowTarget";
import type { SavedPosition } from "./generated/SavedPosition";

/** Where an anchor or line lands below the pane's top edge, in pixels. */
const TOP_GAP = 12;
/** Anchoring ends once the target has held still (within 2 px) for this many frames... */
const STABLE_FRAMES = 3;
const STABLE_PX = 2;
/** ...or after this long. */
const ANCHOR_MS = 1000;
/** Input that means the reader is scrolling: anchoring stops rather than fight it. */
export const READER_INPUT = ["wheel", "keydown", "touchstart", "pointerdown"] as const;
const COPIED_MS = 1200;
const LOCAL_KINDS = new Set<string>(["doc", "file", "path"] satisfies FollowKind[]);

export class DocView {
  private readonly copyTimers = new WeakMap<HTMLElement, number>();
  /** Stops the anchoring in progress, if any. */
  private stopAnchoring: (() => void) | null = null;
  /** Bumped by every followed link, so only the latest click navigates. */
  private follows = 0;

  constructor(
    private readonly host: HTMLElement,
    private readonly app: App,
  ) {
    host.addEventListener("click", (e) => {
      this.onClick(e);
    });
    // A middle click on a link would open a new window.
    host.addEventListener("auxclick", (e) => {
      if (e.target instanceof Element && e.target.closest("a")) {
        e.preventDefault();
      }
    });
    // `error` doesn't bubble, so it is caught on the way down.
    host.addEventListener(
      "error",
      (e) => {
        if (e.target instanceof HTMLImageElement) {
          placeholder(e.target);
        }
      },
      true,
    );
  }

  render(doc: DocPayload): void {
    this.host.innerHTML = doc.html;
    // A blocked image has no src to fail: it shows its placeholder straight away.
    for (const img of this.host.querySelectorAll<HTMLImageElement>("img.img-blocked")) {
      placeholder(img);
    }
  }

  /** The element an anchor names: its exact id, else its slug, inside the document only. */
  findAnchor(id: string, slug?: string): HTMLElement | null {
    return (
      (id === "" ? null : findById(this.host, id)) ??
      (slug === undefined || slug === "" ? null : findById(this.host, slug))
    );
  }

  /** Scrolls the anchor to the top of the pane; false when the document has no such anchor. */
  scrollToAnchor(id: string, slug?: string): boolean {
    const target = this.findAnchor(id, slug);
    if (target) {
      this.placeAt(target, TOP_GAP);
    }
    return target !== null;
  }

  /** Scrolls the block holding source line `line` to the top of the pane. */
  scrollToLine(line: number): boolean {
    const block = this.blockAt(line);
    if (block) {
      this.placeAt(block, TOP_GAP);
    }
    return block !== null;
  }

  /**
   * Where the reader is: the nearest heading above the pane's top and the distance below it, the
   * top block's source line, and the scroll fraction.
   */
  captureAnchor(): SavedPosition {
    const scroller = this.app.scroller;
    const top = scroller.getBoundingClientRect().top;
    const max = scroller.scrollHeight - scroller.clientHeight;
    let headingId: string | null = null;
    let offset = 0;
    for (const heading of this.host.querySelectorAll<HTMLElement>(HEADINGS)) {
      const headingTop = heading.getBoundingClientRect().top;
      if (headingTop > top + 1) {
        break;
      }
      headingId = heading.id;
      offset = top - headingTop;
    }
    let line: number | null = null;
    for (const block of this.host.children) {
      if (block.getBoundingClientRect().bottom > top) {
        line = sourceLine(block);
        break;
      }
    }
    return { headingId, offset, line, fraction: max > 0 ? scroller.scrollTop / max : 0 };
  }

  /** Scrolls back to a captured position: by heading, else by line, else by fraction. */
  restore(p: SavedPosition): void {
    const heading = p.headingId === null ? null : findById(this.host, p.headingId);
    if (heading) {
      this.placeAt(heading, -p.offset);
      return;
    }
    const block = p.line === null ? null : this.blockAt(p.line);
    if (block) {
      this.placeAt(block, 0);
      return;
    }
    const scroller = this.app.scroller;
    scroller.scrollTop = p.fraction * (scroller.scrollHeight - scroller.clientHeight);
  }

  /** The element whose source starts last at or before `line`. */
  private blockAt(line: number): HTMLElement | null {
    let best: HTMLElement | null = null;
    let bestLine = 0;
    for (const el of this.host.querySelectorAll<HTMLElement>("[data-sourcepos]")) {
      const start = sourceLine(el);
      if (start !== null && start <= line && start > bestLine) {
        best = el;
        bestLine = start;
      }
    }
    return best;
  }

  /**
   * Scrolls so `el`'s top sits `at` pixels below the pane's top, and keeps it there. Blocks that
   * content-visibility kept at an estimated height take their real one as they come near the
   * viewport, which moves `el` by up to thousands of pixels a frame or two later; so the position
   * is corrected every frame until `el` has held still for a few frames, for at most a second, and
   * not once the reader scrolls.
   */
  private placeAt(el: HTMLElement, at: number): void {
    this.stopAnchoring?.();
    const scroller = this.app.scroller;
    const correct = (): number => {
      const delta = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - at;
      if (Math.abs(delta) > 0.5) {
        scroller.scrollTop += delta;
      }
      return delta;
    };
    correct();
    const started = performance.now();
    let stable = 0;
    let frame = 0;
    const stop = (): void => {
      cancelAnimationFrame(frame);
      for (const type of READER_INPUT) {
        window.removeEventListener(type, stop, true);
      }
      if (this.stopAnchoring === stop) {
        this.stopAnchoring = null;
      }
    };
    const tick = (): void => {
      if (!el.isConnected || performance.now() - started > ANCHOR_MS) {
        stop();
        return;
      }
      stable = Math.abs(correct()) <= STABLE_PX ? stable + 1 : 0;
      if (stable >= STABLE_FRAMES) {
        stop();
        return;
      }
      frame = requestAnimationFrame(tick);
    };
    for (const type of READER_INPUT) {
      window.addEventListener(type, stop, { capture: true, passive: true });
    }
    frame = requestAnimationFrame(tick);
    this.stopAnchoring = stop;
  }

  private onClick(e: MouseEvent): void {
    const target = e.target instanceof Element ? e.target : null;
    const copy = target?.closest<HTMLElement>(".code-copy");
    if (copy && this.host.contains(copy)) {
      this.copy(copy);
      return;
    }
    const link = target?.closest("a");
    if (link && this.host.contains(link)) {
      // Nothing in a document may navigate the window.
      e.preventDefault();
      this.follow(link);
      return;
    }
    const tag = target?.closest<HTMLElement>(".tag")?.dataset.tag;
    if (tag !== undefined) {
      this.app.openSearch(`#${tag}`);
    }
  }

  private follow(link: HTMLAnchorElement): void {
    const kind = link.dataset.kind;
    const href = link.getAttribute("href") ?? "";
    if (kind === "anchor" || (kind === undefined && href.startsWith("#"))) {
      this.scrollToAnchor(decodeFragment(href.slice(1)), link.dataset.slug);
      return;
    }
    if (kind === "broken") {
      this.app.toast(link.title || "This link doesn't lead anywhere");
      return;
    }
    let target: FollowTarget;
    if (kind !== undefined && LOCAL_KINDS.has(kind)) {
      const line = link.dataset.line;
      target = {
        kind: kind as FollowKind,
        target: link.dataset.target ?? "",
        line: line === undefined ? null : Number(line),
        anchor: link.dataset.anchor ?? null,
      };
    } else if (kind === "external" || /^(https?|mailto):/i.test(href)) {
      target = { kind: "external", target: href, line: null, anchor: null };
    } else {
      return;
    }
    void this.followTarget(target, link.dataset.slug);
  }

  private async followTarget(target: FollowTarget, slug: string | undefined): Promise<void> {
    // Resolving can be slow; by the time it answers, a later click or any other navigation wins.
    const follow = ++this.follows;
    const navigation = this.app.navigation;
    let result: FollowResult;
    try {
      result = await this.app.backend.follow(target);
    } catch (e) {
      if (follow === this.follows) this.app.toast(String(e));
      return;
    }
    if (follow !== this.follows || navigation !== this.app.navigation) {
      return;
    }
    if (result.action === "notFound") {
      this.app.toast(result.message);
      return;
    }
    if (result.action !== "openDoc") {
      return;
    }
    const here = this.app.state.doc;
    const { anchor, line } = result;
    // A link into this same document (`[[#Heading]]`) only scrolls.
    if (here && samePath(here.path, result.path)) {
      if (anchor !== null && this.scrollToAnchor(anchor, slug)) return;
      if (line !== null && this.scrollToLine(line)) return;
    }
    const opts: OpenOptions = { push: true };
    if (anchor !== null) {
      opts.anchor = anchor;
      if (slug !== undefined) opts.slug = slug;
    }
    if (line !== null) {
      opts.line = line;
    }
    await this.app.open(result.path, opts);
  }

  private copy(button: HTMLElement): void {
    const code = button.closest(".code-block")?.querySelector<HTMLElement>("pre code");
    if (!code) {
      return;
    }
    navigator.clipboard.writeText(code.innerText).then(
      () => {
        button.textContent = "Copied";
        window.clearTimeout(this.copyTimers.get(button));
        this.copyTimers.set(
          button,
          window.setTimeout(() => {
            button.textContent = "Copy";
          }, COPIED_MS),
        );
      },
      () => {
        this.app.toast("Couldn't copy to the clipboard");
      },
    );
  }
}

/** The start line of an element's `data-sourcepos` (`12:1-14:3`). */
function sourceLine(el: Element): number | null {
  const match = /^(\d+):/.exec(el.getAttribute("data-sourcepos") ?? "");
  return match ? Number(match[1]) : null;
}

function decodeFragment(fragment: string): string {
  try {
    return decodeURIComponent(fragment);
  } catch {
    return fragment;
  }
}

/** Swaps an image that failed (or was blocked) for a box holding its alt text. */
function placeholder(img: HTMLImageElement): void {
  const box = document.createElement("span");
  box.className = "img-placeholder";
  box.textContent = img.alt || "Image";
  box.title = img.title || "This image couldn't be loaded";
  img.replaceWith(box);
}
