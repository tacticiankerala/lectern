// The breadcrumb chooser: a popover under a crumb that lists one folder of the library's tree,
// folders first (with their status badge when badges show), then Markdown files, in the tree's
// order. On the file's own crumb it lists the file's folder with the file marked, for moving to a
// sibling. A filter takes the keyboard: typing filters, ↑/↓ choose, Enter (or a click) opens a file
// and closes or goes into a folder and stays open, → goes in, ← from an empty filter or its start
// (or Backspace in an empty filter) goes up, and Esc or a click outside closes. At most 1,000 rows
// show; the filter searches them all. It works from the tree in hand, follows it as the library
// changes, and never touches the library sidebar. Loaded on first use.
import { pathKey, within } from "./breadcrumbs";
import { h, samePath } from "./dom";
import type { Crumb } from "./generated/Crumb";
import type { RootView } from "./generated/RootView";
import type { TreeNode } from "./generated/TreeNode";
import { badge } from "./properties";

export interface ChooserHost {
  /** Opens a document, pushing the one on screen onto the history. */
  open(path: string): void;
  /** Whether folders show their README's status badge (a setting). */
  badges(): boolean;
}

const LIST_ID = "lx-cc-list";
/** The gap kept between the popover and the window's edges, in pixels. */
const EDGE_PX = 8;
/** The most rows drawn at once; a huge folder says how many more the filter can reach. */
const MAX_ROWS = 1000;

/** What a crumb's chooser lists. */
export interface CrumbFolder {
  /** The folders from the root's tree down to the one listed, which comes last. */
  trail: TreeNode[];
  /** The document, marked in the list, for the file's own crumb. */
  current: string | null;
}

/**
 * What the chooser lists for crumb `index`: its folder (the file's own folder for the file's
 * crumb), or as far down towards it as the library's tree goes; null when no root's tree holds it.
 */
export function crumbFolder(roots: RootView[], crumbs: Crumb[], index: number): CrumbFolder | null {
  const crumb = crumbs[index];
  const isFile = index === crumbs.length - 1;
  const folder = isFile ? crumbs[index - 1] : crumb;
  if (!crumb || !folder) {
    return null;
  }
  const trail = folderTrail(roots, folder.path);
  return trail && { trail, current: isFile ? crumb.path : null };
}

/**
 * The folders from a root down to folder `path`, or as far towards it as the tree goes, in the
 * deepest root with a tree that holds it (roots may nest); null when none does.
 */
function folderTrail(roots: RootView[], path: string): TreeNode[] | null {
  const wanted = pathKey(path);
  let deepest: RootView | null = null;
  for (const root of roots) {
    const longer = root.path.length > (deepest?.path.length ?? -1);
    if (root.tree && longer && within(wanted, pathKey(root.path))) {
      deepest = root;
    }
  }
  return deepest?.tree ? trailTo(deepest.tree, wanted) : null;
}

/** The folders from `node` down to the one keyed `path`, or to the deepest the tree has. */
function trailTo(node: TreeNode, path: string): TreeNode[] {
  const child = node.children.find((c) => c.isDir && within(path, pathKey(c.path)));
  return child ? [node, ...trailTo(child, path)] : [node];
}

/** A folder's entries whose name holds `filter`, ignoring case, in the tree's order. */
export function chooserRows(folder: TreeNode, filter: string): TreeNode[] {
  const wanted = filter.trim().toLowerCase();
  return wanted === ""
    ? folder.children
    : folder.children.filter((node) => node.name.toLowerCase().includes(wanted));
}

export class CrumbChooser {
  /** The crumb it hangs from, while open. */
  anchor: HTMLElement | null = null;
  private readonly el: HTMLElement;
  private readonly up: HTMLButtonElement;
  private readonly where: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLElement;
  /** The folders from the root down to the one listed. */
  private trail: TreeNode[] = [];
  /** The document on screen, marked when its folder is listed. */
  private current: string | null = null;
  private rows: TreeNode[] = [];
  private selected = 0;

