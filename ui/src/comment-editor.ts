// The comment editor: a small popover with a text box, Save and Cancel, for writing a new comment.
// It opens by what the comment is on, inside the document pane (never inside the note), so it
// scrolls with the text.
//
// Contracts:
// - Ctrl+Enter or Save hands the trimmed text to the caller, which answers whether it was saved.
//   Saved, the popover closes; not (the caller said why), it stays open with the text for another
//   try. Empty text can't be saved. The box is disabled while its text is being saved.
// - Esc or Cancel closes it and keeps nothing; its Esc stops there, before the app's.
// - One at a time: opening it again replaces what was open.
// - While open, Tab keeps focus inside it. A press outside closes it, as the reading panel does,
//   but only while it holds no text: typed text goes only by Esc or Cancel. Closed with focus
//   inside, focus goes back to where it was.
// - It's kept in view: placed by what it's on, then inside the visible part of the pane; brought
//   back into view when focused again, and when the window resizes while it's open.
import { h } from "./dom";

/** What a comment is on, in the viewport. */
export interface EditorAnchor {
  left: number;
  top: number;
  bottom: number;
}

export interface EditorRequest {
  at: EditorAnchor;
  /** The text it opens with; none when omitted. */
  text?: string;
  /** Saves the text; true when it was saved. */
  save: (text: string) => Promise<boolean>;
}

/** Space between the editor and what it's on, and kept clear of the pane's edges, in pixels. */
const GAP = 6;
const EDGE = 8;

export class CommentEditor {
  private readonly el: HTMLElement;
  private readonly area: HTMLTextAreaElement;
  private readonly saveButton: HTMLButtonElement;
  private readonly cancelButton: HTMLButtonElement;
  private request: EditorRequest | null = null;
  private busy = false;
  private returnFocus: Element | null = null;

