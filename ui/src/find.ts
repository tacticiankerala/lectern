// Find in page (Ctrl+F, spec §6): a bar at the top right of the document pane. Matches are painted
// with the CSS Custom Highlight API (`find`, and `find-current` for the chosen one), so nothing is
// added to the document or splits its text. Typing searches again after a pause, Enter and
// Shift+Enter move through the matches, Esc closes. It searches again whenever the document
// re-renders. Loaded on first use.
import { h } from "./dom";

/** The most matches highlighted; past it the count reads "5000+". */
const MAX_MATCHES = 5000;
/** How long typing must pause before the page is searched again. */
const DEBOUNCE_MS = 60;
/** Where the current match lands: this far below the pane's top at least, when centring can't. */
const MIN_GAP = 12;

/** Elements that start a new run of text: a match never spans two of them. */
const BLOCKS = new Set([
  "ADDRESS",
  "ARTICLE",
  "ASIDE",
  "BLOCKQUOTE",
  "BR",
  "CAPTION",
  "DD",
  "DETAILS",
  "DIV",
  "DL",
  "DT",
  "FIGCAPTION",
  "FIGURE",
  "FOOTER",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "HEADER",
  "HR",
  "LI",
  "OL",
  "P",
  "PRE",
  "SECTION",
  "SUMMARY",
  "TABLE",
  "TD",
  "TH",
  "TR",
  "UL",
]);

/** Splits runs of text from different blocks; no query can hold it. */
const BREAK = "\u0000";

/**
 * The matches of `query` in the text of `root`, in document order and without overlaps, at most
 * `limit` of them. Smart case, as the library search: an upper-case letter makes it exact,
 * otherwise case is ignored. A match may run across inline elements but never across blocks, and
 * any whitespace matches any other, as the page renders a line break in a paragraph as a space.
 * Code blocks' labels and copy buttons (`.code-head`) aren't searched.
 */
export function findRanges(root: HTMLElement, query: string, limit = Infinity): Range[] {
  if (query.trim() === "") {
    return [];
  }
  const exact = /\p{Uppercase}/u.test(query);
  const nodes: Text[] = [];
  const starts: number[] = [];
  let text = "";
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node instanceof Element && node.classList.contains("code-head")
        ? NodeFilter.FILTER_REJECT
        : NodeFilter.FILTER_ACCEPT,
  });
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node instanceof Text) {
      nodes.push(node);
      starts.push(text.length);
      text += node.data;
    } else if (node instanceof Element && BLOCKS.has(node.tagName)) {
      text += BREAK;
    }
  }
  const haystack = fold(text.replace(/\s/g, " "), exact);
  const needle = fold(query.replace(/\s/g, " "), exact);
  const ranges: Range[] = [];
  for (
    let at = haystack.indexOf(needle);
    at !== -1 && ranges.length < limit;
    at = haystack.indexOf(needle, at + needle.length)
  ) {
    const first = nodeAt(starts, at);
    const last = nodeAt(starts, at + needle.length - 1);
    const startNode = nodes[first];
    const endNode = nodes[last];
    if (!startNode || !endNode) {
      break;
    }
    const range = document.createRange();
    range.setStart(startNode, at - (starts[first] ?? 0));
    range.setEnd(endNode, at + needle.length - (starts[last] ?? 0));
    ranges.push(range);
  }
  return ranges;
}

/**
 * `s` lower-cased, unless `exact`, keeping each character where it was: the few that lower-case to
 * two (İ) stay as they are.
 */
function fold(s: string, exact: boolean): string {
  if (exact) {
    return s;
  }
  const lower = s.toLowerCase();
  if (lower.length === s.length) {
    return lower;
  }
  let out = "";
  for (const ch of s) {
    const l = ch.toLowerCase();
    out += l.length === ch.length ? l : ch;
  }
  return out;
}

