// This window's workspace (design §1): the chip at the left of the header that names it and opens
// the dropdown of workspaces, the window title's workspace part, `workspaces-changed`, and opening,
// creating, renaming and deleting workspaces. The dropdown, the name field, a blank window's list
// and the prompts are workspace-menu.ts's, loaded on first use.
//
// Contracts:
// - A window shows one workspace, or none: a blank window has no chip, and its welcome screen
//   lists the workspaces to choose from.
// - The title names the workspace only when there is more than one: "‹note› — ‹workspace›", or
//   "‹workspace› — Lectern" with no note. With one it stays "‹note› — Lectern". The first list
//   comes in the startup payload, so the right title shows from the first paint, with no call of
//   its own.
// - The list is fetched again on every `workspaces-changed` (one that came before startup
//   answered, once it has); an older answer is dropped.
// - Opening or creating a workspace here first asks the app whether the window may leave (unsaved
//   comment text is confirmed; the settings and the reading position are saved), and once Rust
//   has turned the window to it, the page stops its listeners (Rust would keep them otherwise) and
//   reloads through the normal startup. A workspace open in another window is asked for in a new
//   window, which brings that window forward and never turns this one, so nothing is asked. While
//   one is being opened or made, asking again joins it.
// - In a blank window a folder (Add folder, a drop, an Explorer launch) asks for a new workspace's
//   name, then makes one holding the folder, here or in a new window. Cancelling drops it. Rust
//   says which paths are folders (`UserOpen.folder`, `OpenRequest.folder`); a file opens as a
//   loose file.
// - Deleting a workspace asks first.
import type { App } from "./app";
import { h } from "./dom";
import type { OpenWhere } from "./generated/OpenWhere";
import type { SettingsSnapshot } from "./generated/SettingsSnapshot";
import type { WorkspaceOutcome } from "./generated/WorkspaceOutcome";
import type { WorkspaceSummary } from "./generated/WorkspaceSummary";
import type { MenuHost, WorkspaceChooser, WorkspaceMenu } from "./workspace-menu";

/** How long a page turning to another workspace waits for its listeners to stop. */
const STOP_LISTENING_MS = 1000;

export class Workspaces {
  /** This window's workspace; null for a blank window. */
  current: WorkspaceSummary | null = null;
  /** Every workspace, in creation order. */
  list: WorkspaceSummary[] = [];
  /** The workspace chip: the header's, while the window shows a workspace. */
  readonly chip: HTMLButtonElement;
  private readonly chipName: HTMLElement;
  private blankWindow = false;
  /** Set while the window turns to another workspace: the page reloads into it. */
  private turning = false;
  /** Set while a folder's new workspace is being named; another folder meanwhile is dropped. */
  private naming = false;
  /** Set at startup (`begin`), which brings the first list. */
  private begun = false;
  /** A `workspaces-changed` came before startup answered: the list is fetched once it has. */
  private changedEarly = false;
  /** Set once the first list is in. */
  private listed = false;
  /** Bumped by every fetch of the list, so only the latest answer is used. */
  private fetches = 0;
  /** The workspace being opened or made, which another ask joins. */
  private going: Promise<void> | null = null;
  /** Loaded on first use. */
  private dropdown: WorkspaceMenu | null = null;
  private chooser: WorkspaceChooser | null = null;

  constructor(private readonly app: App) {
    this.chipName = h("span", { class: "ws-chip-name" });
    this.chip = h(
      "button",
      {
        type: "button",
        id: "lx-workspace-btn",
        class: "ws-chip",
        "aria-haspopup": "dialog",
        "aria-expanded": "false",
      },
      this.chipName,
      h("span", { class: "ws-chip-caret", "aria-hidden": "true" }, "▾"),
    );
    this.chip.hidden = true;
    this.chip.addEventListener("click", () => void this.toggleMenu());
    // Before the breadcrumbs, after back and forward.
    app.layout.breadcrumbs.before(this.chip);
  }

  /** A blank window: it shows no workspace. */
  get isBlank(): boolean {
    return this.blankWindow;
  }

  /** Whether the first list is in, so the title can be set. */
  get ready(): boolean {
    return this.listed;
  }

