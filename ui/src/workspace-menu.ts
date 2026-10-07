// The workspaces' lists and prompts (design §1), loaded on first use: the dropdown under the
// header's workspace chip, a blank window's "Choose a workspace" list, the name field a new
// workspace (or a rename) takes, the name prompt for a folder chosen in a blank window, and the
// confirms before unsaved comment text is left behind (here, or in another window as Lectern
// quits or restarts for an update, or as this window closes) and before a workspace is deleted.
//
// Contracts:
// - A row names a workspace, with a hint: its first folder's name, plus "+N" for more. Its button
//   opens it here; its Open in new window button, always shown, opens it in a new window. One open
//   in another window says "Switch to window" there instead, and opening it brings that window
//   forward. The window's own is checked, and has neither.
// - ↑ and ↓ move between the rows and the actions below them, Enter opens here, Ctrl+Enter in a
//   new window, and Esc closes the dropdown (or, from a name field, cancels the field).
// - A name field starts with its text selected, so typing replaces it. Enter takes its first
//   choice (Open here, or the rename), Ctrl+Enter the second (New window), and Esc cancels.
import { h } from "./dom";
import type { OpenWhere } from "./generated/OpenWhere";
import type { WorkspaceSummary } from "./generated/WorkspaceSummary";

/** What the lists need from the app (workspaces.ts). */
export interface MenuHost {
  /** Every workspace, in creation order. */
  list(): WorkspaceSummary[];
  /** The name a new workspace is offered. */
  suggestName(): Promise<string>;
  open(id: string, where: OpenWhere): void;
  create(name: string, where: OpenWhere): void;
  /** Renames workspace `id`; false when it couldn't. */
  rename(id: string, name: string): Promise<boolean>;
}

/** The gap kept between the dropdown and the window's edges, in pixels. */
const EDGE_PX = 8;
/** The gap between the chip and the dropdown below it, in pixels. */
const GAP_PX = 6;

/** Two overlapping windows, in the header icons' style (icons.ts). */
const NEW_WINDOW_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><rect x="5.5" y="2.5" width="8" height="7" rx="1.5"/><path d="M10.5 9.5v3a1.5 1.5 0 0 1-1.5 1.5H4a1.5 1.5 0 0 1-1.5-1.5V8A1.5 1.5 0 0 1 4 6.5h1.5"/></svg>';
/** A check, marking the window's own workspace. */
const CURRENT_ICON =
  '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';

/** A folder's name: the last part of its path, or the path itself for a drive's root. */
export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const name = trimmed.slice(Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/")) + 1);
  return name === "" ? path : name;
}

/** What a row says under a workspace's name: its first folder, and how many more it has. */
export function rootsHint(roots: string[]): string {
  const first = roots[0];
  if (first === undefined) {
    return "";
  }
  return roots.length > 1 ? `${folderName(first)} +${String(roots.length - 1)}` : folderName(first);
}

/** Moves the focus between a list's items with ↑, ↓, Home and End; true when it did. */
function moveFocus(container: HTMLElement, e: KeyboardEvent): boolean {
  if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) {
    return false;
  }
  const items = [
    ...container.querySelectorAll<HTMLButtonElement>(".ws-open, .ws-action:not(:disabled)"),
  ];
  const at = items.findIndex((item) => item === document.activeElement);
  let next: number;
  switch (e.key) {
    case "ArrowDown":
      next = (at + 1) % items.length;
      break;
    case "ArrowUp":
      next = at <= 0 ? items.length - 1 : at - 1;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = items.length - 1;
      break;
    default:
      return false;
  }
  e.preventDefault();
  items[next]?.focus();
  return true;
}

/** The id of the row whose button has the focus in `rows`, if one has. */
function focusedRow(rows: HTMLElement): string | null {
  const active = document.activeElement;
  return active instanceof HTMLElement && rows.contains(active)
    ? (active.dataset.id ?? null)
    : null;
}

/** Puts the focus back on the row `id`'s button, after the rows were drawn again. */
function refocus(rows: HTMLElement, id: string | null): void {
  if (id !== null) {
    [...rows.querySelectorAll<HTMLElement>(".ws-open")].find((b) => b.dataset.id === id)?.focus();
  }
}

/**
 * A workspace's row: a check for the window's own, the name and its folders' hint. Its button
 * calls `pick` with `here`, Ctrl+Enter on it with `newWindow`. At the right, a closed workspace
 * has an Open in new window button, always shown; one open in another window says "Switch to
 * window" instead, as both actions bring that window forward. The window's own has neither.
 */
