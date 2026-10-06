// Review comments in the reader (spec §6): the cards in the right panel's Comments tab, the
// highlights and margin dots that mark the commented text, live reload when the sidecar changes,
// and adding, re-attaching and editing comments. Loaded after the first paint while the feature is
// on.
//
// Contracts:
// - A review applies only for the note it was loaded for: a newer load, or another note on screen,
//   drops the answer. `review-changed` for another note is ignored.
// - The marks follow the list: each comment the filter shows that is attached to the note gets its
//   quote highlighted (`comment`, and `comment-focus` for the selected one) and a dot on its first
//   block. The highlight goes where core found the quote in the note's text (`textStart` to
//   `textEnd`): the note's visible text is the same on both sides. When the text there isn't the
//   quote, it goes on the quote's first place in its blocks instead. A re-render of the note wipes
//   the dots, so they're put back on every "doc".
// - The note's visible text is indexed once per render, when first needed (for a highlight, or
//   the text before a selection), and dropped on every "doc".
// - A click on a card's quote or header (not its buttons) selects it and goes to its text, as its
//   place button does; going there flashes the blocks holding the text for a moment.
// - Hidden comments (the header toggle) keep their cards and badge but mark nothing, and nothing
//   offers to add one.
// - Loads and operations go to the backend one at a time, in the order they were asked for, so
//   their answers apply in that order: an older answer never hides a newer Claude reply. An
//   operation is for the note it was asked on: one whose note is no longer on screen by its turn
//   isn't sent.
// - Reply drafts belong to their note and comment. They survive a reload of the comments (the box
//   keeps its caret), a failed send, a reply that couldn't be sent because the note changed, and
//   a visit to another note; they never move to another note. A reply box is disabled while its
//   reply is being saved.
// - Adding comments and re-attaching them are comment-adding.ts's.
// - Editing your own entry swaps its body for a box with its raw text, kept across renders, for
//   the note on screen only.
// - In focus mode the panel is hidden: the dots and the adding buttons hide too (comments.css) and
//   clicks on highlighted text do nothing; the highlights stay.
import type { Backend } from "./backend";
import { CommentAdding, noteKey, type Outcome } from "./comment-adding";
import {
  claudeEntryCount,
  copyCount,
  formatCopy,
  isOpen,
  matchPart,
  orderComments,
  placeLabel,
  type Filter,
} from "./comments-model";
import {
  blockForLine,
  buildTextIndex,
  elementsForLines,
  locate,
  normalize,
  rangeAt,
  type TextIndex,
} from "./comments-text";
import { h, samePath } from "./dom";
import type { CommentView } from "./generated/CommentView";
import type { DocChanged } from "./generated/DocChanged";
import type { EntryView } from "./generated/EntryView";
import type { ReviewOp } from "./generated/ReviewOp";
import type { ReviewPayload } from "./generated/ReviewPayload";
import type { RightPanel } from "./right-panel";

/** What the comments need from the app. */
export interface CommentsHost {
  backend: Backend;
  /** The document on screen, or null when there is none. */
  doc: () => HTMLElement | null;
  /** The path of the note on screen, or null. */
  docPath: () => string | null;
  panel: RightPanel;
  /** The element the document scrolls in. */
  scroller: HTMLElement;
  toast: (m: string) => void;
  /** The open comments, on the header button's badge. */
  setBadge: (n: number) => void;
  /** Scrolls the block holding source line `line` to the top. */
  jumpToLine: (line: number) => void;
  /** Follows a link as the document's own are followed. */
  follow: (a: HTMLAnchorElement) => void;
  /** Calls `cb` whenever the document area re-renders; returns the unsubscribe. */
  onDoc: (cb: () => void) => () => void;
  /** Whether comments show (the feature on, and not hidden by the header toggle). */
  visible: () => boolean;
  /** Whether the right panel shows. */
  panelOpen: () => boolean;
  /** Shows the right panel. */
  openPanel: () => void;
  /** Pulses the header button's badge for a moment. */
  pulseBadge: () => void;
  /** Whether focus mode (F11) is on, which hides the panel. */
  focusMode: () => boolean;
  /** Shows comments the header toggle hid. */
  showComments: () => void;
}

const EMPTY = "No comments yet. Select text in the note, or hover a paragraph and press +.";
/** Where a card scrolled to lands below the pane's top, in pixels. */
const CARD_GAP = 12;
/** How long the blocks a card went to stay flashed, in ms: its animation's (comments.css). */
const FLASH_MS = 600;

const supported = (): boolean => typeof Highlight === "function" && "highlights" in CSS;

