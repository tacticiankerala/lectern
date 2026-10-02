// Full-text search (Ctrl+Shift+F, spec §6): an overlay with an input and the library's matching
// lines, grouped by file: its title, path, hit count, then each line's snippet with the hits
// marked. Typing searches again after a pause. ↑/↓ choose a file or a line, Enter or a click opens
// it at that line with the find bar on the query, Esc or a click outside closes. Loaded on first
// use.
import { h } from "./dom";
import type { FileHits } from "./generated/FileHits";
import type { SearchHit } from "./generated/SearchHit";

export interface SearchHost {
  search(query: string): Promise<FileHits[]>;
  /** Opens a result: the file, at `line` when given, then the find bar on `query`. */
  open(path: string, line: number | null, query: string): void;
}

/** How long typing must pause before the library is searched again. */
const DEBOUNCE_MS = 150;
const LIST_ID = "lx-sp-list";

/** What a row opens. */
interface Row {
  path: string;
  line: number | null;
}

export class SearchPanel {
  private readonly backdrop: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLElement;
  private rows: Row[] = [];
  /** The query the results on show answer. */
  private shown = "";
  private selected = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** Bumped by every search, so only the latest answer shows. */
  private searches = 0;
  private returnFocus: Element | null = null;

  constructor(
    root: HTMLElement,
    private readonly host: SearchHost,
  ) {
    this.input = h("input", {
      type: "text",
      class: "sp-input",
      placeholder: "Search the library…",
      "aria-label": "Search the library",
      role: "combobox",
      "aria-expanded": "true",
      "aria-controls": LIST_ID,
      spellcheck: "false",
      autocomplete: "off",
    });
    this.list = h("ul", { class: "sp-list", id: LIST_ID, role: "listbox" });
    const panel = h(
      "div",
      { class: "search-panel", role: "dialog", "aria-label": "Search" },
      this.input,
      this.list,
    );
    this.backdrop = h("div", { class: "sp-backdrop" }, panel);
    this.backdrop.hidden = true;
    root.append(this.backdrop);

    this.input.addEventListener("input", () => {
      this.schedule();
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
      const index = rowIndex(e.target);
      if (index !== null) {
        this.pick(index);
      }
    });
    this.list.addEventListener("mousemove", (e) => {
      const index = rowIndex(e.target);
      if (index !== null && index !== this.selected) {
        this.select(index);
      }
    });
  }

  get isOpen(): boolean {
    return !this.backdrop.hidden;
  }

  /** Shows the panel and searches for `query`, or again for the last query. */
  open(query?: string): void {
    if (!this.isOpen) {
      this.returnFocus = document.activeElement;
      this.backdrop.hidden = false;
    }
    if (query !== undefined) {
      this.input.value = query;
    }
    this.input.focus();
    this.input.select();
    void this.search();
  }

  close(): void {
    if (!this.isOpen) {
      return;
    }
    this.cancel();
    ++this.searches;
    this.backdrop.hidden = true;
    // Hidden, it shouldn't keep the keyboard, wherever focus goes next.
    this.input.blur();
    if (this.returnFocus instanceof HTMLElement && this.returnFocus.isConnected) {
      this.returnFocus.focus({ preventScroll: true });
    }
  }

  private schedule(): void {
    this.cancel();
    this.timer = setTimeout(() => {
      void this.search();
    }, DEBOUNCE_MS);
  }

  private cancel(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private async search(): Promise<void> {
    this.cancel();
    const search = ++this.searches;
    const query = this.input.value;
    if (query.trim() === "") {
      this.rows = [];
      this.list.replaceChildren(h("li", { class: "sp-empty" }, "Type to search every file"));
      this.select(0);
      return;
    }
    let results: FileHits[];
    try {
      results = await this.host.search(query);
    } catch (e) {
      if (search === this.searches) {
        this.rows = [];
        this.list.replaceChildren(h("li", { class: "sp-empty" }, `Search failed: ${String(e)}`));
      }
      return;
    }
    if (search !== this.searches) {
      return;
    }
    this.render(query, results);
  }

  private render(query: string, results: FileHits[]): void {
    this.shown = query;
    this.rows = [];
    if (results.length === 0) {
      this.list.replaceChildren(h("li", { class: "sp-empty" }, "No matches"));
      this.select(0);
      return;
    }
    const items: HTMLElement[] = [];
    for (const file of results) {
      const count = `${String(file.total)} ${file.total === 1 ? "line" : "lines"}`;
      items.push(
        this.row(
          { path: file.path, line: file.hits[0]?.line ?? null },
          "sp-file",
          file.path,
          h("span", { class: "sp-title" }, file.title),
          h("span", { class: "sp-rel" }, file.rel),
          h("span", { class: "sp-count" }, count),
        ),
      );
      for (const hit of file.hits) {
        items.push(
          this.row(
            { path: file.path, line: hit.line },
            "sp-hit",
            `${file.rel}:${String(hit.line)}`,
            h("span", { class: "sp-line" }, String(hit.line)),
            h("span", { class: "sp-snippet" }, ...snippet(hit)),
          ),
        );
      }
      const more = file.total - file.hits.length;
      if (more > 0) {
        items.push(h("li", { class: "sp-more", role: "presentation" }, `${String(more)} more`));
      }
    }
    this.list.replaceChildren(...items);
    this.list.scrollTop = 0;
    this.select(0);
  }

  /** A row that opens `target`. */
  private row(target: Row, cls: string, title: string, ...children: HTMLElement[]): HTMLElement {
    const index = this.rows.push(target) - 1;
    return h(
      "li",
      {
        class: cls,
        role: "option",
        id: `lx-sp-${String(index)}`,
        "data-index": String(index),
        "aria-selected": "false",
        title,
      },
      ...children,
    );
  }

  private select(index: number): void {
    const items = this.list.querySelectorAll<HTMLElement>("[data-index]");
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
    const row = this.rows[index];
    if (row) {
      this.close();
      this.host.open(row.path, row.line, this.shown);
    }
  }

  private onKey(e: KeyboardEvent): void {
    if (e.isComposing) {
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        this.select(this.selected + 1);
        break;
      case "ArrowUp":
        this.select(this.selected - 1);
        break;
      case "Enter":
        if (this.timer !== null || this.input.value !== this.shown) {
          // Typed faster than the pause: the results catch up, then the first one opens.
          const query = this.input.value;
          void this.search().then(() => {
            if (this.isOpen && this.shown === query) {
              this.pick(this.selected);
            }
          });
        } else {
          this.pick(this.selected);
        }
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

/** The index of the row an event happened on. */
function rowIndex(target: EventTarget | null): number | null {
  const row = target instanceof Element ? target.closest<HTMLElement>("[data-index]") : null;
  return row?.dataset.index === undefined ? null : Number(row.dataset.index);
}

/** A hit's snippet: its text, with the hits in `<mark>`. */
function snippet(hit: SearchHit): Node[] {
  return hit.segments.map((s) => (s.hit ? h("mark", {}, s.text) : document.createTextNode(s.text)));
}