/** The index of the text node holding offset `at`: the last one starting at or before it. */
function nodeAt(starts: number[], at: number): number {
  let lo = 0;
  let hi = starts.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((starts[mid] ?? Infinity) <= at) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  return lo - 1;
}

/** What the find bar needs from the app. */
export interface FindHost {
  /** The document on screen, or null when there is none (the welcome screen, an error). */
  doc(): HTMLElement | null;
  readonly scroller: HTMLElement;
  /** Scrolls so `el`'s top sits `at` pixels below the pane's top, held while layout settles. */
  placeAt(el: HTMLElement, at: number): void;
  /** Calls `cb` whenever the document area re-renders. */
  onRender(cb: () => void): void;
}

const ICONS = {
  prev: '<path d="M3.5 10 8 5.5l4.5 4.5"/>',
  next: '<path d="M3.5 6 8 10.5 12.5 6"/>',
  close: '<path d="m4 4 8 8M12 4l-8 8"/>',
};

function button(label: string, keys: string, icon: string): HTMLButtonElement {
  const b = h("button", {
    type: "button",
    class: "icon-btn",
    title: `${label} (${keys})`,
    "aria-label": label,
  });
  b.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">${icon}</svg>`;
  return b;
}

const supported = (): boolean => typeof Highlight === "function" && "highlights" in CSS;

export class FindBar {
  private readonly bar: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly counter: HTMLElement;
  private ranges: Range[] = [];
  /** More matches than were highlighted. */
  private more = false;
  private current = -1;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private returnFocus: Element | null = null;

  constructor(
    pane: HTMLElement,
    private readonly host: FindHost,
  ) {
    this.input = h("input", {
      type: "text",
      class: "find-input",
      placeholder: "Find in page",
      "aria-label": "Find",
      spellcheck: "false",
      autocomplete: "off",
    });
    this.counter = h("span", { class: "find-count", role: "status" });
    const prev = button("Previous match", "Shift+Enter", ICONS.prev);
    const next = button("Next match", "Enter", ICONS.next);
    const close = button("Close", "Esc", ICONS.close);
    this.bar = h(
      "div",
      { class: "find-bar", role: "search", "aria-label": "Find in page" },
      this.input,
      this.counter,
      prev,
      next,
      close,
    );
    this.bar.hidden = true;
    // A sticky dock of no height at the top of the pane holds the bar over the text.
    pane.prepend(h("div", { class: "find-dock" }, this.bar));

    this.input.addEventListener("input", () => {
      this.schedule();
    });
    this.input.addEventListener("keydown", (e) => {
      this.onKey(e);
    });
    prev.addEventListener("click", () => {
      this.prev();
    });
    next.addEventListener("click", () => {
      this.next();
    });
    close.addEventListener("click", () => {
      this.close();
    });
    host.onRender(() => {
      if (this.isOpen) {
        this.run(false);
      }
    });
  }

  get isOpen(): boolean {
    return !this.bar.hidden;
  }

  /** How many matches are highlighted. */
  get count(): number {
    return this.ranges.length;
  }

  /** The current match's index, or -1 without one. */
  get index(): number {
    return this.current;
  }

  /**
   * Shows the bar with the input focused, searching for `prefill` if given, else for the last
   * query. The first match from the top of the pane down becomes current, scrolled to if the
   * reader can't see it. With `near` (the block of a search result's line) the match in it becomes
   * current instead; when it has none, the nearest one does, and nothing scrolls, as the reader is
   * already at the line.
   */
  open(prefill?: string, near?: Element): void {
    const wasOpen = this.isOpen;
    if (!wasOpen) {
      this.returnFocus = document.activeElement;
      this.bar.hidden = false;
    }
    if (prefill !== undefined) {
      this.input.value = prefill;
    }
    this.input.focus();
    this.input.select();
    if (!wasOpen || prefill !== undefined || near) {
      this.run(true, near);
    }
  }

