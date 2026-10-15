// A popup menu: the header's ⋯ and recent files menus and the sidebar's context menu. Loaded on
// first use. Arrow keys move between items, and an item's digit runs it; a click outside, Tab or
// Esc (through the app's keymap) closes it. An item that can be forgotten (a recent file) has a ×,
// and Delete does the same on it: the menu stays open on the entries left, the focus on the next
// such item, else the one before, else the note saying none is left. A menu is never taller than
// the room below the button it hangs from (or above it, where there's more), less a margin: its
// entries scroll, the keys bring the focused one into view, and its foot (`footer`) stays put.
import { h, quietly } from "./dom";

/** The gap between a menu and the button it hangs from, in pixels. */
const GAP_PX = 4;
/** The room a menu leaves at the window's top and bottom edges, in pixels. */
const EDGE_PX = 8;

export interface MenuItem {
  label: string;
  /** A second line, under the label: a recent file's folder. */
  detail?: string;
  /** The tooltip: a recent file's path, and how to forget it. */
  title?: string;
  /** The shortcut, shown after the label. */
  keys?: string;
  /** The digit that runs it while the menu has the focus, shown before the label; "" for none. */
  digit?: string;
  /** Kept at the menu's foot, below a rule, while the entries above it scroll. */
  footer?: boolean;
  disabled?: boolean;
  run(): void;
  /**
   * A × at the row's end, shown on hover or focus, and Delete on the item: `run` forgets it and
   * answers the entries left, which the menu, still open, shows.
   */
  remove?: { label: string; title: string; run(): Promise<MenuEntry[]> };
}

/**
 * A line that says something rather than does it, such as that no file is left: it can hold the
 * focus, and never runs.
 */
export interface MenuNote {
  note: string;
}

export type MenuEntry = MenuItem | MenuNote | "separator";

export class Menu {
  private readonly el: HTMLElement;
  /** The entries but the foot's, which scroll when the menu is too tall for the window. */
  private readonly list: HTMLElement;
  /** The foot: the entries kept below the list. */
  private readonly foot: HTMLElement;
  private readonly returnFocus: Element | null;
  /** The button that opened it, which toggles it itself. */
  readonly anchor: HTMLElement | null;
  /** What runs each item that has a digit, by its digit. */
  private digits = new Map<string, () => void>();
  /** What forgets each item that can be forgotten, by the item. */
  private removers = new Map<Element, () => void>();
  private closed = false;

  /**
   * Opens a menu in `root`: below and right-aligned to `at` when it is an element, else at the
   * point, kept inside the window either way.
   */
  constructor(
    root: HTMLElement,
    entries: MenuEntry[],
    private readonly at: HTMLElement | { x: number; y: number },
    private readonly onClose: () => void = () => undefined,
  ) {
    this.returnFocus = document.activeElement;
    this.anchor = at instanceof HTMLElement ? at : null;
    this.list = h("div", { class: "menu-list", role: "none" });
    this.foot = h("div", { class: "menu-foot", role: "none" });
    this.el = h("div", { class: "menu", role: "menu" }, this.list, this.foot);
    this.render(entries);
    this.el.addEventListener("keydown", (e) => {
      this.onKey(e);
    });
    root.append(this.el);
    this.place(at);
    document.addEventListener("pointerdown", this.onOutside, true);
    this.focusItem(this.items()[0]);
  }

  private render(entries: MenuEntry[]): void {
    this.digits = new Map();
    this.removers = new Map();
    const footer = (entry: MenuEntry) =>
      typeof entry === "object" && "run" in entry && entry.footer;
    const foot = entries.filter(footer);
    this.list.replaceChildren(
      ...entries.filter((entry) => !footer(entry)).map((entry) => this.entryEl(entry)),
    );
    this.foot.replaceChildren(
      ...(foot.length > 0 ? [this.entryEl("separator"), ...foot.map((e) => this.entryEl(e))] : []),
    );
    this.foot.hidden = foot.length === 0;
  }

  /** Focuses `item`, scrolling the list just enough to show it. */
  private focusItem(item: HTMLElement | undefined): void {
    if (!item) {
      return;
    }
    item.focus({ preventScroll: true });
    if (!this.list.contains(item)) {
      return;
    }
    const box = (item.closest(".menu-row") ?? item).getBoundingClientRect();
    const view = this.list.getBoundingClientRect();
    // Whole pixels, rounded up: a row's edge may fall between two.
    if (box.top < view.top) {
      this.list.scrollTop -= Math.ceil(view.top - box.top);
    } else if (box.bottom > view.bottom) {
      this.list.scrollTop += Math.ceil(box.bottom - view.bottom);
    }
  }

  /** The items that can be forgotten, in order. */
  private rows(): HTMLElement[] {
    return [...this.el.querySelectorAll<HTMLElement>(".menu-row > .menu-item")];
  }

  /**
   * Once row `row` is forgotten: the entries left, the focus on the row now in its place, else the
   * one before, else the note.
   */
  private showLeft(entries: MenuEntry[], row: number): void {
    this.render(entries);
    this.place(this.at);
    const rows = this.rows();
    this.focusItem(
      rows[row] ??
        rows[row - 1] ??
        this.el.querySelector<HTMLElement>(".menu-note") ??
        this.items()[0],
    );
  }