  constructor(
    root: HTMLElement,
    private readonly host: ChooserHost,
  ) {
    this.up = h("button", { type: "button", class: "icon-btn cc-up", title: "Up (←)" });
    this.up.innerHTML =
      '<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M8 13V3.5M3.5 8 8 3.5 12.5 8"/></svg>';
    this.up.setAttribute("aria-label", "Up");
    this.where = h("span", { class: "cc-path" });
    this.input = h("input", {
      type: "text",
      class: "cc-filter",
      placeholder: "Filter",
      "aria-label": "Filter",
      role: "combobox",
      "aria-expanded": "true",
      "aria-controls": LIST_ID,
      "aria-autocomplete": "list",
      spellcheck: "false",
      autocomplete: "off",
    });
    this.list = h("ul", { class: "cc-list", id: LIST_ID, role: "listbox" });
    this.el = h(
      "div",
      { class: "crumb-chooser", role: "dialog", "aria-label": "Choose a file" },
      h("div", { class: "cc-head" }, this.up, this.where),
      this.input,
      this.list,
    );
    this.el.hidden = true;
    root.append(this.el);

    this.input.addEventListener("input", () => {
      this.update();
    });
    this.input.addEventListener("keydown", (e) => {
      this.onKey(e);
    });
    // The filter keeps the keyboard whatever the mouse presses.
    for (const el of [this.up, this.list]) {
      el.addEventListener("mousedown", (e) => {
        e.preventDefault();
      });
    }
    this.up.addEventListener("click", () => {
      this.goUp();
    });
    this.list.addEventListener("click", (e) => {
      const row = this.rowAt(e.target);
      if (row !== null) {
        this.pick(row);
      }
    });
    this.list.addEventListener("mousemove", (e) => {
      const row = this.rowAt(e.target);
      if (row !== null && row !== this.selected) {
        this.select(row);
      }
    });
  }

  get isOpen(): boolean {
    return this.anchor !== null;
  }

  /** Shows `target`'s folder under `anchor`, the filter focused. */
  open(anchor: HTMLElement, target: CrumbFolder): void {
    this.anchor?.setAttribute("aria-expanded", "false");
    this.anchor = anchor;
    anchor.setAttribute("aria-expanded", "true");
    this.trail = [...target.trail];
    this.current = target.current;
    this.el.hidden = false;
    document.addEventListener("pointerdown", this.onOutside);
    this.place(anchor);
    this.show(target.current);
    this.input.focus();
  }

  /** Hides it; focus inside it goes back to the crumb. */
  close(): void {
    const anchor = this.anchor;
    if (!anchor) {
      return;
    }
    const hadFocus = this.el.contains(document.activeElement);
    this.anchor = null;
    anchor.setAttribute("aria-expanded", "false");
    this.el.hidden = true;
    this.list.replaceChildren();
    document.removeEventListener("pointerdown", this.onOutside);
    if (hadFocus) {
      this.input.blur();
      if (anchor.isConnected) {
        anchor.focus({ preventScroll: true });
      }
    }
  }

  /**
   * After the crumbs in `crumbs` are drawn again (the library or the document changed): hangs from
   * its crumb's new button and lists from `roots`' trees as they are now, the same folder and
   * filter while the folder is there, else the nearest folder above it. It closes when its crumb,
   * known by its place and path, has gone (another document shows), or no root holds the folder.
   */
  follow(crumbs: HTMLElement, roots: RootView[]): void {
    const old = this.anchor;
    if (!old) {
      return;
    }
    const anchor = crumbs.querySelector<HTMLElement>(
      `button.crumb[data-index="${old.dataset.index ?? ""}"]`,
    );
    const trail = folderTrail(roots, this.folder.path);
    if (!anchor || anchor.title !== old.title || !trail) {
      this.close();
      return;
    }
    this.anchor = anchor;
    anchor.setAttribute("aria-expanded", "true");
    const stays = samePath((trail[trail.length - 1] as TreeNode).path, this.folder.path);
    const chosen = this.rows[this.selected]?.path ?? null;
    this.trail = trail;
    if (stays) {
      this.head();
      this.update(chosen);
    } else {
      this.show(null);
    }
    this.place(anchor);
  }

  private readonly onOutside = (e: Event): void => {
    const target = e.target instanceof Node ? e.target : null;
    if (target && !this.el.contains(target) && !this.anchor?.contains(target)) {
      this.close();
    }
  };

  /** Under the crumb, inside the window. */
  private place(anchor: HTMLElement): void {
    const box = anchor.getBoundingClientRect();
    const room = window.innerWidth - this.el.offsetWidth - EDGE_PX;
    this.el.style.left = `${String(Math.max(EDGE_PX, Math.min(box.left, room)))}px`;
    this.el.style.top = `${String(box.bottom + 6)}px`;
  }

  private get folder(): TreeNode {
    return this.trail[this.trail.length - 1] as TreeNode;
  }

  /** Lists the folder at the end of the trail with an empty filter, choosing `choose` if listed. */
  private show(choose: string | null): void {
    this.input.value = "";
    this.head();
    this.update(choose);
  }

