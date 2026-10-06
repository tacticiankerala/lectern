// Adding review comments (spec §6): the "Comment" button by a selection in the note, the "+" by
// the block under the pointer, what Ctrl+Alt+M comments on, the editor they open
// (comment-editor.ts), and re-attach mode. Part of the comments module (comments.ts), whose lazy
// chunk it's bundled into.
//
// Contracts:
// - The buttons sit in the document pane, never in the note's markup. They show only while
//   comments do, outside focus mode, and for a review that isn't read-only; the "+" not while the
//   editor is open, the selection button not either unless it re-attaches.
// - A comment on a selection is anchored by the lines of the blocks its text starts and ends in,
//   and that text as core sees it (comments-text.ts); on a block, by the block's lines and text.
// - Ctrl+Alt+M with nothing selected comments on the deepest block crossing the top of the view,
//   else the first below it. A selection scrolled out of view comes back into view first, so the
//   editor opens by it.
// - Typed text is never lost but on purpose: while the editor holds text, asking for another
//   comment brings it back into focus instead, and re-attaching leaves it open.
// - Re-attach mode turns the selection button into "Attach C<n> here" until it's used, cancelled
//   or Esc is pressed, or the comment, the note or the comments' visibility goes.
// - Every operation is for the note it was asked on (see `AddingHost.perform`).
import { CommentEditor, type EditorAnchor } from "./comment-editor";
import { SKIP, blockLines, buildTextIndex, leafBlockOf, quoteFromRange } from "./comments-text";
import { h } from "./dom";
import type { NewAnchor } from "./generated/NewAnchor";
import type { ReviewOp } from "./generated/ReviewOp";

/** Why an operation wasn't applied, or that it was. */
export type Outcome = "applied" | "failed" | "skipped";

/** What adding needs from the comments controller. */
export interface AddingHost {
  /** The document on screen, or null when there is none. */
  doc: () => HTMLElement | null;
  /** The path of the note on screen, or null. */
  docPath: () => string | null;
  /** The element the document scrolls in. */
  scroller: HTMLElement;
  /** Whether focus mode (F11) is on. */
  focusMode: () => boolean;
  toast: (m: string) => void;
  /** Whether comments show. */
  shown: () => boolean;
  /** Shows comments the header toggle hid. */
  show: () => void;
  /** Why the review on screen is read-only, or null when it isn't. */
  readOnly: () => string | null;
  /** Applies an operation to the note at `path`, after those before it (comments.ts). */
  perform: (op: ReviewOp, path: string) => Promise<Outcome>;
  /** Whether the note at `path` is still the one on screen. */
  showing: (path: string) => boolean;
  /** The ids of the comments in the review on screen. */
  ids: () => number[];
  /** Selects a comment's card; `reveal` also shows the Comments tab and the panel. */
  select: (id: number, reveal: boolean) => void;
}

/** The blocks a "+" comments on, and Ctrl+Alt+M at the top of the view. */
const LEAF = "p, li, h1, h2, h3, h4, h5, h6, .code-block, tr, blockquote";
/** The "+" button's size, and the space between it and its block, in pixels. */
const PLUS_SIZE = 20;
const PLUS_GAP = 8;
/** The room a list item's bullet or checkbox takes left of it, in its ems (as its dot, comments.css). */
const MARKER_EMS = 1.5;
/** How far the pane may scroll before the selection button goes, in pixels. */
const SCROLL_HIDES = 200;
/** The space between the selection's end and its button, in pixels. */
const SEL_GAP = 6;
/** What the buttons keep clear of the pane's edges, in pixels. */
const EDGE = 8;
/** What a selection brought back into view keeps clear of the pane's edges, in pixels. */
const VIEW_MARGIN = 16;