export class CommentsController {
  private review: ReviewPayload | null = null;
  /** Bumped by every load: a load waiting its turn is skipped once a newer one is asked for. */
  private gen = 0;
  private filter: Filter = "open";
  private shown: boolean;
  /** Where each marked comment's text is in the note, by id. */
  private readonly ranges = new Map<number, Range[]>();
  /** The visible text of the note on screen, built when first needed after its render. */
  private index: TextIndex | null = null;
  private focused: number | null = null;
  /** The blocks flashed because a card went to them, and the timer that unflashes them. */
  private flashed: HTMLElement[] = [];
  private flashTimer: ReturnType<typeof setTimeout> | undefined;
  /** What's typed in the open reply boxes: by note (`noteKey`), then comment id. */
  private readonly drafts = new Map<string, Map<number, string>>();
  /** The replies being saved, by `draftKey`: their boxes are disabled meanwhile. */
  private readonly sending = new Set<string>();
  /** The open reply boxes of the note on screen, kept across renders: by comment id. */
  private readonly replyBoxes = new Map<number, ReplyBox>();
  /** The loads and operations sent or waiting: each starts once the one before it is done. */
  private queue: Promise<unknown> = Promise.resolve();
  private readonly head: HTMLElement;
  private readonly banner: HTMLElement;
  private readonly list: HTMLElement;
  private readonly filterButtons: Record<Filter, HTMLButtonElement>;
  private readonly copyGroup: HTMLElement;
  private readonly menuButton: HTMLButtonElement;
  private readonly menu: HTMLElement;
  /** The entry being edited, kept across renders. */
  private edit: EditBox | null = null;
  /** The selection button, the block "+", the editor and re-attach mode. */
  private readonly adding: CommentAdding;
  private readonly stops: (() => void)[] = [];
  private disposed = false;