  /** The header: where the listed folder is, and whether there is a folder above it. */
  private head(): void {
    this.where.textContent = this.trail.map((node) => node.name).join(" / ");
    this.where.title = this.folder.path;
    this.list.setAttribute("aria-label", this.folder.name);
    this.up.disabled = this.trail.length < 2;
  }

  /** The rows matching the filter, choosing `choose` if listed, else the first. */
  private update(choose: string | null = null): void {
    this.rows = chooserRows(this.folder, this.input.value);
    const shown = this.rows.slice(0, MAX_ROWS);
    const badges = this.host.badges();
    if (this.rows.length === 0) {
      const empty = this.input.value.trim() === "" ? "No Markdown files here" : "No matches";
      this.list.replaceChildren(h("li", { class: "cc-empty" }, empty));
    } else {
      // Through a fragment: spreading a huge folder's rows into a call overflows the stack.
      const rows = document.createDocumentFragment();
      shown.forEach((node, i) => {
        rows.append(this.row(node, i, badges));
      });
      if (this.rows.length > MAX_ROWS) {
        const more = (this.rows.length - MAX_ROWS).toLocaleString("en-US");
        rows.append(
          h(
            "li",
            { class: "cc-more", role: "option", "aria-disabled": "true" },
            `+ ${more} more, type to filter`,
          ),
        );
      }
      this.list.replaceChildren(rows);
    }
    // A row past those shown can't be chosen: the first is, instead.
    const index = choose === null ? -1 : shown.findIndex((n) => samePath(n.path, choose));
    this.select(Math.max(index, 0));
  }

  private row(node: TreeNode, index: number, badges: boolean): HTMLElement {
    const current = !node.isDir && this.current !== null && samePath(node.path, this.current);
    const li = h(
      "li",
      {
        class: `cc-item ${node.isDir ? "dir" : "file"}${current ? " current" : ""}`,
        role: "option",
        id: `lx-cc-${String(index)}`,
        "data-index": String(index),
        "aria-selected": "false",
        title: node.path,
      },
      h("span", { class: node.isDir ? "cc-chevron" : "cc-spacer", "aria-hidden": "true" }),
      h("span", { class: "cc-name" }, node.name),
    );
    if (current) {
      li.setAttribute("aria-current", "true");
    }
    if (badges && node.isDir && node.status !== null) {
      li.append(badge(node.status));
    }
    return li;
  }

  private rowAt(target: EventTarget | null): number | null {
    const item = target instanceof Element ? target.closest<HTMLElement>(".cc-item") : null;
    return item?.dataset.index === undefined ? null : Number(item.dataset.index);
  }

  /** Chooses row `index`, wrapping round, and keeps it in view. */
  private select(index: number): void {
    const items = this.list.querySelectorAll<HTMLElement>(".cc-item");
    if (items.length === 0) {
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    this.selected = (index + items.length) % items.length;
    items.forEach((item, i) => {
      item.setAttribute("aria-selected", String(i === this.selected));
    });
    const chosen = items[this.selected];
    if (chosen) {
      this.input.setAttribute("aria-activedescendant", chosen.id);
      // The list is the items' offset parent.
      const list = this.list;
      if (chosen.offsetTop < list.scrollTop) {
        list.scrollTop = chosen.offsetTop;
      } else if (chosen.offsetTop + chosen.offsetHeight > list.scrollTop + list.clientHeight) {
        list.scrollTop = chosen.offsetTop + chosen.offsetHeight - list.clientHeight;
      }
    }
  }

  /** A file opens, and the chooser closes; a folder is gone into. */
  private pick(index: number): void {
    const node = this.rows[index];
    if (!node) {
      return;
    }
    if (node.isDir) {
      this.trail.push(node);
      this.show(null);
      return;
    }
    this.close();
    this.host.open(node.path);
  }

  /** Lists the folder above, choosing the one just left; false at the root. */
  private goUp(): boolean {
    if (this.trail.length < 2) {
      return false;
    }
    const left = this.trail.pop() as TreeNode;
    this.show(left.path);
    return true;
  }

  private onKey(e: KeyboardEvent): void {
    if (e.ctrlKey || e.altKey || e.metaKey) {
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
        this.pick(this.selected);
        break;
      case "ArrowRight":
        if (!this.rows[this.selected]?.isDir) return;
        this.pick(this.selected);
        break;
      case "ArrowLeft": {
        // Otherwise ← is the filter's, moving the caret.
        const atStart = this.input.selectionStart === 0 && this.input.selectionEnd === 0;
        if (!(this.input.value === "" || atStart) || !this.goUp()) return;
        break;
      }
      case "Backspace":
        if (this.input.value !== "" || !this.goUp()) return;
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