/** A box in the viewport. */
export interface Box {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** The pane the buttons are placed in: its box, inner width and scroll. */
export interface PaneBox {
  left: number;
  top: number;
  clientWidth: number;
  scrollLeft: number;
  scrollTop: number;
}

/**
 * Where the selection button goes, in the pane's scrolled content: on the selection's last line,
 * `SEL_GAP` after its end, or `SEL_GAP` before it when it wouldn't fit before the pane's right
 * edge; never past the pane's left edge.
 */
export function selButtonPlace(
  end: Box,
  size: { width: number; height: number },
  pane: PaneBox,
): { left: number; top: number } {
  let left = end.right - pane.left + SEL_GAP;
  if (left + size.width > pane.clientWidth - EDGE) {
    left = end.right - pane.left - SEL_GAP - size.width;
  }
  const top = (end.top + end.bottom) / 2 - size.height / 2 - pane.top;
  return {
    left: Math.round(Math.max(EDGE, left) + pane.scrollLeft),
    top: Math.round(top + pane.scrollTop),
  };
}

export class CommentAdding {
  /** Re-attach mode's banner, for the Comments pane to hold. */
  readonly attachBanner: HTMLElement;
  private readonly attachText: HTMLElement;
  /** The comment being re-attached, and its note (`noteKey`). */
  private attach: { id: number; note: string } | null = null;
  private readonly editor: CommentEditor;
  /** The note (`noteKey`) the editor was last opened on. */
  private editorNote: string | null = null;
  private readonly selButton: HTMLButtonElement;
  private readonly plus: HTMLButtonElement;
  /** The block the "+" is by. */
  private plusBlock: HTMLElement | null = null;
  /** The pane's scroll when the selection button showed. */
  private selScrollTop = 0;
  /** The frame the selection button waits for, once the selection settles. */
  private selFrame = 0;
  /** A mouse button is held down in the pane: a selection is still being dragged. */
  private dragging = false;
  private disposed = false;
  private readonly stop: () => void;

  constructor(private readonly host: AddingHost) {
    this.attachText = h("span", { class: "comments-attach-text" });
    const cancelAttach = h("button", { type: "button", class: "btn" }, "Cancel");
    cancelAttach.addEventListener("click", () => {
      this.leaveAttach();
    });
    this.attachBanner = h(
      "div",
      { class: "comments-attach", role: "status" },
      this.attachText,
      cancelAttach,
    );
    this.attachBanner.hidden = true;

    const scroller = host.scroller;
    this.editor = new CommentEditor(scroller);
    this.selButton = h("button", { type: "button", class: "lx-sel-comment" }, "Comment");
    this.plus = h(
      "button",
      {
        type: "button",
        class: "lx-block-plus",
        "aria-label": "Comment on this block",
        title: "Comment on this block",
      },
      "+",
    );
    for (const button of [this.selButton, this.plus]) {
      button.hidden = true;
      // A press keeps the selection and the focus where they are.
      button.addEventListener("mousedown", (e) => {
        e.preventDefault();
      });
      scroller.append(button);
    }
    this.selButton.addEventListener("click", () => {
      this.addFromSelection();
    });
    this.plus.addEventListener("click", () => {
      const block = this.plusBlock;
      this.showPlus(null);
      if (block?.isConnected) this.addOnBlock(block);
    });

    scroller.addEventListener("pointerdown", this.onPointerDown);
    scroller.addEventListener("pointermove", this.onPointerMove);
    scroller.addEventListener("pointerleave", this.onPointerLeave);
    scroller.addEventListener("scroll", this.onScroll, { passive: true });
    document.addEventListener("pointerup", this.onPointerUp);
    document.addEventListener("selectionchange", this.onSelectionChange);
    document.addEventListener("keydown", this.onKey);
    this.stop = () => {
      scroller.removeEventListener("pointerdown", this.onPointerDown);
      scroller.removeEventListener("pointermove", this.onPointerMove);
      scroller.removeEventListener("pointerleave", this.onPointerLeave);
      scroller.removeEventListener("scroll", this.onScroll);
      document.removeEventListener("pointerup", this.onPointerUp);
      document.removeEventListener("selectionchange", this.onSelectionChange);
      document.removeEventListener("keydown", this.onKey);
    };
  }

  /**
   * A comment on the text selected in the note, in the editor; in re-attach mode, the comment
   * being re-attached moved to it. Hidden comments show first. False when nothing in the note is
   * selected.
   */
  addFromSelection(): boolean {
    const range = this.selectionInDoc();
    if (range === null) {
      return false;
    }
    this.hideSelButton();
    // Taken before anything changes in the note.
    const anchor = this.anchorFrom(range);
    if (anchor === null || !this.writable()) {
      return true;
    }
    this.ensureShown();
    if (this.attach !== null) {
      void this.reattach(this.attach.id, anchor);
    } else {
      this.bringIntoView(range);
      this.openEditor(anchor, endRect(range));
    }
    return true;
  }