  constructor(private readonly host: CommentsHost) {
    this.shown = host.visible();
    const filter = (f: Filter, label: string): HTMLButtonElement => {
      const b = h(
        "button",
        { type: "button", class: "comments-filter-btn", "data-filter": f },
        label,
      );
      b.addEventListener("click", () => {
        this.setFilter(f);
      });
      return b;
    };
    this.filterButtons = { open: filter("open", "Open"), all: filter("all", "All") };
    const copy = h("button", { type: "button", class: "comments-copy-btn" }, "Copy comments");
    copy.addEventListener("click", () => void this.copy({ includeResolved: false }));
    this.menuButton = h(
      "button",
      {
        type: "button",
        class: "comments-copy-more",
        "aria-label": "More ways to copy",
        "aria-haspopup": "menu",
        "aria-expanded": "false",
      },
      "▾",
    );
    this.menuButton.addEventListener("click", () => {
      if (this.menu.hidden) this.openMenu();
      else this.closeMenu();
    });
    const copyAll = h(
      "button",
      { type: "button", role: "menuitem", class: "comments-menu-item" },
      "Copy all, including resolved",
    );
    copyAll.addEventListener("click", () => {
      this.closeMenu();
      void this.copy({ includeResolved: true });
    });
    this.menu = h("div", { class: "comments-menu", role: "menu" }, copyAll);
    this.menu.hidden = true;
    this.menu.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.closeMenu();
        this.menuButton.focus();
      }
    });
    this.copyGroup = h("div", { class: "comments-copy" }, copy, this.menuButton, this.menu);
    this.head = h(
      "div",
      { class: "comments-head" },
      h(
        "div",
        { class: "comments-filter", role: "group", "aria-label": "Which comments" },
        this.filterButtons.open,
        this.filterButtons.all,
      ),
      this.copyGroup,
    );
    this.banner = h("div", { class: "comments-banner", role: "note" });
    this.list = h("div", { class: "comments-list" });
    const pane = host.panel.commentsPane;
    this.adding = new CommentAdding({
      doc: () => host.doc(),
      docPath: () => host.docPath(),
      scroller: host.scroller,
      focusMode: () => host.focusMode(),
      toast: (m) => {
        host.toast(m);
      },
      shown: () => this.shown,
      show: () => {
        this.ensureShown();
      },
      readOnly: () => this.review?.readOnly ?? null,
      textIndex: () => this.docIndex(),
      perform: (op, path) => this.perform(op, path),
      showing: (path) => this.showing(path),
      ids: () => this.review?.comments.map((c) => c.id) ?? [],
      select: (id, reveal) => {
        if (reveal) this.reveal(id);
        else this.select(id, false);
      },
    });
    pane.replaceChildren(this.head, this.banner, this.adding.attachBanner, this.list);
    pane.addEventListener("click", this.onPaneClick);
    host.scroller.addEventListener("click", this.onNoteClick);
    this.stops.push(
      () => {
        pane.removeEventListener("click", this.onPaneClick);
        host.scroller.removeEventListener("click", this.onNoteClick);
      },
      host.backend.on<DocChanged>("review-changed", (e) => {
        const path = host.docPath();
        if (path !== null && samePath(e.path, path)) void this.load();
      }),
      host.onDoc(() => {
        this.onDoc();
      }),
    );
    this.syncFilter();
    this.render();
  }

  /** The review on screen, or null. */
  get payload(): ReviewPayload | null {
    return this.review;
  }

  /**
   * Loads the review of the note on screen, after the loads and operations before it. Skipped
   * when a newer load is asked for before its turn; an answer for a note no longer on screen is
   * dropped.
   */
  load(): Promise<void> {
    const gen = ++this.gen;
    const path = this.host.docPath();
    if (path === null || this.disposed) {
      return Promise.resolve();
    }
    return this.enqueue(() => this.fetch(gen, path));
  }

  private async fetch(gen: number, path: string): Promise<void> {
    if (gen !== this.gen || !this.showing(path)) {
      return;
    }
    let payload: ReviewPayload;
    try {
      payload = await this.host.backend.loadReview(path);
    } catch (e) {
      if (gen === this.gen && this.showing(path)) {
        this.host.toast(String(e));
      }
      return;
    }
    if (gen === this.gen && this.showing(path)) {
      this.apply(payload);
    }
  }

  /** Shows `payload`: the cards, the badge, the marks. Claude writing flashes the tab. */
  apply(payload: ReviewPayload): void {
    const before = this.review;
    this.review = payload;
    if (
      before !== null &&
      samePath(before.notePath, payload.notePath) &&
      claudeEntryCount(payload) > claudeEntryCount(before)
    ) {
      this.host.panel.flash();
      if (!this.host.panelOpen() || !this.shown) this.host.pulseBadge();
    }
    const ids = new Set(payload.comments.map((c) => c.id));
    if (this.focused !== null && !ids.has(this.focused)) this.focused = null;
    this.adding.reviewChanged(ids);
    const drafts = this.drafts.get(noteKey(payload.notePath));
    for (const id of drafts?.keys() ?? []) {
      if (!ids.has(id)) drafts?.delete(id);
    }
    this.host.setBadge(payload.openCount);
    this.host.panel.setCount(payload.openCount);
    this.render();
    this.decorate();
  }

  /** Shows or hides the marks in the note; hidden, nothing offers to add a comment. */
  setVisible(on: boolean): void {
    if (on === this.shown) {
      return;
    }
    this.shown = on;
    this.adding.setVisible(on);
    this.decorate();
  }

  /**
   * A comment on the text selected in the note, in the editor; in re-attach mode, the comment
   * being re-attached moved to it. Hidden comments show first. False when nothing in the note is
   * selected.
   */
  addFromSelection(): boolean {
    return this.adding.addFromSelection();
  }

  /**
   * A comment on the block at the top of the view: the deepest crossing it, else the first below
   * it. Hidden comments show first.
   */
  addAtTop(): void {
    this.adding.addAtTop();
  }

  /** Re-attach mode for comment `id`: the selection button attaches it to the selected text. */
  startReattach(id: number): void {
    this.adding.startReattach(id);
  }

  /** Selects a comment's card, highlights its text as the focused one and brings it into view. */
  focus(id: number): void {
    this.select(id, false);
  }

  /**
   * Applies an operation and shows the review it leaves; a failure is toasted. True if applied.
   * It waits for the loads and operations before it, so answers never apply out of order; one
   * whose note is no longer on screen by its turn isn't sent.
   */
  async run(op: ReviewOp): Promise<boolean> {
    return (await this.perform(op)) === "applied";
  }

  /** `run`, for the note at `path` (the one on screen), saying why it wasn't applied. */
  private perform(op: ReviewOp, path = this.host.docPath()): Promise<Outcome> {
    if (path === null || this.disposed) {
      return Promise.resolve("skipped");
    }
    return this.enqueue(() => this.send(path, op));
  }

  /** Runs `task` once everything asked for before it is done. */
  private enqueue<T>(task: () => Promise<T>): Promise<T> {
    const done = this.queue.then(task);
    this.queue = done.catch(() => undefined);
    return done;
  }

  private async send(path: string, op: ReviewOp): Promise<Outcome> {
    if (!this.showing(path)) {
      return "skipped";
    }
    let payload: ReviewPayload;
    try {
      payload = await this.host.backend.reviewOp(path, op);
    } catch (e) {
      this.host.toast(String(e));
      return "failed";
    }
    if (this.showing(path)) {
      this.apply(payload);
    }
    return "applied";
  }

  /** Removes everything the comments added: the marks, the cards, the badge, the listeners. */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.gen++;
    for (const stop of this.stops) stop();
    this.closeMenu();
    this.adding.dispose();
    this.edit = null;
    this.unflash();
    this.unmark();
    this.index = null;
    if (supported()) {
      CSS.highlights.delete("comment");
      CSS.highlights.delete("comment-focus");
    }
    this.host.panel.commentsPane.replaceChildren();
    this.host.setBadge(0);
    this.host.panel.setCount(0);
  }

  /** Whether the note at `path` is still the one on screen, for an answer about it. */
  private showing(path: string): boolean {
    return !this.disposed && this.host.docPath() === path;
  }

  /**
   * The document re-rendered or changed: the marks go back, and the review loads again. Another
   * note ends what was being added, re-attached or edited on the last.
   */
  private onDoc(): void {
    this.index = null;
    const path = this.host.docPath();
    const note = path === null ? null : noteKey(path);
    this.adding.docChanged();
    if (this.edit !== null && this.edit.note !== note) this.edit = null;
    if (this.review !== null && (path === null || !samePath(this.review.notePath, path))) {
      this.review = null;
      this.focused = null;
      this.replyBoxes.clear();
      this.host.setBadge(0);
      this.host.panel.setCount(0);
      this.render();
    }
    this.decorate();
    void this.load();
  }

  private setFilter(f: Filter): void {
    this.filter = f;
    this.syncFilter();
    this.render();
    this.decorate();
  }

  private syncFilter(): void {
    for (const f of ["open", "all"] as const) {
      this.filterButtons[f].setAttribute("aria-pressed", String(f === this.filter));
    }
  }

  /** The pane: the read-only banner, the cards (detached first) or why there are none. */
  private render(): void {
    const p = this.review;
    const pane = this.host.panel.commentsPane;
    const scrollTop = pane.scrollTop;
    // A reply box being typed in comes back as it was: the same element, focus and selection.
    const active = document.activeElement;
    const typing =
      active instanceof HTMLTextAreaElement && this.list.contains(active)
        ? {
            area: active,
            start: active.selectionStart,
            end: active.selectionEnd,
            direction: active.selectionDirection,
          }
        : null;
    const drafts = p === null ? undefined : this.drafts.get(noteKey(p.notePath));
    for (const id of this.replyBoxes.keys()) {
      if (!drafts?.has(id)) this.replyBoxes.delete(id);
    }
    if (this.edit !== null && !this.editable(this.edit)) {
      this.edit = null;
    }
    this.head.hidden = p === null;
    this.banner.hidden = p?.readOnly == null;
    this.banner.textContent = p?.readOnly ?? "";
    if (p === null) {
      this.list.replaceChildren();
      return;
    }
    const readOnly = p.readOnly !== null;
    const { detached, attached } = orderComments(p.comments, this.filter);
    const items: HTMLElement[] = [];
    if (detached.length > 0) {
      items.push(
        h(
          "section",
          { class: "comment-group detached" },
          h("h3", { class: "comment-group-title" }, "Detached"),
          ...detached.map((c) => this.card(c, readOnly, p.notePath)),
        ),
      );
    }
    items.push(...attached.map((c) => this.card(c, readOnly, p.notePath)));
    for (const u of p.unreadable) {
      items.push(
        h(
          "article",
          { class: "comment-card unreadable" },
          h("p", { class: "comment-unreadable-title" }, "Couldn't read this comment"),
          h("pre", { class: "comment-raw" }, u.raw),
        ),
      );
    }
    if (items.length === 0) {
      items.push(
        h("p", { class: "comments-empty" }, p.comments.length === 0 ? EMPTY : "No open comments."),
      );
    }
    this.list.replaceChildren(...items);
    pane.scrollTop = scrollTop;
    if (typing?.area.isConnected) {
      typing.area.focus({ preventScroll: true });
      typing.area.setSelectionRange(typing.start, typing.end, typing.direction);
    }
  }

  /** The drafts of the note on screen, if it has any. */
  private currentDrafts(): Map<number, string> | undefined {
    const path = this.host.docPath();
    return path === null ? undefined : this.drafts.get(noteKey(path));
  }

  /** The drafts of the note at `path`, made when it has none yet. */
  private draftsOf(path: string): Map<number, string> {
    const key = noteKey(path);
    let drafts = this.drafts.get(key);
    if (!drafts) {
      drafts = new Map();
      this.drafts.set(key, drafts);
    }
    return drafts;
  }

  private card(c: CommentView, readOnly: boolean, notePath: string): HTMLElement {
    const place = h(
      "button",
      {
        type: "button",
        class: "comment-place",
        "data-action": "jump",
        title: c.state === "detached" ? "Where it was" : "Go to the text",
      },
      placeLabel(c),
    );
    place.disabled = c.jumpLine === null;
    const head = h(
      "header",
      { class: "comment-head" },
      h("span", { class: `comment-status status-${c.status}` }, c.status),
      h("span", { class: "comment-id" }, `C${String(c.id)}`),
      place,
    );
    if (c.state === "detached") {
      head.append(h("span", { class: "comment-chip detached" }, "Detached"));
    }
    const parts: HTMLElement[] = [head, h("blockquote", { class: "comment-quote" }, c.quote)];
    if (c.state === "moved") {
      parts.push(
        h(
          "div",
          { class: "comment-moved" },
          h("span", { class: "comment-chip" }, "text changed"),
          h("p", { class: "comment-current" }, c.currentText ?? ""),
        ),
      );
    }
    parts.push(
      h(
        "div",
        { class: "comment-thread" },
        ...c.entries.map((e, i) => this.entryEl(c.id, e, i, readOnly)),
      ),
    );
    const action = (name: string, label: string, disabled = readOnly): HTMLButtonElement => {
      const b = h(
        "button",
        { type: "button", class: "comment-action", "data-action": name },
        label,
      );
      b.disabled = disabled;
      return b;
    };
    const actions = [
      action("reply", "Reply"),
      isOpen(c) ? action("resolve", "Resolve") : action("reopen", "Reopen"),
    ];
    if (c.status !== "dismissed") actions.push(action("dismiss", "Dismiss"));
    actions.push(action("copy", "Copy", false));
    if (c.state !== "anchored") {
      const reattach = action("reattach", "Re-attach");
      reattach.title = "Attach this comment to other text";
      actions.push(reattach);
    }
    parts.push(h("footer", { class: "comment-actions" }, ...actions));
    if (!readOnly && this.drafts.get(noteKey(notePath))?.has(c.id)) {
      parts.push(this.replyBox(c.id, notePath));
    }
    return h(
      "article",
      {
        class: c.id === this.focused ? "comment-card selected" : "comment-card",
        "data-id": String(c.id),
        "data-state": c.state,
        "data-status": c.status,
      },
      ...parts,
    );
  }

  /**
   * One entry of a thread: who wrote it, Claude's kind, and its Markdown as core rendered it. Your
   * own entries can be edited (`edit` action); the one being edited shows its box instead.
   */
  private entryEl(id: number, e: EntryView, index: number, readOnly: boolean): HTMLElement {
    const author = h("div", { class: "comment-author" }, e.author === "you" ? "You" : "Claude");
    if (e.author === "claude" && e.kind !== null) {
      author.append(" ", h("span", { class: `comment-kind kind-${e.kind}` }, e.kind));
    }
    const edit = this.edit;
    if (edit !== null && edit.id === id && edit.entry === index) {
      edit.sync();
      return h("div", { class: "comment-entry you editing" }, author, edit.el);
    }
    const body = h("div", { class: "comment-body" });
    // Sanitised by core, with the note's link rules.
    body.innerHTML = e.html;
    const el = h("div", { class: `comment-entry ${e.author}` }, author, body);
    if (e.author === "you" && !readOnly) {
      author.after(
        h(
          "button",
          {
            type: "button",
            class: "comment-edit",
            "data-action": "edit",
            "data-entry": String(index),
          },
          "Edit",
        ),
      );
    }
    return el;
  }

  /** Whether an edit still has its entry, yours, in the review on screen. */
  private editable(edit: EditBox): boolean {
    const p = this.review;
    const e = p?.comments.find((c) => c.id === edit.id)?.entries[edit.entry];
    return (
      p !== null && p.readOnly === null && noteKey(p.notePath) === edit.note && e?.author === "you"
    );
  }

  /** Opens the box for editing your entry `index` of comment `id`, in place of its body. */
  private openEdit(id: number, index: number): void {
    const path = this.host.docPath();
    const e = this.review?.comments.find((c) => c.id === id)?.entries[index];
    if (path === null || e?.author !== "you") {
      return;
    }
    this.edit = this.editBox(noteKey(path), id, index, e.text);
    this.render();
    this.edit.area.focus();
  }

  /** An edit box: Ctrl+Enter saves, Esc cancels, empty text can't be saved; disabled while saving. */
  private editBox(note: string, id: number, entry: number, text: string): EditBox {
    const area = h("textarea", {
      class: "comment-reply-text comment-edit-text",
      rows: "3",
      "aria-label": `Edit your entry in C${String(id)}`,
    });
    area.value = text;
    const save = h("button", { type: "button", class: "btn" }, "Save");
    const cancel = h("button", { type: "button", class: "btn" }, "Cancel");
    const box: EditBox = {
      note,
      id,
      entry,
      busy: false,
      area,
      el: h(
        "div",
        { class: "comment-reply comment-edit-box" },
        area,
        h("div", { class: "comment-reply-actions" }, save, cancel),
      ),
      sync: () => {
        area.disabled = box.busy;
        save.disabled = box.busy || area.value.trim() === "";
      },
    };
    box.sync();
    area.addEventListener("input", box.sync);
    area.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.ctrlKey) {
        e.preventDefault();
        void this.saveEdit(box);
      } else if (e.key === "Escape") {
        // Before the app's Esc, which would close something else.
        e.preventDefault();
        this.closeEdit(box);
      }
    });
    save.addEventListener("click", () => void this.saveEdit(box));
    cancel.addEventListener("click", () => {
      this.closeEdit(box);
    });
    return box;
  }

  private closeEdit(box: EditBox): void {
    if (this.edit === box) {
      this.edit = null;
      this.render();
    }
  }

  /**
   * Saves an edit box's text as its entry, its box disabled meanwhile. Saved, the box goes;
   * otherwise (toasted) it comes back as it was.
   */
  private async saveEdit(box: EditBox): Promise<void> {
    const text = box.area.value.trim();
    const path = this.host.docPath();
    if (text === "" || box.busy || path === null || noteKey(path) !== box.note) {
      return;
    }
    box.busy = true;
    box.sync();
    const outcome = await this.perform({ op: "edit", id: box.id, entry: box.entry, text }, path);
    box.busy = false;
    if (outcome === "applied") {
      this.closeEdit(box);
      return;
    }
    if (outcome === "skipped" && !this.disposed) {
      this.host.toast("Your edit wasn't saved: the note changed.");
    }
    box.sync();
    if (box.el.isConnected) {
      box.area.focus();
    }
  }

  /**
   * A reply box: Ctrl+Enter sends, Esc cancels, an empty reply can't be sent. Made once per
   * opening and kept across renders.
   */
  private replyBox(id: number, notePath: string): HTMLElement {
    const kept = this.replyBoxes.get(id);
    if (kept) {
      kept.sync();
      return kept.el;
    }
    const drafts = this.draftsOf(notePath);
    const key = draftKey(notePath, id);
    const area = h("textarea", {
      class: "comment-reply-text",
      rows: "3",
      placeholder: "Reply…",
      "aria-label": `Reply to C${String(id)}`,
    });
    area.value = drafts.get(id) ?? "";
    const send = h("button", { type: "button", class: "btn", "data-action": "send" }, "Send");
    const cancel = h("button", { type: "button", class: "btn", "data-action": "cancel" }, "Cancel");
    // Disabled while its reply is being saved, so what it shows is what was sent.
    const sync = (): void => {
      const busy = this.sending.has(key);
      area.disabled = busy;
      send.disabled = busy || area.value.trim() === "";
    };
    sync();
    area.addEventListener("input", () => {
      drafts.set(id, area.value);
      sync();
    });
    area.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.ctrlKey) {
        e.preventDefault();
        void this.sendReply(id);
      } else if (e.key === "Escape") {
        // Before the app's Esc, which would close something else.
        e.preventDefault();
        this.closeReply(id);
      }
    });
    const el = h(
      "div",
      { class: "comment-reply" },
      area,
      h("div", { class: "comment-reply-actions" }, send, cancel),
    );
    this.replyBoxes.set(id, { el, area, sync });
    return el;
  }

  private openReply(id: number): void {
    const path = this.host.docPath();
    if (path === null) {
      return;
    }
    const drafts = this.draftsOf(path);
    if (!drafts.has(id)) {
      drafts.set(id, "");
      this.render();
    }
    this.list
      .querySelector<HTMLTextAreaElement>(`.comment-card[data-id="${String(id)}"] textarea`)
      ?.focus();
  }

  private closeReply(id: number): void {
    this.currentDrafts()?.delete(id);
    this.render();
  }

  /**
   * Sends a reply box's text, its box disabled meanwhile. Saved, the draft goes; otherwise it
   * stays the draft of its own note and comment, never another's: after a failure (toasted) its
   * box comes back as it was, and a reply not sent because the note changed waits for the note.
   */
  private async sendReply(id: number): Promise<void> {
    const path = this.host.docPath();
    if (path === null) {
      return;
    }
    const key = draftKey(path, id);
    const text = this.drafts.get(noteKey(path))?.get(id)?.trim() ?? "";
    if (text === "" || this.sending.has(key)) {
      return;
    }
    this.sending.add(key);
    this.replyBoxes.get(id)?.sync();
    const outcome = await this.perform({ op: "reply", id, text });
    this.sending.delete(key);
    if (outcome === "applied") {
      this.draftsOf(path).delete(id);
    } else if (outcome === "skipped" && !this.disposed) {
      this.host.toast("Your reply wasn't sent: the note changed. It's kept as a draft.");
    }
    if (!this.showing(path)) {
      return;
    }
    const box = this.replyBoxes.get(id);
    if (outcome !== "applied" && box?.el.isConnected) {
      box.sync();
      if (document.activeElement === document.body || document.activeElement === null) {
        box.area.focus();
      }
    } else {
      this.render();
    }
  }

  private async copy(opts: { includeResolved: boolean; ids?: number[] }): Promise<void> {
    const p = this.review;
    if (p === null) {
      return;
    }
    const n = copyCount(p, opts);
    if (n === 0) {
      this.host.toast("No comments to copy");
      return;
    }
    await this.toClipboard(
      formatCopy(p, opts),
      `Copied ${String(n)} ${n === 1 ? "comment" : "comments"}`,
    );
  }

  /** A code block's Copy button in a comment: its code, as the note's own copy it. */
  private async copyCode(button: HTMLElement): Promise<void> {
    const code = button.closest(".code-block")?.querySelector("pre code")?.textContent;
    if (code !== undefined) {
      await this.toClipboard(code, "Copied the code");
    }
  }

  /** Puts `text` on the clipboard and says so, or that it couldn't. */
  private async toClipboard(text: string, done: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      this.host.toast(done);
    } catch {
      this.host.toast("Couldn't copy to the clipboard");
    }
  }

  private openMenu(): void {
    this.menu.hidden = false;
    this.menuButton.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", this.onOutside, true);
    this.menu.querySelector<HTMLButtonElement>("button")?.focus();
  }

  private closeMenu(): void {
    this.menu.hidden = true;
    this.menuButton.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", this.onOutside, true);
  }

  private readonly onOutside = (e: PointerEvent): void => {
    if (!(e.target instanceof Node) || !this.copyGroup.contains(e.target)) {
      this.closeMenu();
    }
  };

  /** The cards' buttons, and links and code blocks' Copy in comments, which work as the note's do. */
  private readonly onPaneClick = (e: MouseEvent): void => {
    const target = e.target instanceof Element ? e.target : null;
    const codeCopy = target?.closest<HTMLElement>(".comment-body .code-copy");
    if (codeCopy) {
      void this.copyCode(codeCopy);
      return;
    }
    const link = target?.closest("a");
    if (link && target?.closest(".comment-body")) {
      e.preventDefault();
      this.host.follow(link);
      return;
    }
    const button = target?.closest<HTMLButtonElement>("button[data-action]");
    const id = Number(target?.closest<HTMLElement>(".comment-card")?.dataset.id);
    if (!button && Number.isInteger(id) && target && onCardPlace(target)) {
      this.select(id, true);
      return;
    }
    if (!button || button.disabled || !Number.isInteger(id)) {
      return;
    }
    const action = button.dataset.action;
    switch (action) {
      case "jump":
        this.select(id, true);
        break;
      case "reply":
        this.openReply(id);
        break;
      case "send":
        void this.sendReply(id);
        break;
      case "cancel":
        this.closeReply(id);
        break;
      case "resolve":
      case "reopen":
      case "dismiss":
        void this.run({ op: "setStatus", id, change: action });
        break;
      case "copy":
        void this.copy({ includeResolved: true, ids: [id] });
        break;
      case "reattach":
        this.startReattach(id);
        break;
      case "edit":
        this.openEdit(id, Number(button.dataset.entry));
        break;
    }
  };

  /** A click on a dot, or on highlighted text (not a link, not ending a selection), shows its card. */
  private readonly onNoteClick = (e: MouseEvent): void => {
    const doc = this.host.doc();
    const target = e.target instanceof Element ? e.target : null;
    if (!this.shown || this.host.focusMode() || !doc || !target || !doc.contains(target)) {
      return;
    }
    const dot = target.closest<HTMLElement>(".lx-cdot");
    if (dot) {
      const id = Number(dot.dataset.comments?.split(" ")[0]);
      if (Number.isInteger(id)) this.reveal(id);
      return;
    }
    if (target.closest("a") || this.ranges.size === 0 || !document.getSelection()?.isCollapsed) {
      return;
    }
    const caret =
      typeof document.caretPositionFromPoint === "function"
        ? document.caretPositionFromPoint(e.clientX, e.clientY)
        : null;
    if (!caret) {
      return;
    }
    for (const [id, ranges] of this.ranges) {
      if (ranges.some((r) => holds(r, caret))) {
        this.reveal(id);
        return;
      }
    }
  };

  /** Shows comments the header toggle hid. */
  private ensureShown(): void {
    if (!this.shown) {
      this.host.showComments();
      this.setVisible(this.host.visible());
    }
  }

  /** Shows a comment's card: the Comments tab, the panel opened if it was closed. */
  private reveal(id: number): void {
    this.host.panel.show("comments");
    if (!this.host.panelOpen()) {
      this.host.openPanel();
    }
    this.select(id, false);
  }

  /**
   * Selects a comment: its card, and its text as the focused highlight. Its text scrolls into view
   * when `jump` (the card asked to go there), flashing its blocks, or when it's off screen.
   */
  private select(id: number, jump: boolean): void {
    const c = this.review?.comments.find((x) => x.id === id);
    if (!c) {
      return;
    }
    this.focused = id;
    for (const el of this.list.querySelectorAll(".comment-card.selected")) {
      el.classList.remove("selected");
    }
    const card = this.list.querySelector<HTMLElement>(`.comment-card[data-id="${String(id)}"]`);
    if (card) {
      card.classList.add("selected");
      this.scrollCardIntoView(card);
    }
    this.paint();
    if (c.jumpLine !== null && (jump || !this.onScreen(id))) {
      this.host.jumpToLine(c.jumpLine);
    }
    if (jump) {
      this.flash(c);
    }
  }

  /** Flashes the blocks holding a comment's highlighted text for a moment; nothing if it has none. */
  private flash(c: CommentView): void {
    this.unflash();
    const doc = this.host.doc();
    if (!doc || !this.ranges.has(c.id)) {
      return;
    }
    this.flashed = elementsForLines(doc, c.startLine, c.endLine);
    // Lays out with the class gone first, so flashing the same blocks again starts over.
    doc.getBoundingClientRect();
    for (const el of this.flashed) {
      el.classList.add("lx-flash");
    }
    this.flashTimer = setTimeout(() => {
      this.unflash();
    }, FLASH_MS);
  }

  private unflash(): void {
    clearTimeout(this.flashTimer);
    for (const el of this.flashed) {
      el.classList.remove("lx-flash");
    }
    this.flashed = [];
  }

  private scrollCardIntoView(card: HTMLElement): void {
    const pane = this.host.panel.commentsPane;
    const top =
      card.getBoundingClientRect().top - pane.getBoundingClientRect().top + pane.scrollTop;
    if (top < pane.scrollTop || top + card.offsetHeight > pane.scrollTop + pane.clientHeight) {
      pane.scrollTop = Math.max(0, top - CARD_GAP);
    }
  }

  /** Whether a comment's highlighted text is in the document pane's view. */
  private onScreen(id: number): boolean {
    const el = this.ranges.get(id)?.[0]?.startContainer.parentElement;
    if (!el) {
      return false;
    }
    const r = el.getBoundingClientRect();
    const view = this.host.scroller.getBoundingClientRect();
    return r.bottom > view.top && r.top < view.bottom;
  }

  /** Marks the comments the list shows in the note: highlights and dots, once each. */
  private decorate(): void {
    this.unmark();
    const doc = this.host.doc();
    const path = this.host.docPath();
    const p = this.review;
    if (this.shown && doc && p && path !== null && samePath(p.notePath, path)) {
      const blocks = new Map<HTMLElement, number[]>();
      for (const c of orderComments(p.comments, this.filter).attached) {
        const range = this.rangeOf(c, doc);
        if (range) {
          this.ranges.set(c.id, [range]);
        }
        const block = blockForLine(doc, c.startLine);
        if (block) {
          blocks.set(block, [...(blocks.get(block) ?? []), c.id]);
        }
      }
      for (const [block, ids] of blocks) {
        addDot(block, ids);
      }
    }
    this.paint();
  }

  /** The visible text of the note on screen, indexed on first use after its render; or null. */
  private docIndex(): TextIndex | null {
    const doc = this.host.doc();
    if (!doc) {
      return null;
    }
    this.index ??= buildTextIndex([doc]);
    return this.index;
  }

  /**
   * Where a comment's text is in `doc`: its quote, or the passage it became when it moved. At the
   * place core found it when the note's text holds it there, else its first place in its blocks.
   */
  private rangeOf(c: CommentView, doc: HTMLElement): Range | null {
    const needle =
      c.state === "moved" && c.currentText !== null ? c.currentText : matchPart(c.quote);
    if (c.textStart !== null && c.textEnd !== null) {
      const index = this.docIndex();
      if (index && index.text.slice(c.textStart, c.textEnd) === normalize(needle)) {
        const range = rangeAt(index, c.textStart, c.textEnd);
        if (range) {
          return range;
        }
      }
    }
    const els = elementsForLines(doc, c.startLine, c.endLine);
    return els.length > 0 ? locate(buildTextIndex(els), needle) : null;
  }

  /** Takes the dots out of the note and forgets the highlighted ranges. */
  private unmark(): void {
    const doc = this.host.doc();
    for (const dot of doc?.querySelectorAll(".lx-cdot") ?? []) {
      dot.remove();
    }
    for (const el of doc?.querySelectorAll(".lx-has-comment") ?? []) {
      el.classList.remove("lx-has-comment");
    }
    this.ranges.clear();
  }

  /** Paints the highlights: every marked comment, and the focused one above them. */
  private paint(): void {
    if (!supported()) {
      return;
    }
    const all = [...this.ranges.values()].flat();
    if (all.length > 0) {
      const highlight = new Highlight(...all);
      highlight.priority = 0;
      CSS.highlights.set("comment", highlight);
    } else {
      CSS.highlights.delete("comment");
    }
    const focus = this.focused === null ? undefined : this.ranges.get(this.focused);
    if (focus && focus.length > 0) {
      const highlight = new Highlight(...focus);
      highlight.priority = 1;
      CSS.highlights.set("comment-focus", highlight);
    } else {
      CSS.highlights.delete("comment-focus");
    }
  }
}

