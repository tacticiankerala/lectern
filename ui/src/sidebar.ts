// The library sidebar (spec §6): a section per root with its name and state, and a lazily built
// tree. A folder's rows exist only while it is expanded; which folders are expanded is kept in
// localStorage. Clicking a folder's name opens its README (and expands it) when it has one, else
// toggles it; README folders carry their `status:` badge when the setting asks for them. The open
// document is highlighted, its folders expanded and its row scrolled into view. With no roots it
// offers to add a folder instead.
import { h, samePath } from "./dom";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { RootView } from "./generated/RootView";
import type { TreeNode } from "./generated/TreeNode";
import { badge } from "./properties";

export interface SidebarHost {
  /** Opens a document, pushing the one on screen onto the history. */
  open(path: string): void;
  retry(root: string): void;
  /** The context menu for a row: `root` when it is a root's own row. */
  contextMenu(e: MouseEvent, path: string, root: boolean): void;
  /** Asks for a folder and adds it to the library. */
  addFolder(): void;
}

/** A rendered row and what it shows. */
interface Row {
  node: TreeNode;
  el: HTMLElement;
  /** The `<li>` holding the row and, while expanded, its children's group. */
  item: HTMLElement;
  depth: number;
  /** The children's `<ul>`, while expanded. */
  group: HTMLElement | null;
  /** The root it belongs to, when it is the root's own row. */
  root: RootView | null;
}

const EXPANDED_KEY = "lx-expanded";
const COLLAPSED_ROOTS_KEY = "lx-collapsed-roots";

/** Paths compare as on Windows: case-insensitively, either separator. */
function key(path: string): string {
  return path.toLowerCase().replaceAll("/", "\\");
}

/** Whether `path` lies below folder `dir` (not equal to it). */
function isBelow(path: string, dir: string): boolean {
  const d = key(dir);
  return key(path).startsWith(d.endsWith("\\") ? d : `${d}\\`);
}

function loadSet(name: string): Set<string> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(name) ?? "[]");
    return new Set(Array.isArray(value) ? value.map(String) : []);
  } catch {
    return new Set();
  }
}

function saveSet(name: string, set: Set<string>): void {
  try {
    localStorage.setItem(name, JSON.stringify([...set]));
  } catch {
    // Without storage, folders start collapsed next time; nothing else changes.
  }
}

export class Sidebar {
  private library: LibraryPayload = { roots: [] };
  private readonly rows = new Map<string, Row>();
  private readonly expanded = loadSet(EXPANDED_KEY);
  private readonly collapsedRoots = loadSet(COLLAPSED_ROOTS_KEY);
  private active: string | null = null;
  private activeRow: HTMLElement | null = null;
  /** Nothing renders until `start`, after the first paint. */
  private started = false;
  /** README folders show their `status:` badge (a setting, on by default). */
  private badges = true;

  constructor(
    private readonly host: HTMLElement,
    private readonly app: SidebarHost,
  ) {
    host.addEventListener("click", (e) => {
      this.onClick(e);
    });
    host.addEventListener("contextmenu", (e) => {
      const el = e.target instanceof Element ? e.target.closest<HTMLElement>(".tree-row") : null;
      const row = el?.dataset.path === undefined ? undefined : this.rows.get(key(el.dataset.path));
      e.preventDefault();
      if (row) {
        this.app.contextMenu(e, row.node.path, row.root !== null);
      }
    });
  }

  /** Renders the library for the first time; until now updates were only stored. */
  start(): void {
    this.started = true;
    this.render();
    // Scrolled into view with the next frame's layout rather than forcing one now, as the window
    // waits for this to show.
    requestAnimationFrame(() => {
      if (this.activeRow) this.scrollTo(this.activeRow);
    });
  }

  setLibrary(library: LibraryPayload): void {
    this.library = library;
    if (this.started) {
      this.render();
    }
  }

  /** Shows or hides README folders' status badges. */
  setBadges(show: boolean): void {
    if (show === this.badges) {
      return;
    }
    this.badges = show;
    if (this.started) {
      this.render();
    }
  }

  /** Highlights the document at `path` (none for null), expanding its folders to show it. */
  setActive(path: string | null): void {
    this.active = path;
    if (!this.started) {
      return;
    }
    this.activeRow?.classList.remove("active");
    this.activeRow?.removeAttribute("aria-current");
    this.activeRow = null;
    const row = path === null ? null : this.show(path);
    if (row) {
      row.el.classList.add("active");
      row.el.setAttribute("aria-current", "page");
      this.activeRow = row.el;
      this.scrollTo(row.el);
    }
  }

