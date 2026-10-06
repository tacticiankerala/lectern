// The right panel (`#lx-outline`): "Outline | Comments (n)" tabs over two panes, each its own
// scroll container. The outline fills the first (outline.ts); the review comments, once their
// module loads, the second (comments.ts). While comments are off or hidden the tabs go and the
// panel shows the outline alone, as before.
import { h } from "./dom";

export type Tab = "outline" | "comments";

/** How long the Comments tab's pulse lasts (comments.css), with some to spare. */
const FLASH_MS = 1600;

export class RightPanel {
  readonly outlinePane: HTMLElement;
  readonly commentsPane: HTMLElement;
  private readonly strip: HTMLElement;
  private readonly tabs: Record<Tab, HTMLButtonElement>;
  private current: Tab = "outline";
  private enabled = true;
  private flashTimer?: ReturnType<typeof setTimeout>;

  constructor(aside: HTMLElement) {
    const tab = (id: Tab, label: string): HTMLButtonElement =>
      h(
        "button",
        {
          type: "button",
          role: "tab",
          id: `lx-tab-${id}`,
          class: "panel-tab",
          "aria-controls": `lx-${id}-pane`,
        },
        label,
      );
    this.tabs = { outline: tab("outline", "Outline"), comments: tab("comments", "Comments") };
    this.strip = h(
      "div",
      { class: "panel-tabs", role: "tablist", "aria-label": "Right panel" },
      this.tabs.outline,
      this.tabs.comments,
    );
    const pane = (id: Tab): HTMLElement =>
      h("div", {
        id: `lx-${id}-pane`,
        class: "panel-pane",
        role: "tabpanel",
        "aria-labelledby": `lx-tab-${id}`,
      });
    this.outlinePane = pane("outline");
    this.commentsPane = pane("comments");
    aside.replaceChildren(this.strip, this.outlinePane, this.commentsPane);

    for (const id of ["outline", "comments"] as const) {
      this.tabs[id].addEventListener("click", () => {
        this.show(id);
      });
    }
    this.strip.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") {
        return;
      }
      e.preventDefault();
      const next = this.current === "outline" ? "comments" : "outline";
      this.show(next);
      this.tabs[next].focus();
    });
    this.sync();
  }

  get tab(): Tab {
    return this.current;
  }

  /** Shows a tab's pane. While comments are off only the outline shows. */
  show(tab: Tab): void {
    this.current = this.enabled ? tab : "outline";
    this.sync();
  }

  /** Off, the tabs hide and the outline shows; on, the tabs come back on the outline. */
  setCommentsEnabled(on: boolean): void {
    if (on === this.enabled) {
      return;
    }
    this.enabled = on;
    this.current = "outline";
    this.sync();
  }

  /** The open comments, in the Comments tab's label. */
  setCount(n: number): void {
    this.tabs.comments.textContent = n > 0 ? `Comments (${String(n)})` : "Comments";
  }

  /** A brief pulse on the Comments tab: Claude has written something. */
  flash(): void {
    const tab = this.tabs.comments;
    clearTimeout(this.flashTimer);
    tab.classList.remove("flash");
    // A layout in between restarts the animation when it was still running.
    tab.getBoundingClientRect();
    tab.classList.add("flash");
    this.flashTimer = setTimeout(() => {
      tab.classList.remove("flash");
    }, FLASH_MS);
  }

  private sync(): void {
    this.strip.hidden = !this.enabled;
    for (const id of ["outline", "comments"] as const) {
      const selected = id === this.current;
      this.tabs[id].setAttribute("aria-selected", String(selected));
      this.tabs[id].tabIndex = selected ? 0 : -1;
    }
    this.outlinePane.hidden = this.current !== "outline";
    this.commentsPane.hidden = this.current !== "comments";
  }
}