  /**
   * A comment on the block at the top of the view (see the module comment), in the editor. Hidden
   * comments show first.
   */
  addAtTop(): void {
    this.ensureShown();
    const block = this.topBlock();
    if (block) {
      this.addOnBlock(block);
    }
  }

  /** Re-attach mode for comment `id` on the note on screen. */
  startReattach(id: number): void {
    const path = this.host.docPath();
    if (path === null || !this.writable()) {
      return;
    }
    this.editor.closeIfEmpty();
    this.attach = { id, note: noteKey(path) };
    this.syncAttach();
  }

  leaveAttach(): void {
    if (this.attach !== null) {
      this.attach = null;
      this.syncAttach();
    }
  }

  /** Comments shown or hidden: hidden, nothing offers to add one. */
  setVisible(on: boolean): void {
    if (!on) {
      this.editor.close();
      this.leaveAttach();
      this.showPlus(null);
      this.hideSelButton();
    }
  }

  /** The review on screen changed: re-attach mode ends with its comment. */
  reviewChanged(ids: Set<number>): void {
    if (this.attach !== null && !ids.has(this.attach.id)) {
      this.leaveAttach();
    }
  }

  /** The document re-rendered or changed: another note ends the editor and re-attach mode. */
  docChanged(): void {
    const path = this.host.docPath();
    const note = path === null ? null : noteKey(path);
    this.showPlus(null);
    if (this.editorNote !== note) this.editor.close();
    if (this.attach !== null && this.attach.note !== note) this.leaveAttach();
  }

  /** Takes the buttons, the editor and the listeners away. */
  dispose(): void {
    this.disposed = true;
    this.stop();
    cancelAnimationFrame(this.selFrame);
    this.editor.dispose();
    this.selButton.remove();
    this.plus.remove();
    this.attach = null;
  }

  /** Shows comments the header toggle hid. */
  private ensureShown(): void {
    if (!this.host.shown()) {
      this.host.show();
    }
  }

  /** Whether the review on screen takes changes; when it's read-only, says why. */
  private writable(): boolean {
    const reason = this.host.readOnly();
    if (reason !== null) {
      this.host.toast(reason);
    }
    return reason === null;
  }

  /** Whether the buttons may show: comments showing, outside focus mode, not read-only. */
  private offering(): boolean {
    return this.host.shown() && !this.host.focusMode() && this.host.readOnly() === null;
  }

  /** The selection, when it's made and both its ends are in the note; else null. */
  private selected(): Range | null {
    const doc = this.host.doc();
    const selection = document.getSelection();
    if (!doc || !selection || selection.rangeCount === 0 || selection.isCollapsed) {
      return null;
    }
    const range = selection.getRangeAt(0);
    return doc.contains(range.startContainer) && doc.contains(range.endContainer) ? range : null;
  }

  /** The text selected in the note, trimmed to the text it covers; null when there's none. */
  private selectionInDoc(): Range | null {
    const doc = this.host.doc();
    const range = this.selected();
    return doc && range ? textOnly(range, doc) : null;
  }

  /**
   * What a comment on `range` is anchored by: the lines from the start of the block its text
   * starts in to the end of the block it ends in, and its text. Null without either.
   */
  private anchorFrom(range: Range): NewAnchor | null {
    const doc = this.host.doc();
    const startEl = doc && leafBlockOf(range.startContainer, doc);
    const endEl = doc && leafBlockOf(range.endContainer, doc);
    const start = startEl ? blockLines(startEl) : null;
    const end = endEl ? blockLines(endEl) : null;
    const quote = quoteFromRange(range);
    if (!start || !end || quote === "") {
      return null;
    }
    return { startLine: start[0], endLine: Math.max(start[0], end[1]), quote };
  }