  /** Brings the row of `path` (a folder, usually) into view and focuses it; false if it has none. */
  reveal(path: string): boolean {
    const row = this.show(path);
    if (row) {
      this.scrollTo(row.el);
      row.el.focus({ preventScroll: true });
    }
    return row !== null;
  }

  private render(): void {
    const focused = document.activeElement?.closest<HTMLElement>("#lx-library .tree-row");
    const focusedPath = focused?.dataset.path;
    const scrollTop = this.host.scrollTop;
    this.rows.clear();
    this.activeRow = null;
    const roots = this.library.roots;
    this.host.replaceChildren(
      ...(roots.length === 0 ? [this.emptyHint()] : roots.map((root) => this.section(root))),
    );
    // Setting it forces a layout, which the first render (at the top) can do without.
    if (scrollTop !== 0) {
      this.host.scrollTop = scrollTop;
    }
    if (this.active !== null) {
      const row = this.rows.get(key(this.active)) ?? this.show(this.active);
      if (row) {
        row.el.classList.add("active");
        row.el.setAttribute("aria-current", "page");
        this.activeRow = row.el;
      }
    }
    if (focusedPath !== undefined) {
      this.rows.get(key(focusedPath))?.el.focus({ preventScroll: true });
    }
  }

  /** With no roots: what the library is for, and a button to start one. */
  private emptyHint(): HTMLElement {
    const add = h("button", { type: "button", class: "btn" }, "Add folder…");
    add.addEventListener("click", () => {
      this.app.addFolder();
    });
    return h(
      "div",
      { class: "lib-empty" },
      h("p", { class: "lib-empty-title" }, "Add a folder to build your library"),
      add,
      h("p", { class: "lib-empty-note" }, "Lectern indexes the Markdown files in folders you add."),
    );
  }

  /**
   * A root's section: its row (name, badge, state), a note when its index stopped at the file
   * cap, and, unless collapsed, its tree.
   */
  private section(root: RootView): HTMLElement {
    const node: TreeNode = root.tree ?? {
      name: root.name,
      path: root.path,
      isDir: true,
      children: [],
      readme: null,
      status: null,
      comments: null,
    };
    const section = h("section", {
      class: "lib-root",
      "data-root": root.path,
      "aria-label": root.name,
    });
    const head = this.rowElement(node, 0, root.tree !== null);
    head.classList.add("lib-root-head");
    head.title = root.path;
    section.append(head);
    if (root.state.state === "scanning") {
      head.append(h("span", { class: "spinner", role: "status", title: "Scanning…" }));
    } else if (root.state.state === "unavailable") {
      const retry = h("button", { type: "button", class: "btn lib-retry" }, "Retry");
      retry.addEventListener("click", () => {
        this.app.retry(root.path);
      });
      section.append(
        h(
          "div",
          { class: "lib-unavailable" },
          h("span", { class: "lib-root-state" }, root.state.reason),
          retry,
        ),
      );
    }
    if (root.truncated) {
      section.append(
        h(
          "p",
          { class: "lib-truncated", role: "note" },
          "This folder holds too many files to index them all. Some are missing from the library, quick open and search.",
        ),
      );
    }
    const tree = h("ul", { class: "tree" });
    const row: Row = { node, el: head, item: section, depth: 0, group: null, root };
    this.rows.set(key(node.path), row);
    section.append(tree);
    if (root.tree && !this.collapsedRoots.has(key(root.path))) {
      this.fill(row, tree);
    } else {
      head.setAttribute("aria-expanded", "false");
    }
    return section;
  }

  /** A row: chevron for a folder, the name, and a README folder's badge when badges show. */
  private rowElement(node: TreeNode, depth: number, expandable: boolean): HTMLElement {
    const el = h("div", {
      class: node.isDir ? "tree-row dir" : "tree-row file",
      "data-path": node.path,
      tabindex: "-1",
    });
    el.style.setProperty("--depth", String(depth));
    if (node.isDir && expandable) {
      el.setAttribute("aria-expanded", "false");
      el.append(
        h("button", {
          type: "button",
          class: "tree-chevron",
          tabindex: "-1",
          "aria-hidden": "true",
        }),
      );
    } else {
      el.append(h("span", { class: "tree-spacer" }));
    }
    el.append(h("button", { type: "button", class: "tree-name", title: node.name }, node.name));
    if (this.badges && node.isDir && node.status !== null) {
      el.append(badge(node.status));
    }
    return el;
  }