/** A reply box, and how to bring its disabled state up to date. */
interface ReplyBox {
  el: HTMLElement;
  area: HTMLTextAreaElement;
  sync: () => void;
}

/** The box editing your entry `entry` of comment `id` on a note (`noteKey`). */
interface EditBox extends ReplyBox {
  note: string;
  id: number;
  entry: number;
  /** Its text is being saved. */
  busy: boolean;
}

/**
 * Whether a click on `target` in a card is one that goes to its text: on its quote or header, and
 * not on a button, a link or a text box there.
 */
function onCardPlace(target: Element): boolean {
  return (
    target.closest(".comment-quote, .comment-head") !== null &&
    target.closest("button, a, textarea") === null &&
    document.getSelection()?.isCollapsed !== false
  );
}

/** A reply draft's key: its note and comment. */
function draftKey(path: string, id: number): string {
  return `${noteKey(path)}#${String(id)}`;
}

/** A block's dot for the comments `ids`: its first child, or in its first cell for a table row. */
function addDot(block: HTMLElement, ids: number[]): void {
  const names = ids.map((id) => `C${String(id)}`).join(", ");
  const label = `${ids.length === 1 ? "Comment" : "Comments"} ${names}`;
  const dot = h("button", {
    type: "button",
    class: "lx-cdot",
    "data-comments": ids.join(" "),
    "aria-label": label,
    title: label,
  });
  const where = block.tagName === "TR" ? (block.firstElementChild ?? block) : block;
  where.prepend(dot);
  block.classList.add("lx-has-comment");
}

/** Whether `caret`'s point lies in `range`; false when they can't be compared. */
function holds(range: Range, caret: CaretPosition): boolean {
  try {
    return range.isPointInRange(caret.offsetNode, caret.offset);
  } catch {
    return false;
  }
}
