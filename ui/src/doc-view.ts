// The document: inserted with one innerHTML assignment, with delegated handlers for links, copy
// buttons, tags and images, scrolling to anchors, lines and saved positions (position.ts), and room
// after the text for its last heading to reach the top.
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
import { blockAt, capture, restore } from "./position";
import { placeAt } from "./reflow";

/** Where an anchor or line lands below the pane's top edge, in pixels. */
const TOP_GAP = 12;
const COPIED_MS = 1200;
const LOCAL_KINDS = new Set<string>(["doc", "file", "path"] satisfies FollowKind[]);

export class DocView {
  private readonly copyTimers = new WeakMap<HTMLElement, number>();
  /** Bumped by every followed link, so only the latest click navigates. */
  private follows = 0;
  /** The document's last heading, which the room after the text lets reach the top. */
  private lastHeading: HTMLElement | null = null;
  /** The room after the text, in pixels, as last set. */
  private room = -1;

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
    // Lazy layout, reflows and resizes move the last heading or the pane's height.
    if (typeof ResizeObserver === "function") {
      const observer = new ResizeObserver(() => {
        this.fitRoom();
      });
      observer.observe(host);
      observer.observe(app.scroller, { box: "border-box" });
    }
  }

  render(doc: DocPayload): void {
    this.host.innerHTML = doc.html;
    // A blocked image has no src to fail: it shows its placeholder straight away.
    for (const img of this.host.querySelectorAll<HTMLImageElement>("img.img-blocked")) {
      placeholder(img);
    }
    const headings = this.host.querySelectorAll<HTMLElement>(HEADINGS);
    this.lastHeading = headings[headings.length - 1] ?? null;
    // Before anything scrolls, so a spot near the end can be reached.
    this.fitRoom();
  }

  /**
   * Leaves room after the text for its last heading to reach the top of the pane, as any other
   * can: just enough, so a document without headings, or one whose last section fills a screen,
   * ends where its text does.
   */
  private fitRoom(): void {
    const scroller = this.app.scroller;
    const last = this.lastHeading;
    let room = 0;
    if (last?.isConnected) {
      const below = this.host.getBoundingClientRect().bottom - last.getBoundingClientRect().top;
      room = Math.max(0, Math.round(scroller.clientHeight - TOP_GAP - below));
    }
    if (room !== this.room) {
      this.room = room;
      scroller.style.setProperty("--tail-room", `${String(room)}px`);
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
      placeAt(this.app.scroller, target, TOP_GAP);
    }
    return target !== null;
  }

  /** Scrolls the block holding source line `line` to the top of the pane. */
  scrollToLine(line: number): boolean {
    const block = blockAt(this.host, line);
    if (block) {
      placeAt(this.app.scroller, block, TOP_GAP);
    }
    return block !== null;
  }

  /** Where the reader is in the document (position.ts). */
  captureAnchor(): SavedPosition {
    return capture(this.host, this.app.scroller);
  }

  /** Scrolls back to a captured position (position.ts). */
  restore(p: SavedPosition): void {
    restore(this.host, this.app.scroller, p);
  }

  /**
   * Scrolls so `el`'s top sits `at` pixels below the pane's top and holds it there while lazy
   * layout settles (reflow.ts), for the modules loaded on first use.
   */
  placeAt(el: HTMLElement, at: number): void {
    placeAt(this.app.scroller, el, at);
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