  /** Builds the rows of `row`'s children into `group`, recursing into expanded folders. */
  private fill(row: Row, group: HTMLElement): void {
    row.group = group;
    row.el.setAttribute("aria-expanded", "true");
    for (const child of row.node.children) {
      const el = this.rowElement(child, row.depth + 1, child.children.length > 0);
      const item = h("li", {}, el);
      const childRow: Row = {
        node: child,
        el,
        item,
        depth: row.depth + 1,
        group: null,
        root: null,
      };
      this.rows.set(key(child.path), childRow);
      group.append(item);
      if (child.isDir && this.expanded.has(key(child.path))) {
        const sub = h("ul");
        item.append(sub);
        this.fill(childRow, sub);
      }
    }
  }

  private expand(row: Row): void {
    if (row.group || row.node.children.length === 0) {
      return;
    }
    if (row.root) {
      this.collapsedRoots.delete(key(row.node.path));
      saveSet(COLLAPSED_ROOTS_KEY, this.collapsedRoots);
      const tree = row.item.querySelector<HTMLElement>(":scope > ul.tree");
      if (tree) this.fill(row, tree);
    } else {
      this.expanded.add(key(row.node.path));
      saveSet(EXPANDED_KEY, this.expanded);
      const group = h("ul");
      row.item.append(group);
      this.fill(row, group);
    }
    if (this.active !== null && this.activeRow === null) {
      const active = this.rows.get(key(this.active));
      if (active) {
        active.el.classList.add("active");
        active.el.setAttribute("aria-current", "page");
        this.activeRow = active.el;
      }
    }
  }

  private collapse(row: Row): void {
    if (!row.group) {
      return;
    }
    if (row.root) {
      this.collapsedRoots.add(key(row.node.path));
      saveSet(COLLAPSED_ROOTS_KEY, this.collapsedRoots);
      row.group.replaceChildren();
    } else {
      this.expanded.delete(key(row.node.path));
      saveSet(EXPANDED_KEY, this.expanded);
      row.group.remove();
    }
    row.group = null;
    row.el.setAttribute("aria-expanded", "false");
    for (const [k, other] of this.rows) {
      if (isBelow(other.node.path, row.node.path)) {
        this.rows.delete(k);
        if (other.el === this.activeRow) this.activeRow = null;
      }
    }
  }

  private toggle(row: Row): void {
    if (row.group) {
      this.collapse(row);
    } else {
      this.expand(row);
    }
  }

  /**
   * The row of `path`, expanding the folders above it (and its root) as needed; null when no
   * root's tree holds it.
   */
  private show(path: string): Row | null {
    const root = this.library.roots
      .filter((r) => r.tree !== null && (samePath(r.path, path) || isBelow(path, r.path)))
      .sort((a, b) => b.path.length - a.path.length)[0];
    if (!root?.tree) {
      return null;
    }
    let row = this.rows.get(key(root.path));
    let node: TreeNode | undefined = root.tree;
    while (row && node && !samePath(node.path, path)) {
      this.expand(row);
      node = node.children.find((c) => samePath(c.path, path) || isBelow(path, c.path));
      row = node ? this.rows.get(key(node.path)) : undefined;
    }
    return row ?? null;
  }

  /** Scrolls the sidebar so `el` is in view, a third of the way down when it had to move. */
  private scrollTo(el: HTMLElement): void {
    const box = this.host.getBoundingClientRect();
    const rect = el.getBoundingClientRect();
    if (rect.top < box.top || rect.bottom > box.bottom) {
      this.host.scrollTop += rect.top - box.top - box.height / 3;
    }
  }

  private onClick(e: MouseEvent): void {
    const target = e.target instanceof Element ? e.target : null;
    const el = target?.closest<HTMLElement>(".tree-row");
    const row = el?.dataset.path === undefined ? undefined : this.rows.get(key(el.dataset.path));
    if (!row || !target) {
      return;
    }
    if (target.closest(".tree-chevron")) {
      this.toggle(row);
      return;
    }
    if (!target.closest(".tree-name")) {
      return;
    }
    const { node } = row;
    if (!node.isDir) {
      this.app.open(node.path);
    } else if (node.readme !== null) {
      this.expand(row);
      this.app.open(node.readme);
    } else {
      this.toggle(row);
    }
  }
}