function workspaceRow(
  ws: WorkspaceSummary,
  pick: (ws: WorkspaceSummary, where: OpenWhere) => void,
): HTMLElement {
  const elsewhere = ws.open && !ws.current;
  const mark = h("span", { class: "ws-mark", "aria-hidden": "true" });
  if (ws.current) {
    mark.innerHTML = CURRENT_ICON;
  }
  const open = h(
    "button",
    { type: "button", class: "ws-open", "data-id": ws.id, title: ws.name },
    mark,
    h("span", { class: "ws-name" }, ws.name),
    h("span", { class: "ws-hint" }, rootsHint(ws.roots)),
  );
  if (elsewhere) {
    open.append(h("span", { class: "ws-switch" }, "Switch to window"));
  }
  open.addEventListener("click", () => {
    pick(ws, "here");
  });
  open.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.ctrlKey) {
      e.preventDefault();
      pick(ws, "newWindow");
    }
  });
  const row = h("div", { class: "ws-row" }, open);
  if (ws.current) {
    open.setAttribute("aria-current", "true");
    row.classList.add("current");
  } else if (!elsewhere) {
    const second = h("button", {
      type: "button",
      class: "icon-btn ws-new-window",
      title: "Open in new window",
      "aria-label": "Open in new window",
    });
    second.innerHTML = NEW_WINDOW_ICON;
    second.addEventListener("click", () => {
      pick(ws, "newWindow");
    });
    row.append(second);
  }
  return row;
}

/** One of a name field's buttons: Enter takes the first, Ctrl+Enter the second. */
interface Choice {
  label: string;
  run: (name: string) => void;
}

/** A name field (see the module comment). `focus()` it once it is in the page. */
export class NameField {
  readonly el: HTMLElement;
  readonly input: HTMLInputElement;

  constructor(value: string, choices: Choice[], cancel: () => void) {
    this.input = h("input", {
      type: "text",
      class: "ws-name-input",
      "aria-label": "Workspace name",
      spellcheck: "false",
      autocomplete: "off",
      maxlength: "60",
    });
    this.input.value = value;
    const buttons = choices.map((choice, i) => {
      const button = h(
        "button",
        { type: "button", class: i === 0 ? "btn primary" : "btn" },
        choice.label,
      );
      button.addEventListener("click", () => {
        choice.run(this.input.value);
      });
      return button;
    });
    this.input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        const choice = choices[e.ctrlKey ? 1 : 0];
        if (choice && !e.altKey && !e.shiftKey) {
          e.preventDefault();
          choice.run(this.input.value);
        }
      } else if (e.key === "Escape") {
        // Before whatever holds the field, which would close too.
        e.preventDefault();
        e.stopPropagation();
        cancel();
      }
    });
    this.el = h(
      "div",
      { class: "ws-name-field" },
      this.input,
      h("div", { class: "ws-name-actions" }, ...buttons),
    );
  }

  /** Puts the focus in the field, its text selected. */
  focus(): void {
    this.input.focus();
    this.input.select();
  }
}

/** The New workspace field's choices: Open here, then New window. */
function createChoices(run: (name: string, where: OpenWhere) => void): Choice[] {
  return [
    {
      label: "Open here",
      run: (name) => {
        run(name, "here");
      },
    },
    {
      label: "New window",
      run: (name) => {
        run(name, "newWindow");
      },
    },
  ];
}

/** The name a new workspace is offered, or nothing when it can't be had. */
async function suggestion(suggest: () => Promise<string>): Promise<string> {
  try {
    return await suggest();
  } catch {
    return "";
  }
}

/**
 * The dropdown under the chip: a row per workspace, then New workspace and Rename "‹current›". It
 * never reaches below the window: the rows scroll, and the actions stay put below them. A click
 * outside or Esc closes it.
 */
export class WorkspaceMenu {
  private readonly el: HTMLElement;
  private readonly rows: HTMLElement;
  private readonly foot: HTMLElement;
  /** The chip it hangs from, while open. */
  private anchor: HTMLElement | null = null;
  /** The name field in place of the actions, while one is open. */
  private field: NameField | null = null;
  /** Bumped by every field asked for, so a slow suggestion for an older one is dropped. */
  private asks = 0;