  /** Hides the bar and clears the highlights. */
  close(): void {
    if (!this.isOpen) {
      return;
    }
    this.cancel();
    this.bar.hidden = true;
    this.ranges = [];
    this.current = -1;
    if (supported()) {
      CSS.highlights.delete("find");
      CSS.highlights.delete("find-current");
    }
    this.input.blur();
    if (this.returnFocus instanceof HTMLElement && this.returnFocus.isConnected) {
      this.returnFocus.focus({ preventScroll: true });
    }
  }

  next(): void {
    this.step(1);
  }

  prev(): void {
    this.step(-1);
  }

  private schedule(): void {
    this.cancel();
    this.timer = setTimeout(() => {
      this.run(true);
    }, DEBOUNCE_MS);
  }

  private cancel(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  /**
   * Searches the document for the input's text and highlights the matches. With `reveal` (the
   * reader asked), a current match the reader can't see is scrolled to; a re-render never
   * scrolls, so the reading position holds. With `near`, see `open`.
   */
  private run(reveal: boolean, near?: Element): void {
    this.cancel();
    const doc = this.host.doc();
    const found = doc ? findRanges(doc, this.input.value, MAX_MATCHES + 1) : [];
    this.more = found.length > MAX_MATCHES;
    this.ranges = this.more ? found.slice(0, MAX_MATCHES) : found;
    this.current = -1;
    if (doc && this.ranges.length > 0) {
      this.current = near ? this.nearest(doc, near) : this.firstOnScreen(doc);
    }
    if (supported()) {
      if (this.ranges.length > 0) {
        CSS.highlights.set("find", new Highlight(...this.ranges));
      } else {
        CSS.highlights.delete("find");
      }
    }
    this.showCurrent();
    const range = this.ranges[this.current];
    // A match outside the search result's block isn't worth leaving its line for.
    const wanted = !near || (range !== undefined && near.contains(range.startContainer));
    if (doc && reveal && wanted && range && !this.visible(doc, range)) {
      this.reveal(range);
    }
  }

  /** Moves to the next or previous match, wrapping around, and scrolls it to the middle. */
  private step(by: 1 | -1): void {
    if (this.timer !== null) {
      this.run(false);
    }
    const n = this.ranges.length;
    if (n === 0) {
      return;
    }
    this.current = (this.current + by + n) % n;
    this.showCurrent();
    const range = this.ranges[this.current];
    if (range) {
      this.reveal(range);
    }
  }

  /** Paints the current match and shows where it is among them. */
  private showCurrent(): void {
    const range = this.ranges[this.current];
    if (supported()) {
      if (range) {
        const current = new Highlight(range);
        current.priority = 1;
        CSS.highlights.set("find-current", current);
      } else {
        CSS.highlights.delete("find-current");
      }
    }
    if (this.input.value.trim() === "") {
      this.counter.textContent = "";
    } else if (this.ranges.length === 0) {
      this.counter.textContent = "No results";
    } else {
      const total = `${String(this.ranges.length)}${this.more ? "+" : ""}`;
      this.counter.textContent = `${String(this.current + 1)} / ${total}`;
    }
  }

  /**
   * The first match that ends below the pane's top. Found by binary search over the matches'
   * top-level blocks, which, unlike text inside a block content-visibility skipped, always lie in
   * page order; then forward within the block that straddles the top.
   */
  private firstOnScreen(doc: HTMLElement): number {
    const top = this.host.scroller.getBoundingClientRect().top;
    let lo = 0;
    let hi = this.ranges.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const block = topBlock(doc, this.ranges[mid]);
      if (block && block.getBoundingClientRect().bottom <= top) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    let i = lo;
    while (
      i < this.ranges.length - 1 &&
      (this.ranges[i]?.getBoundingClientRect().bottom ?? 0) <= top
    ) {
      i++;
    }
    return i < this.ranges.length ? i : 0;
  }

  /**
   * The first match in `near`, else the closest one before or after it, by the distance between
   * their blocks. Matches lie in page order, so the first at or after `near`'s start is found by
   * binary search.
   */
  private nearest(doc: HTMLElement, near: Element): number {
    let lo = 0;
    let hi = this.ranges.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      // 1: `near` starts after this match ends.
      if (this.ranges[mid]?.comparePoint(near, 0) === 1) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    const next = this.ranges[lo];
    if (lo === 0 || (next && near.contains(next.startContainer))) {
      return lo;
    }
    if (!next) {
      return lo - 1;
    }
    const at = near.getBoundingClientRect();
    const above = topBlock(doc, this.ranges[lo - 1])?.getBoundingClientRect().bottom ?? -Infinity;
    const below = topBlock(doc, next)?.getBoundingClientRect().top ?? Infinity;
    return at.top - above < below - at.bottom ? lo - 1 : lo;
  }

  /**
   * Whether the reader can see the match: inside the pane, not in a closed `<details>`, and not
   * scrolled out of sight sideways in a block that scrolls (a long line of code).
   */
  private visible(doc: HTMLElement, range: Range): boolean {
    const r = range.getBoundingClientRect();
    const pane = this.host.scroller.getBoundingClientRect();
    const el = range.startContainer.parentElement;
    if (!el || r.height === 0 || r.top < pane.top || r.bottom > pane.bottom) {
      return false;
    }
    if (closedAround(doc, el).length > 0) {
      return false;
    }
    for (let box: HTMLElement | null = el; box && box !== doc; box = box.parentElement) {
      if (box.scrollWidth > box.clientWidth) {
        const b = box.getBoundingClientRect();
        if (r.left < b.left || r.right > b.right) {
          return false;
        }
      }
    }
    return true;
  }

  /**
   * Scrolls the match to the middle of the pane, held there while lazy layout settles. A closed
   * `<details>` around it opens, as browsers' find does, and its block is laid out for real first,
   * so it is measured where it will stay.
   */
  private reveal(range: Range): void {
    const doc = this.host.doc();
    const el = range.startContainer.parentElement;
    if (!doc || !el) {
      return;
    }
    for (const closed of closedAround(doc, el)) {
      closed.open = true;
    }
    const block = topBlock(doc, range);
    if (block instanceof HTMLElement) {
      block.style.contentVisibility = "visible";
    }
    const match = range.getBoundingClientRect();
    const into = match.top - el.getBoundingClientRect().top;
    const at = Math.max(MIN_GAP, (this.host.scroller.clientHeight - match.height) / 2);
    this.host.placeAt(el, at - into);
    // Off to the side in a block that scrolls sideways (a long line of code), it comes into view.
    for (let box: HTMLElement | null = el; box && box !== doc; box = box.parentElement) {
      if (box.scrollWidth > box.clientWidth) {
        const b = box.getBoundingClientRect();
        if (match.left < b.left || match.right > b.right) {
          box.scrollLeft += match.left - b.left - (b.width - match.width) / 2;
        }
      }
    }
  }

  private onKey(e: KeyboardEvent): void {
    if (e.isComposing) {
      return;
    }
    if (e.key === "Enter") {
      this.step(e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape") {
      this.close();
    } else {
      return;
    }
    e.preventDefault();
  }
}

/** The closed `<details>` of `doc` that hide `el`: those it sits in, outside their summary. */
function closedAround(doc: HTMLElement, el: Element): HTMLDetailsElement[] {
  const closed: HTMLDetailsElement[] = [];
  for (
    let details = el.closest("details");
    details instanceof HTMLDetailsElement && doc.contains(details);
    details = details.parentElement?.closest("details") ?? null
  ) {
    if (!details.open && !details.querySelector(":scope > summary")?.contains(el)) {
      closed.push(details);
    }
  }
  return closed;
}

/** The child of `doc` holding the start of `range`. */
function topBlock(doc: HTMLElement, range: Range | undefined): Element | null {
  let node: Node | null = range?.startContainer ?? null;
  while (node && node.parentNode !== doc) {
    node = node.parentNode;
  }
  return node instanceof Element ? node : null;
}
