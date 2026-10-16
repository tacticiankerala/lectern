// The outline: the document's headings, indented by level, with a scrollspy that marks the heading
// being read and keeps it in view. It fills the right panel's Outline pane (right-panel.ts), which
// is its scroll container.
import { HEADINGS, h } from "./dom";
import type { OutlineItem } from "./generated/OutlineItem";

/**
 * The top part of the pane a heading is being read in, as a percentage of the pane's height. The
 * scrollspy marks the first heading in it, else the last scrolled past; agent.ts answers `status`
 * by the same rule.
 */
export const READING_BAND_PERCENT = 30;

export class Outline {
  private observer: IntersectionObserver | null = null;
  /** Each outline entry, by its heading's id. */
  private readonly entries = new Map<string, HTMLLIElement>();
  private active: HTMLLIElement | null = null;

  constructor(
    private readonly host: HTMLElement,
    private readonly scroller: HTMLElement,
    /** A heading was clicked: its id, and its place in the list (two may share an id). */
    private readonly pick: (id: string, index: number) => void,
  ) {
    host.addEventListener("click", (e) => {
      const link = e.target instanceof Element ? e.target.closest("a") : null;
      const id = link?.dataset.id;
      const index = Number(link?.dataset.index);
      if (id === undefined || !Number.isInteger(index)) {
        return;
      }
      e.preventDefault();
      this.markEntry(link?.closest("li") ?? null);
      this.pick(id, index);
    });
  }

  /** Lists `items` and starts following the headings of `doc` as it scrolls. */
  render(items: OutlineItem[], doc: HTMLElement): void {
    this.clear();
    if (items.length === 0) {
      return;
    }
    const top = Math.min(...items.map((item) => item.level));
    const list = h("ul", { class: "outline-list" });
    for (const [index, item] of items.entries()) {
      const li = h("li");
      li.style.setProperty("--depth", String(item.level - top));
      // A heading of emoji alone has no id to scroll to.
      if (item.id === "") {
        li.append(h("span", {}, item.text));
      } else {
        li.append(
          h(
            "a",
            { href: "#", "data-id": item.id, "data-index": String(index), title: item.text },
            item.text,
          ),
        );
        this.entries.set(item.id, li);
      }
      list.append(li);
    }
    this.host.replaceChildren(h("div", { class: "outline-title" }, "On this page"), list);
    this.watch(doc);
  }

  /** Shows `message` in place of an outline Lectern can't list (a PDF's). */
  note(message: string): void {
    this.clear();
    this.host.replaceChildren(h("p", { class: "outline-note" }, message));
  }

  clear(): void {
    this.observer?.disconnect();
    this.observer = null;
    this.entries.clear();
    this.active = null;
    this.host.replaceChildren();
  }

  /**
   * Marks the topmost heading inside the top part of the pane (READING_BAND_PERCENT); when none
   * is, the last heading scrolled past.
   */
  private watch(doc: HTMLElement): void {
    if (typeof IntersectionObserver === "undefined") {
      return;
    }
    const headings = [...doc.querySelectorAll<HTMLElement>(HEADINGS)].filter((el) =>
      this.entries.has(el.id),
    );
    const visible = new Set<HTMLElement>();
    this.observer = new IntersectionObserver(
      (records) => {
        for (const record of records) {
          const el = record.target as HTMLElement;
          if (record.isIntersecting) {
            visible.add(el);
          } else {
            visible.delete(el);
          }
        }
        const current = headings.find((el) => visible.has(el)) ?? this.lastAbove(headings);
        if (current) {
          this.mark(current.id);
        }
      },
      { root: this.scroller, rootMargin: `0px 0px -${String(100 - READING_BAND_PERCENT)}% 0px` },
    );
    for (const el of headings) {
      this.observer.observe(el);
    }
  }

  /** The last heading above the pane's top, by binary search: headings come in page order. */
  private lastAbove(headings: HTMLElement[]): HTMLElement | undefined {
    const top = this.scroller.getBoundingClientRect().top;
    let lo = 0;
    let hi = headings.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const el = headings[mid];
      if (el && el.getBoundingClientRect().top < top) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    return headings[lo - 1];
  }

  private mark(id: string): void {
    this.markEntry(this.entries.get(id) ?? null);
  }

  /** Marks `li`, the entry of the heading being read, and keeps it in view in the pane. */
  private markEntry(li: HTMLLIElement | null): void {
    if (!li || li === this.active) {
      return;
    }
    this.active?.classList.remove("active");
    li.classList.add("active");
    this.active = li;
    // Keep it in view in the pane, which is the entries' offset parent.
    const host = this.host;
    if (
      li.offsetTop < host.scrollTop ||
      li.offsetTop + li.offsetHeight > host.scrollTop + host.clientHeight
    ) {
      host.scrollTop = li.offsetTop - host.clientHeight / 3;
    }
  }
}