  constructor(
    root: HTMLElement,
    private readonly host: MenuHost,
    private readonly onToggle: (open: boolean) => void,
  ) {
    this.rows = h("div", { class: "ws-rows" });
    this.foot = h("div", { class: "ws-foot" });
    this.el = h(
      "div",
      { class: "menu ws-menu", role: "dialog", "aria-label": "Workspaces" },
      this.rows,
      h("div", { class: "menu-sep", role: "separator" }),
      this.foot,
    );
    this.el.hidden = true;
    this.el.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      } else if (e.target instanceof HTMLButtonElement) {
        moveFocus(this.el, e);
      }
    });
    root.append(this.el);
  }

  get isOpen(): boolean {
    return this.anchor !== null;
  }

  /** Shows it under `anchor`, the window's own workspace focused. */
  open(anchor: HTMLElement): void {
    this.anchor = anchor;
    this.field = null;
    this.render();
    this.el.hidden = false;
    this.place(anchor);
    document.addEventListener("pointerdown", this.onOutside, true);
    this.onToggle(true);
    const rows = [...this.rows.querySelectorAll<HTMLButtonElement>(".ws-open")];
    (rows.find((b) => b.hasAttribute("aria-current")) ?? rows[0])?.focus();
  }

  /** Hides it; focus inside it goes back to the chip unless `restoreFocus` is false. */
  close(restoreFocus = true): void {
    const anchor = this.anchor;
    if (!anchor) {
      return;
    }
    const hadFocus = this.el.contains(document.activeElement);
    this.anchor = null;
    this.field = null;
    this.asks++;
    this.el.hidden = true;
    document.removeEventListener("pointerdown", this.onOutside, true);
    this.onToggle(false);
    if (hadFocus) {
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      if (restoreFocus && anchor.isConnected && !anchor.hidden) {
        anchor.focus({ preventScroll: true });
      }
    }
  }

  /** The workspaces changed: the rows follow, keeping the focus on the same one. */
  update(): void {
    if (this.isOpen) {
      this.render();
    }
  }

  private readonly onOutside = (e: Event): void => {
    const target = e.target instanceof Node ? e.target : null;
    if (target && !this.el.contains(target) && !this.anchor?.contains(target)) {
      this.close(false);
    }
  };

  /** Under the chip, inside the window, and no taller than the room left below the chip. */
  private place(anchor: HTMLElement): void {
    const box = anchor.getBoundingClientRect();
    const room = window.innerWidth - this.el.offsetWidth - EDGE_PX;
    this.el.style.left = `${String(Math.max(EDGE_PX, Math.min(box.left, room)))}px`;
    this.el.style.top = `${String(box.bottom + GAP_PX)}px`;
    this.el.style.maxHeight = `${String(window.innerHeight - box.bottom - GAP_PX - EDGE_PX)}px`;
  }

  private render(): void {
    const focused = focusedRow(this.rows);
    this.rows.replaceChildren(
      ...this.host.list().map((ws) =>
        workspaceRow(ws, (w, where) => {
          this.pick(w, where);
        }),
      ),
    );
    refocus(this.rows, focused);
    if (this.field === null) {
      this.renderActions();
    }
  }

  /** New workspace, then Rename "‹current›". */
  private renderActions(): void {
    const create = h("button", { type: "button", class: "menu-item ws-action" }, "New workspace");
    create.addEventListener("click", () => void this.askCreate());
    this.foot.replaceChildren(create);
    const current = this.host.list().find((ws) => ws.current);
    if (current) {
      const rename = h(
        "button",
        { type: "button", class: "menu-item ws-action" },
        `Rename "${current.name}"`,
      );
      rename.addEventListener("click", () => {
        this.askRename(current);
      });
      this.foot.append(rename);
    }
  }

  private pick(ws: WorkspaceSummary, where: OpenWhere): void {
    this.close();
    if (!ws.current) {
      this.host.open(ws.id, where);
    }
  }

  /** The New workspace field, once its suggested name is in. */
  private async askCreate(): Promise<void> {
    const ask = ++this.asks;
    const name = await suggestion(() => this.host.suggestName());
    if (ask !== this.asks || !this.isOpen) {
      return;
    }
    this.showField(
      new NameField(
        name,
        createChoices((typed, where) => {
          this.close();
          this.host.create(typed, where);
        }),
        () => {
          this.cancelField();
        },
      ),
    );
  }

  /** The Rename field, holding the name. */
  private askRename(ws: WorkspaceSummary): void {
    this.asks++;
    this.showField(
      new NameField(
        ws.name,
        [
          {
            label: "Rename",
            run: (typed) => {
              void this.host.rename(ws.id, typed).then((renamed) => {
                if (renamed) this.close();
              });
            },
          },
        ],
        () => {
          this.cancelField();
        },
      ),
    );
  }

  private showField(field: NameField): void {
    this.field = field;
    this.foot.replaceChildren(field.el);
    field.focus();
  }

  /** Back to the actions, the focus on the first. */
  private cancelField(): void {
    this.field = null;
    this.asks++;
    this.renderActions();
    this.foot.querySelector<HTMLElement>(".ws-action")?.focus();
  }
}