  /** A comment on a whole block: its lines and its text. */
  private addOnBlock(block: HTMLElement): void {
    const lines = blockLines(block);
    const quote = buildTextIndex([block]).text;
    if (!lines || quote === "" || !this.writable()) {
      return;
    }
    this.ensureShown();
    this.openEditor(
      { startLine: lines[0], endLine: lines[1], quote },
      block.getBoundingClientRect(),
    );
  }

  /**
   * The deepest block (`LEAF`) crossing the top of the pane's view; when none does (the top falls
   * between blocks), the first below it, or the deepest starting where it does (a loose list
   * item's first paragraph). Null when there's none.
   */
  private topBlock(): HTMLElement | null {
    const doc = this.host.doc();
    if (!doc) {
      return null;
    }
    const top = this.host.scroller.getBoundingClientRect().top;
    // Down the note's own blocks first, so those above the view are measured once each.
    for (const child of doc.children) {
      const box = child.getBoundingClientRect();
      if (box.top < top && box.bottom <= top) {
        continue;
      }
      let crossing: HTMLElement | null = null;
      for (const el of [child, ...child.querySelectorAll(LEAF)]) {
        if (!(el instanceof HTMLElement) || !el.matches(LEAF) || blockLines(el) === null) {
          continue;
        }
        const box = el.getBoundingClientRect();
        if (box.top <= top && top < box.bottom) {
          // Blocks crossing one line nest, so the last one met is the deepest.
          crossing = el;
        } else if (box.top >= top) {
          return crossing ?? deepestAt(el, box.top);
        }
      }
      if (crossing) {
        return crossing;
      }
    }
    return null;
  }

  /** Scrolls the pane, at once and as little as it takes, until `range`'s end is in view. */
  private bringIntoView(range: Range): void {
    const pane = this.host.scroller;
    const view = pane.getBoundingClientRect();
    const end = endRect(range);
    if (end.top < view.top) {
      pane.scrollTop -= view.top - end.top + VIEW_MARGIN;
    } else if (end.bottom > view.bottom) {
      pane.scrollTop += end.bottom - view.bottom + VIEW_MARGIN;
    }
  }

  /**
   * The editor on what `anchor` says, by `at`, saving a new comment on the note on screen. An
   * editor holding text comes back into focus instead, so the text isn't lost.
   */
  private openEditor(anchor: NewAnchor, at: EditorAnchor): void {
    const path = this.host.docPath();
    if (path === null) {
      return;
    }
    this.showPlus(null);
    this.hideSelButton();
    if (this.editor.hasText) {
      this.editor.focus();
      return;
    }
    this.editorNote = noteKey(path);
    this.editor.open({ at, save: (text) => this.saveNew(path, anchor, text) });
  }

  /**
   * Adds a comment to the note at `path`: saved, the Comments tab shows it (the highest id in the
   * new review) selected. False when it wasn't saved, said in a toast.
   */
  private async saveNew(path: string, anchor: NewAnchor, text: string): Promise<boolean> {
    const outcome = await this.host.perform({ op: "add", anchor, text }, path);
    if (outcome === "skipped" && !this.disposed) {
      this.host.toast("Your comment wasn't saved: the note changed.");
    }
    if (outcome !== "applied") {
      return false;
    }
    const ids = this.host.ids();
    if (ids.length > 0 && this.host.showing(path)) {
      this.host.select(Math.max(...ids), true);
    }
    return true;
  }

  /** Moves comment `id` to `anchor`; done, re-attach mode ends and its card is selected. */
  private async reattach(id: number, anchor: NewAnchor): Promise<void> {
    const path = this.host.docPath();
    if (path === null) {
      return;
    }
    const outcome = await this.host.perform({ op: "reattach", id, anchor }, path);
    if (outcome === "applied" && this.host.showing(path)) {
      if (this.attach?.id === id) this.leaveAttach();
      this.host.select(id, false);
    }
  }

  /** Re-attach mode's banner, and the selection button's label. */
  private syncAttach(): void {
    const id = this.attach === null ? "" : `C${String(this.attach.id)}`;
    this.attachBanner.hidden = this.attach === null;
    this.attachText.textContent =
      this.attach === null ? "" : `Select the new text for ${id}, then press Attach here.`;
    this.syncSelButton();
  }