  /** The dropdown, while it is loaded. */
  get menu(): WorkspaceMenu | null {
    return this.dropdown;
  }

  /**
   * At startup: the window shows `workspace`, or nothing (`blank`), and `list` is every workspace,
   * as the startup payload has them. The title follows once the window shows something.
   */
  begin(workspace: WorkspaceSummary | null, blank: boolean, list: WorkspaceSummary[]): void {
    this.blankWindow = blank;
    this.begun = true;
    this.list = list;
    this.listed = true;
    this.current = blank ? null : (list.find((ws) => ws.current) ?? workspace);
    this.syncChip();
    if (this.changedEarly) {
      void this.refresh();
    }
  }

  /** Fetches the list again: on `workspaces-changed`, and when the dropdown opens. */
  async refresh(): Promise<void> {
    if (!this.begun) {
      this.changedEarly = true;
      return;
    }
    const fetch = ++this.fetches;
    let list: WorkspaceSummary[];
    try {
      list = await this.app.backend.listWorkspaces();
    } catch (e) {
      console.warn(e);
      list = this.listed ? this.list : this.current ? [this.current] : [];
    }
    if (fetch === this.fetches) {
      this.apply(list);
    }
  }

  /** The window title for a document titled `docTitle`, or for none. */
  title(docTitle: string | null): string {
    const ws = this.current;
    if (ws === null || this.list.length <= 1) {
      return docTitle === null ? "Lectern" : `${docTitle} — Lectern`;
    }
    return docTitle === null ? `${ws.name} — Lectern` : `${docTitle} — ${ws.name}`;
  }

  /**
   * Opens workspace `id` here or in a new window (see the module comment). One open in another
   * window is asked for in a new window whichever was chosen: Rust brings that window forward,
   * and should it have closed meanwhile, the workspace opens in a new one. This window, and any
   * draft in it, stays either way.
   */
  async open(id: string, where: OpenWhere): Promise<void> {
    const elsewhere = this.list.some((ws) => ws.id === id && ws.open && !ws.current);
    const place = elsewhere ? "newWindow" : where;
    await this.go(place === "here", () => this.app.backend.openWorkspace(id, place));
  }

  /** Makes a workspace named `name`, holding `folder` when given, and opens it. */
  async create(name: string, where: OpenWhere, folder?: string): Promise<void> {
    await this.go(where === "here", () => this.app.backend.createWorkspace(name, where, folder));
  }

  /** Renames workspace `id`; false when it couldn't, said in a toast. */
  async rename(id: string, name: string): Promise<boolean> {
    try {
      this.take(await this.app.backend.renameWorkspace(id, name));
      return true;
    } catch (e) {
      this.app.toast(String(e));
      return false;
    }
  }

  /** Forgets workspace `id`, never its files, once the user confirms it. */
  async remove(id: string): Promise<void> {
    const name = this.list.find((ws) => ws.id === id)?.name ?? "";
    const { confirmDelete } = await import("./workspace-menu.js");
    if (!(await confirmDelete(this.app.layout.overlayRoot, name))) {
      return;
    }
    try {
      this.take(await this.app.backend.deleteWorkspace(id));
    } catch (e) {
      this.app.toast(String(e));
    }
  }

  /** Gives this window's workspace a theme of its own, or has it follow the shared one again. */
  async setOwnTheme(own: boolean): Promise<void> {
    const ws = this.current;
    if (ws === null) {
      return;
    }
    let settings: SettingsSnapshot;
    try {
      settings = await this.app.backend.setWorkspaceTheme(own);
    } catch (e) {
      this.app.toast(String(e));
      return;
    }
    this.take(this.list.map((w) => (w.id === ws.id ? { ...w, ownTheme: own } : w)));
    this.app.settingsChanged(settings);
    // A fetch on its way was dropped for the list just taken, which may lack what it brought.
    void this.refresh();
  }

  /** Opens a blank window, which lists the workspaces. */
  async newWindow(): Promise<void> {
    try {
      await this.app.backend.newWindow();
    } catch (e) {
      this.app.toast(String(e));
    }
  }