/**
 * A blank window's list, on its welcome screen: the rows, whose button opens here, and a New
 * workspace button that becomes the name field.
 */
export class WorkspaceChooser {
  readonly el: HTMLElement;
  private readonly rows: HTMLElement;
  private readonly foot: HTMLElement;
  private field: NameField | null = null;
  private asks = 0;

  constructor(private readonly host: MenuHost) {
    this.rows = h("div", { class: "ws-rows" });
    this.foot = h("div", { class: "ws-foot" });
    this.el = h("div", { class: "ws-chooser" }, this.rows, this.foot);
    this.el.addEventListener("keydown", (e) => {
      if (e.target instanceof HTMLButtonElement) {
        moveFocus(this.el, e);
      }
    });
    this.renderButton();
  }

  /** Puts the list in `slot` (a welcome screen drawn again keeps a field being typed in). */
  mount(slot: HTMLElement): void {
    slot.replaceChildren(this.el);
    this.update();
  }

  /** The workspaces changed: the rows follow, keeping the focus on the same one. */
  update(): void {
    const focused = focusedRow(this.rows);
    this.rows.replaceChildren(
      ...this.host.list().map((ws) =>
        workspaceRow(ws, (w, where) => {
          this.host.open(w.id, where);
        }),
      ),
    );
    refocus(this.rows, focused);
  }

  private renderButton(): void {
    const create = h("button", { type: "button", class: "btn primary ws-action" }, "New workspace");
    create.addEventListener("click", () => void this.askCreate());
    this.foot.replaceChildren(create);
  }

  private async askCreate(): Promise<void> {
    const ask = ++this.asks;
    const name = await suggestion(() => this.host.suggestName());
    if (ask !== this.asks) {
      return;
    }
    this.field = new NameField(
      name,
      createChoices((typed, where) => {
        this.host.create(typed, where);
      }),
      () => {
        this.field = null;
        this.asks++;
        this.renderButton();
        this.foot.querySelector<HTMLElement>(".ws-action")?.focus();
      },
    );
    this.foot.replaceChildren(this.field.el);
    this.field.focus();
  }
}

/**
 * Asks for the name of a new workspace that will hold `folder` (a folder chosen in a blank
 * window), offering `suggest`'s. Null when cancelled: Esc, or a click outside.
 */
export async function askName(
  root: HTMLElement,
  folder: string,
  suggest: () => Promise<string>,
): Promise<{ name: string; where: OpenWhere } | null> {
  const name = await suggestion(suggest);
  const returnFocus = document.activeElement;
  return new Promise((resolve) => {
    const done = (answer: { name: string; where: OpenWhere } | null): void => {
      backdrop.remove();
      if (answer === null && returnFocus instanceof HTMLElement && returnFocus.isConnected) {
        returnFocus.focus({ preventScroll: true });
      }
      resolve(answer);
    };
    const field = new NameField(
      name,
      createChoices((typed, where) => {
        done({ name: typed, where });
      }),
      () => {
        done(null);
      },
    );
    const dialog = h(
      "div",
      { class: "quick-open ws-ask", role: "dialog", "aria-label": "New workspace" },
      h("h2", { class: "ws-ask-title" }, "New workspace"),
      h("p", { class: "ws-ask-folder", title: folder }, folder),
      field.el,
    );
    dialog.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        done(null);
      }
    });
    const backdrop = h("div", { class: "qo-backdrop ws-ask-backdrop" }, dialog);
    backdrop.addEventListener("pointerdown", (e) => {
      if (e.target === backdrop) done(null);
    });
    root.append(backdrop);
    field.focus();
  });
}

/** What leaving this window's unsaved comment text behind is for. */
export type Leaving = "switch" | "quit" | "update";