  /**
   * The selection button: on the last line of the text selected in the note, once the selection
   * is made, while comments may be added; "Attach C<n> here" in re-attach mode.
   */
  private syncSelButton(): void {
    const allowed =
      this.offering() && !this.dragging && (this.attach !== null || !this.editor.isOpen);
    const range = allowed ? this.selectionInDoc() : null;
    if (range === null) {
      this.hideSelButton();
      return;
    }
    const pane = this.host.scroller;
    const box = pane.getBoundingClientRect();
    this.selButton.textContent =
      this.attach === null ? "Comment" : `Attach C${String(this.attach.id)} here`;
    this.selButton.hidden = false;
    const at = selButtonPlace(
      endRect(range),
      { width: this.selButton.offsetWidth, height: this.selButton.offsetHeight },
      {
        left: box.left,
        top: box.top,
        clientWidth: pane.clientWidth,
        scrollLeft: pane.scrollLeft,
        scrollTop: pane.scrollTop,
      },
    );
    this.selButton.style.left = `${String(at.left)}px`;
    this.selButton.style.top = `${String(at.top)}px`;
    this.selScrollTop = pane.scrollTop;
  }

  private hideSelButton(): void {
    this.selButton.hidden = true;
  }

  /**
   * The "+" by `block`: just left of its own left edge (and of a list item's bullet, and of its
   * dot), level with its first line, inside the pane. Null hides it.
   */
  private showPlus(block: HTMLElement | null): void {
    this.plusBlock = block;
    const doc = this.host.doc();
    if (!block || !doc) {
      this.plus.hidden = true;
      return;
    }
    const pane = this.host.scroller;
    const box = pane.getBoundingClientRect();
    const rect = block.getBoundingClientRect();
    let edge = rect.left;
    // A list item's bullet or checkbox sits left of its first line.
    const item = block.closest<HTMLElement>("li");
    if (item && doc.contains(item)) {
      const itemBox = item.getBoundingClientRect();
      if (Math.abs(itemBox.top - rect.top) < 1 && itemBox.left >= rect.left - 1) {
        edge = itemBox.left - MARKER_EMS * (parseFloat(getComputedStyle(item).fontSize) || 16);
      }
    }
    let left = edge - PLUS_SIZE - PLUS_GAP;
    const dot = block.querySelector(":scope > .lx-cdot");
    if (dot) {
      left = Math.min(left, dot.getBoundingClientRect().left - PLUS_SIZE - 4);
    }
    left = Math.max(box.left + 2, Math.min(left, box.left + pane.clientWidth - PLUS_SIZE - 2));
    const line = parseFloat(getComputedStyle(block).lineHeight);
    const top = rect.top + (Number.isFinite(line) ? Math.max(0, (line - PLUS_SIZE) / 2) : 0);
    this.plus.style.left = `${String(Math.round(left - box.left + pane.scrollLeft))}px`;
    this.plus.style.top = `${String(Math.round(top - box.top + pane.scrollTop))}px`;
    this.plus.hidden = false;
  }

  private readonly onSelectionChange = (): void => {
    cancelAnimationFrame(this.selFrame);
    this.selFrame = requestAnimationFrame(() => {
      this.selFrame = 0;
      this.syncSelButton();
    });
  };

  private readonly onPointerDown = (e: PointerEvent): void => {
    if (e.button === 0 && e.target !== this.selButton && e.target !== this.plus) {
      this.dragging = true;
    }
  };

  private readonly onPointerUp = (): void => {
    if (this.dragging) {
      this.dragging = false;
      this.syncSelButton();
    }
  };

  /** The "+" follows the pointer from block to block; it stays while level with its block. */
  private readonly onPointerMove = (e: PointerEvent): void => {
    const target = e.target instanceof Element ? e.target : null;
    if (target === this.plus) {
      return;
    }
    const doc = this.host.doc();
    if (!doc || !this.offering() || this.editor.isOpen || this.selected() !== null) {
      this.showPlus(null);
      return;
    }
    const block = target && doc.contains(target) ? leafAt(target, doc) : null;
    if (block === null && this.plusBlock?.isConnected) {
      // On the way to the "+", in the gutter beside its block.
      const rect = this.plusBlock.getBoundingClientRect();
      if (e.clientY >= rect.top && e.clientY < rect.bottom) {
        return;
      }
    }
    if (block !== this.plusBlock) {
      this.showPlus(block);
    }
  };

