// A popup menu: the header's ⋯ menu and the sidebar's context menu. Loaded on first use. Arrow
// keys move between items; a click outside, Tab or Esc (through the app's keymap) closes it.
import { h } from "./dom";

export interface MenuItem {
  label: string;
  /** The shortcut, shown after the label. */
  keys?: string;
  disabled?: boolean;
  run(): void;
}

export type MenuEntry = MenuItem | "separator";

export class Menu {
  private readonly el: HTMLElement;
  private readonly returnFocus: Element | null;
  /** The button that opened it, which toggles it itself. */
  private readonly anchor: HTMLElement | null;
  private closed = false;

  /**
   * Opens a menu in `root`: below and right-aligned to `at` when it is an element, else at the
   * point, kept inside the window either way.
   */
  constructor(
    root: HTMLElement,
    entries: MenuEntry[],
    at: HTMLElement | { x: number; y: number },
    private readonly onClose: () => void = () => undefined,
  ) {
    this.returnFocus = document.activeElement;
    this.anchor = at instanceof HTMLElement ? at : null;
    this.el = h("div", { class: "menu", role: "menu" });
    for (const entry of entries) {
      if (entry === "separator") {
        this.el.append(h("div", { class: "menu-sep", role: "separator" }));
        continue;
      }
      const item = h(
        "button",
        { type: "button", class: "menu-item", role: "menuitem" },
        h("span", { class: "menu-label" }, entry.label),
      );
      if (entry.keys !== undefined) {
        item.append(h("kbd", {}, entry.keys));
      }
      if (entry.disabled) {
        item.disabled = true;
      }
      item.addEventListener("click", () => {
        this.close(false);
        entry.run();
      });
      this.el.append(item);
    }
    this.el.addEventListener("keydown", (e) => {
      this.onKey(e);
    });
    root.append(this.el);
    this.place(at);
    document.addEventListener("pointerdown", this.onOutside, true);
    this.items()[0]?.focus();
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

  private items(): HTMLButtonElement[] {
    return [...this.el.querySelectorAll<HTMLButtonElement>(".menu-item:not(:disabled)")];
  }

  private place(at: HTMLElement | { x: number; y: number }): void {
    const width = this.el.offsetWidth;
    const height = this.el.offsetHeight;
    let x: number;
    let y: number;
    if (at instanceof HTMLElement) {
      const r = at.getBoundingClientRect();
      x = r.right - width;
      y = r.bottom + 4;
    } else {
      ({ x, y } = at);
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
    items[next]?.focus();
  }
}