const LEAVING: Record<Leaving, { text: string; go: string }> = {
  switch: {
    text: "Switching this window to another workspace discards what you've typed.",
    go: "Switch anyway",
  },
  quit: { text: "Quitting Lectern discards what you've typed.", go: "Quit anyway" },
  update: { text: "Updating Lectern discards what you've typed.", go: "Update anyway" },
};

/**
 * Asks before comment text that isn't saved is left behind, by switching this window to another
 * workspace, by quitting, or by updating, which restarts Lectern. True to go on.
 */
export function confirmLeave(root: HTMLElement, action: Leaving): Promise<boolean> {
  const { text, go } = LEAVING[action];
  return ask(root, "Your comment isn't saved", text, [
    { label: "Keep writing", go: false },
    { label: go, go: true },
  ]);
}

/**
 * Asks before Lectern quits while other windows hold comment text that isn't saved yet, named in
 * `windows` as Rust names them. True to quit all the same.
 */
export function confirmQuitElsewhere(root: HTMLElement, windows: string[]): Promise<boolean> {
  return ask(root, `Unsaved comment in ${listed(windows)}.`, "Quit anyway?", [
    { label: "Quit", go: true },
    { label: "Cancel", go: false },
  ]);
}

/**
 * Asks before Lectern restarts to update while other windows hold comment text that isn't saved
 * yet, named as `confirmQuitElsewhere` names them. True to update all the same.
 */
export function confirmUpdateElsewhere(root: HTMLElement, windows: string[]): Promise<boolean> {
  return ask(
    root,
    `Unsaved comment in ${listed(windows)}.`,
    "Lectern restarts to update, which discards it.",
    [
      { label: "Update anyway", go: true },
      { label: "Cancel", go: false },
    ],
  );
}

/** Asks before this window closes with comment text that isn't saved. True to close it. */
export function confirmDiscard(root: HTMLElement): Promise<boolean> {
  return ask(
    root,
    "Discard the unsaved comment?",
    "Closing this window discards what you've typed.",
    [
      { label: "Discard", go: true },
      { label: "Cancel", go: false },
    ],
  );
}

/** "A", "A and B", "A, B and C". */
function listed(names: string[]): string {
  const last = names[names.length - 1] ?? "";
  return names.length < 2 ? last : `${names.slice(0, -1).join(", ")} and ${last}`;
}

/** Asks before workspace `name` is deleted (Preferences). True to delete it. */
export function confirmDelete(root: HTMLElement, name: string): Promise<boolean> {
  return ask(
    root,
    `Delete the workspace "${name}"?`,
    "Lectern forgets it; its folders stay on disk.",
    [
      { label: "Delete", go: true },
      { label: "Cancel", go: false },
    ],
  );
}

/** Numbers the confirms, so each one's title and text have ids of their own. */
let asked = 0;

/**
 * A confirm: `title`, `text`, and a button per answer, the first styled as the main one. The
 * focus starts on the answer that stays; Esc or a click outside stays too.
 */
function ask(
  root: HTMLElement,
  title: string,
  text: string,
  answers: { label: string; go: boolean }[],
): Promise<boolean> {
  const returnFocus = document.activeElement;
  return new Promise((resolve) => {
    const done = (go: boolean): void => {
      backdrop.remove();
      if (!go && returnFocus instanceof HTMLElement && returnFocus.isConnected) {
        returnFocus.focus({ preventScroll: true });
      }
      resolve(go);
    };
    const buttons = answers.map(({ label, go }, i) => {
      const button = h("button", { type: "button", class: i === 0 ? "btn primary" : "btn" }, label);
      button.addEventListener("click", () => {
        done(go);
      });
      return button;
    });
    const id = `lx-ws-confirm-${String(++asked)}`;
    const dialog = h(
      "div",
      {
        class: "about ws-confirm",
        role: "alertdialog",
        "aria-modal": "true",
        "aria-labelledby": `${id}-title`,
        "aria-describedby": `${id}-text`,
      },
      h("h2", { id: `${id}-title` }, title),
      h("p", { id: `${id}-text` }, text),
      h("div", { class: "ws-confirm-actions" }, ...buttons),
    );
    dialog.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        done(false);
      }
    });
    const backdrop = h("div", { class: "prefs-backdrop ws-confirm-backdrop" }, dialog);
    backdrop.addEventListener("pointerdown", (e) => {
      if (e.target === backdrop) done(false);
    });
    root.append(backdrop);
    buttons[answers.findIndex((a) => !a.go)]?.focus();
  });
}