  /**
   * A folder chosen in a blank window: asks for a new workspace's name, then makes one holding the
   * folder. Cancelled, nothing happens; so does another folder while the name is asked for.
   */
  async createWithFolder(folder: string): Promise<void> {
    if (this.naming) {
      return;
    }
    this.naming = true;
    let answer;
    try {
      const { askName } = await import("./workspace-menu.js");
      answer = await askName(this.app.layout.overlayRoot, folder, () =>
        this.app.backend.suggestWorkspaceName(),
      );
    } finally {
      this.naming = false;
    }
    if (answer !== null) {
      await this.create(answer.name, answer.where, folder);
    }
  }

  /** A blank window's welcome screen: the workspaces to choose from go in `slot`. */
  async fillChooser(slot: HTMLElement): Promise<void> {
    const { WorkspaceChooser } = await import("./workspace-menu.js");
    if (slot.isConnected) {
      this.chooser ??= new WorkspaceChooser(this.host());
      this.chooser.mount(slot);
    }
  }

  /** Opens the dropdown under the chip, or closes it. */
  private async toggleMenu(): Promise<void> {
    if (this.dropdown?.isOpen) {
      this.dropdown.close();
      return;
    }
    const { WorkspaceMenu } = await import("./workspace-menu.js");
    if (this.chip.hidden) {
      return;
    }
    this.dropdown ??= new WorkspaceMenu(this.app.layout.overlayRoot, this.host(), (open) => {
      this.chip.setAttribute("aria-expanded", String(open));
    });
    this.app.actions.closeOverlays(this.dropdown);
    this.dropdown.open(this.chip);
    void this.refresh();
  }

  /** What the dropdown and a blank window's list need. */
  private host(): MenuHost {
    return {
      list: () => this.list,
      suggestName: () => this.app.backend.suggestWorkspaceName(),
      open: (id, where) => void this.open(id, where),
      create: (name, where) => void this.create(name, where),
      rename: (id, name) => this.rename(id, name),
    };
  }

  /**
   * Opens a workspace by `call`; when it turns this window to it (`turns`), once the window may
   * leave. An answer of `reload` reloads the page, its listeners stopped first. While one is on
   * its way, another joins it.
   */
  private go(turns: boolean, call: () => Promise<WorkspaceOutcome>): Promise<void> {
    this.going ??= this.goNow(turns, call).finally(() => {
      this.going = null;
    });
    return this.going;
  }

  private async goNow(turns: boolean, call: () => Promise<WorkspaceOutcome>): Promise<void> {
    if (turns && !(await this.app.readyToLeave("switch"))) {
      return;
    }
    // Until the page reloads, it goes on naming the workspace it shows.
    this.turning = turns;
    try {
      if ((await call()) === "reload") {
        // Bounded: the reload must happen even if Rust is slow to answer.
        await Promise.race([
          this.app.backend.stopListening().catch((e: unknown) => {
            console.warn(e);
          }),
          new Promise((resolve) => setTimeout(resolve, STOP_LISTENING_MS)),
        ]);
        this.app.reloadWindow();
        return;
      }
    } catch (e) {
      this.app.toast(String(e));
    }
    if (this.turning) {
      this.turning = false;
      void this.refresh();
    }
  }

  /** Takes a list a command answered with, newer than any fetch still on its way. */
  private take(list: WorkspaceSummary[]): void {
    this.fetches++;
    this.apply(list);
  }

  /** Takes a list: the window's workspace, the chip, the title and whatever lists them follow. */
  private apply(list: WorkspaceSummary[]): void {
    if (this.turning) {
      return;
    }
    this.list = list;
    this.listed = true;
    if (!this.blankWindow) {
      const id = this.current?.id;
      this.current =
        list.find((ws) => ws.current) ?? list.find((ws) => ws.id === id) ?? this.current;
    }
    this.syncChip();
    this.dropdown?.update();
    this.chooser?.update();
    this.app.syncTitle();
    this.app.emit("workspaces");
  }

  /** The chip names the window's workspace, and is there only while it shows one. */
  private syncChip(): void {
    const ws = this.current;
    this.chip.hidden = ws === null;
    if (ws === null) {
      this.dropdown?.close();
      return;
    }
    this.chipName.textContent = ws.name;
    // The name may be cut short with an ellipsis.
    this.chip.title = ws.name;
    this.chip.setAttribute("aria-label", `Workspace: ${ws.name}`);
  }
}