  private entryEl(entry: MenuEntry): HTMLElement {
    if (entry === "separator") {
      return h("div", { class: "menu-sep", role: "separator" });
    }
    if ("note" in entry) {
      return h(
        "div",
        {
          class: "menu-item menu-note",
          role: "menuitem",
          "aria-disabled": "true",
          tabindex: "-1",
        },
        h("span", { class: "menu-label" }, entry.note),
      );
    }
    const label = h("span", { class: "menu-label" }, entry.label);
    const item = h(
      "button",
      { type: "button", class: "menu-item", role: "menuitem" },
      entry.detail === undefined
        ? label
        : h(
            "span",
            { class: "menu-text" },
            label,
            h("span", { class: "menu-detail" }, entry.detail),
          ),
    );
    if (entry.digit !== undefined) {
      item.prepend(h("span", { class: "menu-digit", "aria-hidden": "true" }, entry.digit));
    }
    if (entry.title !== undefined) {
      item.title = entry.title;
    }
    if (entry.keys !== undefined) {
      item.append(h("kbd", {}, entry.keys));
    }
    const activate = (): void => {
      this.close(false);
      entry.run();
    };
    const shortcuts: string[] = [];
    if (entry.disabled) {
      item.disabled = true;
    } else if (entry.digit) {
      this.digits.set(entry.digit, activate);
      shortcuts.push(entry.digit);
    }
    item.addEventListener("click", activate);
    const remove = entry.remove;
    if (remove === undefined) {
      if (shortcuts.length > 0) item.setAttribute("aria-keyshortcuts", shortcuts.join(" "));
      return item;
    }
    shortcuts.push("Delete");
    item.setAttribute("aria-keyshortcuts", shortcuts.join(" "));
    const forget = (): void => {
      const row = this.rows().indexOf(item);
      quietly(
        remove.run().then((entries) => {
          if (!this.closed) this.showLeft(entries, row);
        }),
      );
    };
    this.removers.set(item, forget);
    const button = h(
      "button",
      { type: "button", class: "menu-remove", "aria-label": remove.label, title: remove.title },
      "×",
    );
    button.addEventListener("click", forget);
    return h("div", { class: "menu-row", role: "none" }, item, button);
  }

  get isOpen(): boolean {
    return !this.closed;
  }

  /** Closes the menu; focus goes back where it was unless an item is about to move it. */
  close(restoreFocus = true): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    document.removeEventListener("pointerdown", this.onOutside, true);
    const hadFocus = this.el.contains(document.activeElement);
    this.el.remove();
    if (restoreFocus && hadFocus && this.returnFocus instanceof HTMLElement) {
      this.returnFocus.focus({ preventScroll: true });
    }
    this.onClose();
  }

  /** The items that can take the focus: buttons not disabled, and notes. */
  private items(): HTMLElement[] {
    return [...this.el.querySelectorAll<HTMLElement>(".menu-item:not(:disabled)")];
  }

  /**
   * Below `at`, a button, and right-aligned to it, no taller than the room below it less a margin;
   * above it instead when the menu doesn't fit below and there is more room above. At a point, no
   * taller than the window less the margins. Kept inside the window either way.
   */
  private place(at: HTMLElement | { x: number; y: number }): void {
    this.el.style.maxHeight = "";
    const natural = this.el.offsetHeight;
    let room = window.innerHeight - 2 * EDGE_PX;
    let x = 0;
    let y: number;
    // A button's right edge, which the menu's lines up with.
    let right: number | null = null;
    if (at instanceof HTMLElement) {
      const r = at.getBoundingClientRect();
      const below = window.innerHeight - r.bottom - GAP_PX - EDGE_PX;
      const above = r.top - GAP_PX - EDGE_PX;
      right = r.right;
      if (natural <= below || below >= above) {
        room = below;
        y = r.bottom + GAP_PX;
      } else {
        room = above;
        y = r.top - GAP_PX - Math.min(natural, above);
      }
    } else {
      ({ x, y } = at);
    }
    this.el.style.maxHeight = `${String(Math.max(0, room))}px`;
    // Measured once capped: a list that scrolls may change it.
    const width = this.el.offsetWidth;
    const height = this.el.offsetHeight;
    if (right !== null) {
      x = right - width;
    }
    x = Math.max(4, Math.min(x, window.innerWidth - width - 4));
    y = Math.max(4, Math.min(y, window.innerHeight - height - 4));
    this.el.style.left = `${String(x)}px`;
    this.el.style.top = `${String(y)}px`;
  }

  private readonly onOutside = (e: Event): void => {
    const target = e.target instanceof Node ? e.target : null;
    if (!target || (!this.el.contains(target) && !this.anchor?.contains(target))) {
      this.close(false);
    }
  };

  private onKey(e: KeyboardEvent): void {
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    const digit = plain ? this.digits.get(e.key) : undefined;
    if (digit) {
      e.preventDefault();
      digit();
      return;
    }
    const forget =
      plain && !e.shiftKey && e.key === "Delete" && document.activeElement
        ? this.removers.get(document.activeElement)
        : undefined;
    if (forget) {
      e.preventDefault();
      forget();
      return;
    }
    const items = this.items();
    const at = items.findIndex((item) => item === document.activeElement);
    let next: number;
    switch (e.key) {
      case "ArrowDown":
        next = (at + 1) % items.length;
        break;
      case "ArrowUp":
        next = (at - 1 + items.length) % items.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = items.length - 1;
        break;
      case "Tab":
        this.close();
        return;
      default:
        return;
    }
    e.preventDefault();
    this.focusItem(items[next]);
  }
}
