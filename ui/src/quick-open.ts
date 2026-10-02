// Quick open (Ctrl+P, spec §6): an overlay with an input and the best fuzzy matches among the
// library's Markdown files, recent files first while the query is empty. ↑/↓ choose, Enter opens,
// Esc or a click outside closes. Loaded on first use.
import { h } from "./dom";
import { fuzzyFilter, type Scored } from "./fuzzy";
import type { Candidate } from "./generated/Candidate";
import type { RecentEntry } from "./generated/RecentEntry";

export interface QuickOpenHost {
  /** The library's files; the host caches them until the library changes. */
  candidates(): Promise<Candidate[]>;
  recent(): RecentEntry[];
  open(path: string): void;
}

const LIMIT = 50;
const LIST_ID = "lx-qo-list";

function key(path: string): string {
  return path.toLowerCase().replaceAll("/", "\\");
}

function baseName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

const toKey = (c: Candidate) => ({ name: c.name, rel: c.rel });

export class QuickOpen {
  private readonly backdrop: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLElement;
  private candidates: Candidate[] = [];
  private results: Scored<Candidate>[] = [];
  private selected = 0;
  private returnFocus: Element | null = null;
  /** Bumped by every fetch of the candidates, so only the latest answer is used. */
  private fetches = 0;

  constructor(
    root: HTMLElement,
    private readonly host: QuickOpenHost,
  ) {
    this.input = h("input", {
      type: "text",
      class: "qo-input",
      placeholder: "Go to file…",
      "aria-label": "File name",
      role: "combobox",
      "aria-expanded": "true",
      "aria-controls": LIST_ID,
      spellcheck: "false",
      autocomplete: "off",
    });
    this.list = h("ul", { class: "qo-list", id: LIST_ID, role: "listbox" });
    const panel = h(
      "div",
      { class: "quick-open", role: "dialog", "aria-label": "Quick open" },
      this.input,
      this.list,
    );
    this.backdrop = h("div", { class: "qo-backdrop" }, panel);
    this.backdrop.hidden = true;
    root.append(this.backdrop);

    this.input.addEventListener("input", () => {
      this.update();
    });
    this.input.addEventListener("keydown", (e) => {
      this.onKey(e);
    });
    this.backdrop.addEventListener("pointerdown", (e) => {
      if (e.target === this.backdrop) {
        this.close();
      }
    });
    this.list.addEventListener("click", (e) => {
      const item = e.target instanceof Element ? e.target.closest<HTMLElement>(".qo-item") : null;
      if (item?.dataset.index !== undefined) {
        this.pick(Number(item.dataset.index));
      }
    });
    this.list.addEventListener("mousemove", (e) => {
      const item = e.target instanceof Element ? e.target.closest<HTMLElement>(".qo-item") : null;
      if (item?.dataset.index !== undefined && Number(item.dataset.index) !== this.selected) {
        this.select(Number(item.dataset.index));
      }
    });
  }

  get isOpen(): boolean {
    return !this.backdrop.hidden;
  }

  open(): void {
    if (!this.isOpen) {
      this.returnFocus = document.activeElement;
      this.backdrop.hidden = false;
      this.input.value = "";
    }
    this.input.focus();
    this.input.select();
    this.update();
    this.fetch();
  }

  /** The files changed: fetches them again, keeping the query and the chosen file. */
  refresh(): void {
    this.fetch();
  }

  private fetch(): void {
    const fetch = ++this.fetches;
    void this.host.candidates().then(
      (candidates) => {
        if (fetch !== this.fetches) return;
        this.candidates = candidates;
        if (this.isOpen) this.update(true);
      },
      () => undefined,
    );
  }

  close(): void {
    if (!this.isOpen) {
      return;
    }
    this.backdrop.hidden = true;
    this.list.replaceChildren();
    // Hidden, it shouldn't keep the keyboard, wherever focus goes next.
    this.input.blur();
    if (this.returnFocus instanceof HTMLElement && this.returnFocus.isConnected) {
      this.returnFocus.focus({ preventScroll: true });
    }
  }