  private readonly onPointerLeave = (): void => {
    this.showPlus(null);
  };

  private readonly onScroll = (): void => {
    if (
      !this.selButton.hidden &&
      Math.abs(this.host.scroller.scrollTop - this.selScrollTop) > SCROLL_HIDES
    ) {
      this.hideSelButton();
    }
  };

  /** Esc, when nothing before it took it: leaves re-attach mode, else hides the selection button. */
  private readonly onKey = (e: KeyboardEvent): void => {
    if (e.key !== "Escape" || e.defaultPrevented || e.ctrlKey || e.altKey) {
      return;
    }
    if (this.attach !== null) {
      e.preventDefault();
      this.leaveAttach();
    } else if (!this.selButton.hidden) {
      e.preventDefault();
      this.hideSelButton();
    }
  };
}

/** A note's path as comments keep things by it: compared as on Windows, as `samePath` does. */
export function noteKey(path: string): string {
  return path.toLowerCase().replaceAll("/", "\\");
}

/**
 * `range` trimmed to the text it covers, as far as the note's text goes (see comments-text.ts):
 * each end moved into the first or last text node it selects a character of, so an end between
 * nodes, or at the very start of the next block (a triple click), counts where its text is.
 * Null when it covers no text.
 */
function textOnly(range: Range, doc: Element): Range | null {
  const root = range.commonAncestorContainer;
  const counts = (t: Text): boolean => {
    const from = t === range.startContainer ? range.startOffset : 0;
    const to = t === range.endContainer ? range.endOffset : t.length;
    return (
      range.intersectsNode(t) &&
      t.data.slice(from, to).trim() !== "" &&
      t.parentElement?.closest(SKIP) == null &&
      leafBlockOf(t, doc) !== null
    );
  };
  let first: Text | null = null;
  let last: Text | null = null;
  if (root instanceof Text) {
    first = last = counts(root) ? root : null;
  } else {
    // In from each end, so a long selection isn't walked whole.
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n && !first; n = walker.nextNode()) {
      if (n instanceof Text && counts(n)) first = n;
    }
    walker.currentNode = root;
    for (let n = walker.lastChild(); n && !last && n !== first; n = walker.previousNode()) {
      if (n instanceof Text && counts(n)) last = n;
    }
    last ??= first;
  }
  if (!first || !last) {
    return null;
  }
  const out = document.createRange();
  out.setStart(first, first === range.startContainer ? range.startOffset : 0);
  out.setEnd(last, last === range.endContainer ? range.endOffset : last.length);
  return out;
}

/** Where a range ends on screen: its last line's box, or its end's element's without layout. */
function endRect(range: Range): DOMRect {
  const rects =
    typeof range.getClientRects === "function"
      ? [...range.getClientRects()].filter((r) => r.width > 0 || r.height > 0)
      : [];
  const last = rects[rects.length - 1];
  if (last) {
    return last;
  }
  const end = range.endContainer;
  const el = end instanceof Element ? end : end.parentElement;
  return el?.getBoundingClientRect() ?? new DOMRect();
}

/** The block (`LEAF`) of the note holding `target`, the innermost with source lines; or null. */
function leafAt(target: Element, doc: Element): HTMLElement | null {
  for (
    let el = target.closest<HTMLElement>(LEAF);
    el && doc.contains(el);
    el = el.parentElement?.closest<HTMLElement>(LEAF) ?? null
  ) {
    if (el !== doc && blockLines(el) !== null) {
      return el;
    }
  }
  return null;
}

/** The deepest block (`LEAF`, with lines) in `el` starting at the same height, or `el` itself. */
function deepestAt(el: HTMLElement, top: number): HTMLElement {
  let best = el;
  for (const inner of el.querySelectorAll<HTMLElement>(LEAF)) {
    if (blockLines(inner) !== null && Math.abs(inner.getBoundingClientRect().top - top) < 1) {
      best = inner;
    }
  }
  return best;
}