  constructor(private readonly pane: HTMLElement) {
    this.area = h("textarea", { class: "comment-editor-text", rows: "4", placeholder: "Comment…" });
    this.saveButton = h("button", { type: "button", class: "btn primary" }, "Save");
    this.cancelButton = h("button", { type: "button", class: "btn" }, "Cancel");
    this.el = h(
      "div",
      { class: "comment-editor", role: "dialog", "aria-label": "New comment" },
      this.area,
      h("div", { class: "comment-editor-actions" }, this.saveButton, this.cancelButton),
    );
    this.el.hidden = true;
    this.area.addEventListener("input", () => {
      this.sync();
    });
    this.saveButton.addEventListener("click", () => void this.submit());
    this.cancelButton.addEventListener("click", () => {
      this.close();
    });
    this.el.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.ctrlKey) {
        e.preventDefault();
        void this.submit();
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      } else if (e.key === "Tab") {
        this.trapTab(e);
      }
    });
    pane.append(this.el);
  }

  get isOpen(): boolean {
    return !this.el.hidden;
  }

  /** Whether it's open with text typed in it. */
  get hasText(): boolean {
    return this.isOpen && this.area.value.trim() !== "";
  }

  /** The text in it, as typed. */
  get text(): string {
    return this.area.value;
  }

  /** Whether it's open with its text being saved. */
  get saving(): boolean {
    return this.isOpen && this.busy;
  }

  /** Brings it back into view and puts the focus in its text box. */
  focus(): void {
    if (this.isOpen) {
      this.keepInView();
      this.area.focus({ preventScroll: true });
    }
  }

  /** Closes it unless it holds text. */
  closeIfEmpty(): void {
    if (!this.hasText) {
      this.close();
    }
  }

  /** Opens the editor by `request.at`, with `request.text` in it, replacing any open one. */
  open(request: EditorRequest): void {
    if (this.isOpen) {
      this.close();
    }
    this.request = request;
    this.busy = false;
    this.area.value = request.text ?? "";
    this.returnFocus = document.activeElement;
    this.el.hidden = false;
    this.sync();
    this.place(request.at);
    document.addEventListener("pointerdown", this.onOutside);
    window.addEventListener("resize", this.onResize);
    this.area.focus({ preventScroll: true });
  }

  /** Closes the editor, keeping nothing; focus inside it goes back to where it was. */
  close(): void {
    if (!this.isOpen) {
      return;
    }
    const hadFocus = this.el.contains(document.activeElement);
    this.el.hidden = true;
    this.request = null;
    this.busy = false;
    this.area.value = "";
    document.removeEventListener("pointerdown", this.onOutside);
    window.removeEventListener("resize", this.onResize);
    const back = this.returnFocus;
    this.returnFocus = null;
    if (hadFocus && back instanceof HTMLElement && back.isConnected) {
      back.focus({ preventScroll: true });
    }
  }

  /** Closes it and takes it out of the pane. */
  dispose(): void {
    this.close();
    this.el.remove();
  }

  private sync(): void {
    this.area.disabled = this.busy;
    this.saveButton.disabled = this.busy || this.area.value.trim() === "";
  }

  private async submit(): Promise<void> {
    const request = this.request;
    const text = this.area.value.trim();
    if (!request || this.busy || text === "") {
      return;
    }
    this.busy = true;
    this.sync();
    const saved = await request.save(text);
    if (this.request !== request) {
      // Closed or replaced meanwhile.
      return;
    }
    if (saved) {
      this.close();
      return;
    }
    this.busy = false;
    this.sync();
    this.area.focus({ preventScroll: true });
  }

  /** Tab and Shift+Tab go round the editor's controls. */
  private trapTab(e: KeyboardEvent): void {
    const stops = [this.area, this.saveButton, this.cancelButton].filter((el) => !el.disabled);
    const at = stops.indexOf(document.activeElement as HTMLTextAreaElement | HTMLButtonElement);
    const next = stops[(at + (e.shiftKey ? stops.length - 1 : 1)) % stops.length];
    e.preventDefault();
    next?.focus();
  }

  /**
   * Below what it's on when it fits in the pane's view, else above it, else over its top; at its
   * left. Then, whatever it's on, kept inside the pane and the window.
   */
  private place(at: EditorAnchor): void {
    const pane = this.pane.getBoundingClientRect();
    const width = this.el.offsetWidth;
    const height = this.el.offsetHeight;
    const left = Math.max(
      EDGE,
      Math.min(at.left - pane.left, this.pane.clientWidth - width - EDGE),
    );
    const { top: viewTop, bottom: viewBottom } = this.view();
    let top = at.bottom + GAP;
    if (top + height > viewBottom) {
      const above = at.top - GAP - height;
      top = above >= viewTop ? above : at.top + GAP;
    }
    // The top wins when it doesn't fit at all.
    top = Math.max(viewTop, Math.min(top, viewBottom - height));
    this.el.style.left = `${String(Math.round(left + this.pane.scrollLeft))}px`;
    this.el.style.top = `${String(Math.round(top - pane.top + this.pane.scrollTop))}px`;
  }

  /** The part of the pane in the window, `EDGE` in from its top and bottom. */
  private view(): { top: number; bottom: number } {
    const pane = this.pane.getBoundingClientRect();
    return {
      top: Math.max(pane.top, 0) + EDGE,
      bottom: Math.min(pane.bottom, window.innerHeight) - EDGE,
    };
  }

  /**
   * Scrolls the pane, at once and as little as it takes, until the editor is in its view; its top
   * when it's taller than the view.
   */
  private keepInView(): void {
    const view = this.view();
    const box = this.el.getBoundingClientRect();
    if (box.top < view.top) {
      this.pane.scrollTop -= view.top - box.top;
    } else if (box.bottom > view.bottom) {
      this.pane.scrollTop += Math.min(box.bottom - view.bottom, box.top - view.top);
    }
  }

  private readonly onResize = (): void => {
    this.keepInView();
  };

  private readonly onOutside = (e: Event): void => {
    if (!(e.target instanceof Node) || !this.el.contains(e.target)) {
      this.closeIfEmpty();
    }
  };
}