  /** The matches for the current query; recent files, then the rest, when it is empty. */
  private update(keepChoice = false): void {
    const chosen = keepChoice ? this.results[this.selected]?.item.path : undefined;
    const query = this.input.value.trim();
    if (query === "") {
      const known = new Map(this.candidates.map((c) => [key(c.path), c]));
      const recent = this.host.recent().map(
        (r): Candidate =>
          known.get(key(r.path)) ?? {
            path: r.path,
            name: baseName(r.path),
            rel: r.path,
            root: "",
          },
      );
      const seen = new Set(recent.map((c) => key(c.path)));
      const rest = this.candidates.filter((c) => !seen.has(key(c.path)));
      this.results = [...recent, ...rest]
        .slice(0, LIMIT)
        .map((item) => ({ item, score: 0, positions: [] }));
    } else {
      this.results = fuzzyFilter(query, this.candidates, toKey, LIMIT);
    }
    this.render();
    const keep = chosen === undefined ? -1 : this.results.findIndex((r) => r.item.path === chosen);
    this.select(Math.max(keep, 0));
  }

  private render(): void {
    if (this.results.length === 0) {
      const empty = this.candidates.length === 0 ? "No files in the library yet" : "No matches";
      this.list.replaceChildren(h("li", { class: "qo-empty" }, empty));
      return;
    }
    this.list.replaceChildren(
      ...this.results.map(({ item, positions }, i) => {
        const nameStart = item.rel.endsWith(item.name) ? item.rel.length - item.name.length : 0;
        const dir = item.rel.slice(0, nameStart).replace(/\/$/, "");
        return h(
          "li",
          {
            class: "qo-item",
            role: "option",
            id: `lx-qo-${String(i)}`,
            "data-index": String(i),
            "aria-selected": "false",
            title: item.path,
          },
          h("span", { class: "qo-name" }, ...marked(item.name, positions, nameStart)),
          h("span", { class: "qo-path" }, ...marked(dir, positions, 0)),
        );
      }),
    );
  }

  private select(index: number): void {
    const items = this.list.querySelectorAll<HTMLElement>(".qo-item");
    if (items.length === 0) {
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    this.selected = (index + items.length) % items.length;
    items.forEach((item, i) => {
      item.setAttribute("aria-selected", String(i === this.selected));
    });
    const current = items[this.selected];
    if (current) {
      this.input.setAttribute("aria-activedescendant", current.id);
      // The list is the items' offset parent.
      const list = this.list;
      if (current.offsetTop < list.scrollTop) {
        list.scrollTop = current.offsetTop;
      } else if (current.offsetTop + current.offsetHeight > list.scrollTop + list.clientHeight) {
        list.scrollTop = current.offsetTop + current.offsetHeight - list.clientHeight;
      }
    }
  }

  private pick(index: number): void {
    const result = this.results[index];
    if (result) {
      this.close();
      this.host.open(result.item.path);
    }
  }

  private onKey(e: KeyboardEvent): void {
    switch (e.key) {
      case "ArrowDown":
        this.select(this.selected + 1);
        break;
      case "ArrowUp":
        this.select(this.selected - 1);
        break;
      case "Enter":
        this.pick(this.selected);
        break;
      case "Escape":
        this.close();
        break;
      default:
        return;
    }
    e.preventDefault();
  }
}

/**
 * `text` as text nodes and `<mark>`s for the matched characters: `positions` index the string
 * `text` starts at `offset` in, and consecutive matches share one mark.
 */
function marked(text: string, positions: number[], offset: number): Node[] {
  const hit = new Set(positions.map((p) => p - offset).filter((p) => p >= 0 && p < text.length));
  const nodes: Node[] = [];
  let i = 0;
  while (i < text.length) {
    let j = i;
    const on = hit.has(i);
    while (j < text.length && hit.has(j) === on) j++;
    const part = text.slice(i, j);
    nodes.push(on ? h("mark", {}, part) : document.createTextNode(part));
    i = j;
  }
  return nodes;
}
