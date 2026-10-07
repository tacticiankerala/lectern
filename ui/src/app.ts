// The app: its state and settings, startup, and showing documents in the layout. Navigation
// (navigation.ts), the library (library-controller.ts) and what the user asks for (actions.ts)
// are its parts.
//
// Contracts with the Rust side:
// - The `open-request` and `doc-changed` listeners are registered before `startup`: Rust holds
//   second launches until then and sends them as events. One arriving before startup finishes is
//   applied after the first document.
// - Navigations are numbered, refreshes apart from them (navigation.ts).
// - `startupNotice` shows once, as a toast.
// - The library sidebar renders after the first paint, so it never holds it up. Quick open,
//   Preferences, the menus, find in page, full-text search, update checks, About, the breadcrumb
//   chooser and the workspaces' lists and prompts are separate modules, loaded on first use. The
//   automatic update check runs 5 s after the first paint, once per process: in the window that
//   started first (`primary`), again if it turned to another workspace before the check ran; a
//   manual one runs in any.
// - The review comments module loads after the first paint while the feature is on, and goes when
//   it's switched off; while it holds comment text that isn't saved (another window switched the
//   feature off), it stays, hidden, until that text is saved or let go. `#lx-app.no-comments`
//   marks every comment surface hidden.
// - The reading position is saved once scrolling stops for a moment and when the document is
//   left; a document opened without an anchor or line goes back to its saved position.
// - The window shows one workspace, or none (a blank window), as workspaces.ts has it. Before the
//   window turns to another workspace, or Lectern quits or restarts for an update, unsaved comment
//   text is confirmed, and the settings and the reading position are saved.
// - Every settings snapshot Rust sends (startup's, `settings-changed`, a command's answer) carries
//   a revision, and one not newer than the last applied is dropped, whichever way it came. The
//   newest applies as a change made here does, under the changes made here whose saves Rust
//   hasn't answered yet: the echo of an older save never undoes a newer change, and a save's
//   answer retires its change, so what Rust sent meanwhile isn't left hidden under it.
// - Rust is told whenever this window starts or stops holding comment text that isn't saved yet,
//   so quitting from another window can ask first, and closing this one asks here first
//   (`close-requested`). Quitting or updating from here asks about this window's own text, then
//   about any other window's. A second Quit while one is asking joins it.
import { Actions } from "./actions";
import type { Backend } from "./backend";
import { renderBreadcrumbs } from "./breadcrumbs";
import type { CommentsController } from "./comments";
import { DocView } from "./doc-view";
import { buildLayout, h, nextPaint, quietly, samePath, type Layout } from "./dom";
import type { Crumb } from "./generated/Crumb";
import type { DocChanged } from "./generated/DocChanged";
import type { DocPayload } from "./generated/DocPayload";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { OpenError } from "./generated/OpenError";
import type { OpenRequest } from "./generated/OpenRequest";
import type { OpenResult } from "./generated/OpenResult";
import type { RecentEntry } from "./generated/RecentEntry";
import type { SavedPosition } from "./generated/SavedPosition";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { SettingsSnapshot } from "./generated/SettingsSnapshot";
import { installKeymap } from "./keymap";
import { LibraryController, NARROW } from "./library-controller";
import { Navigation } from "./navigation";
import { Outline } from "./outline";
import { trackProgress } from "./progress";
import { renderProperties } from "./properties";
import { ReadingPanel } from "./reading-panel";
import { flowAnchor, keepFlow, type FlowAnchor } from "./reflow";
import { RightPanel } from "./right-panel";
import { applySettings, loadFonts, loadRememberedFonts } from "./themes";
import { Toasts } from "./toast";
import { renderError, renderWelcome } from "./welcome";
import type { Leaving } from "./workspace-menu";
import { Workspaces } from "./workspaces";

/** The settings defaults, as Rust has them; shown until `startup` answers. */
export const DEFAULT_SETTINGS: Settings = {
  themeMode: "system",
  lightTheme: "paper",
  darkTheme: "graphite",
  bodyFont: "Segoe UI Variable Text",
  codeFont: "JetBrains Mono",
  fontSize: 18,
  lineHeight: 1.65,
  measure: 100,
  codeWrap: false,
  libraryVisible: true,
  outlineVisible: true,
  libraryWidth: 280,
  outlineWidth: 240,
  libraryRoots: [],
  pathMappings: [],
  editor: { mode: "auto" },
  autoUpdate: true,
  showStatusBadges: true,
  sidebarFontSize: 13,
  reviewComments: true,
  commentsVisible: true,
};

/** How long a settings change waits for more before it is saved, when asked to. */
const SAVE_DEBOUNCE_MS = 150;
/** How long the first paint waits for the selected bundled fonts. */
const FONT_WAIT_MS = 150;
/** As many recent files as Rust keeps. */
const MAX_RECENT = 20;
/** How long scrolling must stop before the reading position is saved. */
const POSITION_SAVE_MS = 400;
/** How long after the first paint the automatic update check waits. */
const UPDATE_CHECK_DELAY_MS = 5000;
/** Settings that move the text, so the reading position is kept across them. */
const REFLOWING: (keyof Settings)[] = [
  "fontSize",
  "lineHeight",
  "measure",
  "bodyFont",
  "codeFont",
  "codeWrap",
];

export interface AppState {
  settings: Settings;
  library: LibraryPayload;
  doc: DocPayload | null;
  error: OpenError | null;
  recent: RecentEntry[];
  portable: boolean;
  version: string;
  /**
   * When the file on screen last changed on disk while it was open (live reload), for the
   * properties strip's note. Null once another document, or none, is shown.
   */
  updated: number | null;
}

export interface OpenOptions {
  /** A heading to scroll to: tried as an exact id, then as `slug`. */
  anchor?: string;
  slug?: string;
  /** A source line to scroll to. */
  line?: number;
  /** A captured position to scroll back to (history, live reload). */
  position?: SavedPosition;
  /** Record the document on screen, with its position, on the history once this lands. */
  push?: boolean;
}

export type Change = "doc" | "settings" | "library" | "workspaces";

/** A settings change sent to Rust and not yet answered. */
interface Sending {
  patch: SettingsPatch;
  done: Promise<void>;
}

export class App {
  readonly state: AppState = {
    settings: DEFAULT_SETTINGS,
    library: { roots: [] },
    doc: null,
    error: null,
    recent: [],
    portable: false,
    version: "",
    updated: null,
  };
  readonly layout: Layout;
  readonly view: DocView;
  /** Opens full-text search, for `query` when given, else for the last query. */
  openSearch = (query?: string): void => {
    void this.actions.showSearch(query);
  };
  /** Opens the find bar, searching for `prefill` when given. */
  openFind = (prefill?: string): void => {
    void this.actions.showFind(prefill);
  };
  /** The reading panel ("Aa"). */
  readonly panel: ReadingPanel;
  readonly nav: Navigation;
  readonly library: LibraryController;
  readonly actions: Actions;
  /** The right panel: its Outline and Comments tabs. */
  readonly rightPanel: RightPanel;
  /** This window's workspace, the chip that names it, and the others. */
  readonly workspaces: Workspaces;
  /** Reloads the page, once the window has turned to another workspace; tests replace it. */
  reloadWindow = (): void => {
    location.reload();
  };
  private readonly toasts: Toasts;
  private readonly outline: Outline;
  /** The OS colour scheme, which `system` theme mode follows. */
  private readonly darkQuery: MediaQueryList | null =
    typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
  /** Settings changes not yet saved, and the timer that saves them. */
  private unsaved: SettingsPatch = {};
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  /** Settings changes sent to Rust and not yet answered, oldest first: laid over what Rust sent. */
  private sending: Sending[] = [];
  /** A `settings-changed` arrived before startup answered. */
  private changedEarly = false;
  /** The latest settings Rust sent, and their revision; an older snapshot is dropped. */
  private fromRust: Settings | null = null;
  private rev = 0;
  /** What Rust was last told about this window's unsaved comment text. */
  private unsavedTold = false;
  /** The Quit under way, which another joins. */
  private quitting: Promise<void> | null = null;
  /** The close being asked about (`close-requested`), which another joins. */
  private closing: Promise<void> | null = null;
  private readonly updateProgress: () => void;
  private readonly listeners = new Map<Change, Set<() => void>>();
  /** The native window title last set, and the document's title it names (null for none). */
  private title = "";
  private docTitle: string | null = null;
  private started = false;
  /** The latest `open-request` that arrived before startup finished. */
  private queued: OpenRequest | null = null;
  /** Set once the first paint is done: modules loaded after it may load. */
  private painted = false;
  /**
   * The review comments, while the feature is on and their module has loaded, or while they hold
   * comment text that isn't saved after the feature was switched off (`commentsKept`).
   */
  private comments: CommentsController | null = null;
  /** The comments are kept, hidden, for their unsaved text while the feature is off. */
  private commentsKept = false;
  private commentsLoading: Promise<void> | null = null;
  /** Saves the reading position once scrolling has stopped for a moment. */
  private positionTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    readonly backend: Backend,
    root: HTMLElement,
  ) {
    this.layout = buildLayout(root);
    this.toasts = new Toasts(this.layout.toasts);
    this.view = new DocView(this.layout.doc, this);
    this.rightPanel = new RightPanel(this.layout.outline);
    this.outline = new Outline(this.rightPanel.outlinePane, this.layout.docPane, (id) => {
      this.view.scrollToAnchor(id);
    });
    this.updateProgress = trackProgress(this.layout.docPane, this.layout.progress);
    this.layout.docPane.addEventListener(
      "scroll",
      () => {
        this.schedulePositionSave();
      },
      { passive: true },
    );

    const readingButton = h(
      "button",
      {
        type: "button",
        id: "lx-reading-btn",
        class: "icon-btn",
        title: "Reading settings",
        "aria-label": "Reading settings",
        "aria-haspopup": "dialog",
        "aria-expanded": "false",
      },
      "Aa",
    );
    this.layout.headerActions.append(readingButton);
    this.panel = new ReadingPanel(readingButton, this.layout.overlayRoot, {
      settings: () => this.state.settings,
      systemDark: () => this.systemDark(),
      update: (patch, opts) => {
        this.updateSettings(patch, opts);
      },
      listSystemFonts: () => this.backend.listSystemFonts(),
    });
    this.on("settings", () => {
      this.panel.refresh();
      this.syncComments();
    });
    this.darkQuery?.addEventListener("change", () => {
      this.applySettings();
    });

    this.nav = new Navigation(this);
    this.library = new LibraryController(this);
    this.actions = new Actions(this);
    this.workspaces = new Workspaces(this);
    // Whether a crumb opens the chooser, and what the chooser lists, depend on the library's tree.
    this.on("library", () => {
      this.renderCrumbs(this.state.doc?.breadcrumbs ?? []);
    });
    // Once a test replaces the page's app, the old one stops listening.
    installKeymap(root.ownerDocument, (action) => root.isConnected && this.actions.run(action));
    // Comment text starts or stops being unsaved on typing, keys and clicks, or once a save
    // they asked for is done.
    for (const type of ["input", "keydown", "click"]) {
      root.ownerDocument.addEventListener(
        type,
        () => {
          if (root.isConnected) this.checkUnsaved();
        },
        true,
      );
    }
  }

  /** The element the document scrolls in. */
  get scroller(): HTMLElement {
    return this.layout.docPane;
  }

  async start(): Promise<void> {
    this.backend.on<OpenRequest>("open-request", (request) => {
      if (this.started) {
        void this.openRequested(request);
      } else {
        this.queued = request;
      }
    });
    this.backend.on<SettingsSnapshot>("settings-changed", (snapshot) => {
      if (this.started) {
        this.settingsChanged(snapshot);
      } else {
        this.changedEarly = true;
      }
    });
    this.backend.on("workspaces-changed", () => void this.workspaces.refresh());
    this.backend.on("close-requested", () => void this.closeRequested());
    this.backend.on<DocChanged>("doc-changed", (e) => {
      this.nav.changed(e.path);
    });
    this.backend.on<DocChanged>("doc-removed", (e) => {
      this.nav.removed(e.path);
    });
    this.backend.on<LibraryPayload>("library-updated", (library) => {
      this.library.setLibrary(library);
    });

    const navigation = this.nav.beginNavigation();
    let notice: string | null;
    let initial: OpenResult | null = null;
    let primary = false;
    // The fonts selected last time load while the startup payload is on its way.
    const earlyFonts = loadRememberedFonts();
    try {
      const payload = await this.backend.startup();
      this.fromRust = payload.settings;
      this.rev = payload.settingsRev;
      Object.assign(this.state, {
        settings: payload.settings,
        library: payload.library,
        recent: payload.recent,
        version: payload.version,
        portable: payload.portable,
      });
      notice = payload.startupNotice;
      initial = payload.initial;
      primary = payload.primary;
      this.workspaces.begin(payload.workspace, payload.workspace === null, payload.workspaces);
    } catch (e) {
      notice = `Lectern didn't start properly: ${String(e)}`;
      // No list came: it is asked for, as on a change.
      this.workspaces.begin(null, false, []);
      void this.workspaces.refresh();
    }
    // The theme and fonts are in place before the first paint, so nothing flashes.
    this.applySettings();
    const fonts = Promise.all([earlyFonts, loadFonts(this.state.settings)]);
    if (this.nav.endNavigation(navigation)) {
      this.show(initial);
      this.nav.settleDeferred();
    }
    // Showing something sets the title; otherwise it names the workspace alone.
    this.syncTitle();
    // A workspace with no folders yet, new or emptied, offers to add one.
    const ws = this.workspaces.current;
    if (ws && ws.roots.length === 0 && this.state.doc === null && this.state.error === null) {
      this.layout.doc.querySelector<HTMLElement>(".welcome-add-folder")?.focus();
    }
    await Promise.race([fonts, delay(FONT_WAIT_MS)]);
    await nextPaint();
    this.backend.perfMark("first-paint");
    this.painted = true;
    // The sidebar fills in right after, before the window shows.
    this.library.start();
    quietly(this.backend.showWindow());
    // Nothing can be dropped on a window that isn't showing yet.
    this.backend.onDragDrop((paths) => void this.nav.dropped(paths));
    this.library.watchIndex();
    this.syncComments();
    // Off unless the setting is on when the time comes; not for an app a test has replaced. Once
    // per process: in its first window.
    if (primary) {
      setTimeout(() => {
        if (this.layout.app.isConnected && this.state.settings.autoUpdate) {
          quietly(this.actions.checkForUpdates(false));
        }
      }, UPDATE_CHECK_DELAY_MS);
    }
    if (notice !== null) {
      this.toast(notice);
    }
    this.started = true;
    // Whether it came before or after startup read the settings, Rust's are the latest; the
    // answer's revision tells whether an event since has overtaken it.
    if (this.changedEarly) {
      this.backend.getSettings().then(
        (snapshot) => {
          this.settingsChanged(snapshot);
        },
        (e: unknown) => {
          console.warn(e);
        },
      );
    }
    const queued = this.queued;
    this.queued = null;
    if (queued) {
      await this.openRequested(queued);
    }
  }

  /**
   * A second launch's file opens; a folder launched into a blank window asks for a new
   * workspace's name instead.
   */
  private async openRequested(request: OpenRequest): Promise<void> {
    if (request.folder) {
      await this.workspaces.createWithFolder(request.path);
    } else {
      await this.nav.openRequested(request);
    }
  }

  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    await this.nav.open(path, opts);
  }

  /** Asks for a file and opens it, trusting its host: the user chose it. */
  async openFile(): Promise<void> {
    const path = await this.backend.pickFile();
    if (path !== null) {
      await this.openUserPath(path);
    }
  }

  /**
   * Asks for a folder and adds it to the library, opening its README if it has one. A blank
   * window has no library: it asks for a new workspace's name, then makes one holding the folder
   * (navigation.ts).
   */
  async addFolder(): Promise<void> {
    const path = await this.backend.pickFolder();
    if (path !== null) {
      await this.openUserPath(path);
    }
  }

  /** Opens a file or folder the user chose, as a launch would (navigation.ts). */
  async openUserPath(path: string): Promise<void> {
    await this.nav.openUserPath(path);
  }

  toast(message: string): void {
    this.toasts.show(message);
  }

  /** Saves the reading position in `path`; a save waiting for scrolling to stop is dropped. */
  savePosition(path: string, position: SavedPosition): void {
    if (this.positionTimer !== null) {
      clearTimeout(this.positionTimer);
      this.positionTimer = null;
    }
    quietly(this.backend.savePosition(path, position));
  }

  private schedulePositionSave(): void {
    if (this.positionTimer !== null) {
      clearTimeout(this.positionTimer);
    }
    this.positionTimer = setTimeout(() => {
      this.positionTimer = null;
      const doc = this.state.doc;
      if (doc) {
        this.savePosition(doc.path, this.view.captureAnchor());
      }
    }, POSITION_SAVE_MS);
  }

  /**
   * Changes settings: applied at once, keeping the reading position when the text moves, then
   * saved. With `debounce` the save waits until changes stop for a moment (slider drags).
   */
  updateSettings(patch: SettingsPatch, opts: { debounce?: boolean } = {}): void {
    const changes = Object.fromEntries(
      Object.entries(patch).filter(([, value]) => value !== null),
    ) as Partial<Settings>;
    this.applyChanges(changes);
    Object.assign(this.unsaved, changes);
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    if (opts.debounce) {
      this.saveTimer = setTimeout(() => {
        void this.saveSettings();
      }, SAVE_DEBOUNCE_MS);
    } else {
      void this.saveSettings();
    }
  }

  /**
   * Settings Rust sent (`settings-changed`, or a command's answer): applied as a change made here
   * is, without saving them, under the changes made here that Rust hasn't answered yet. A
   * snapshot not newer than the last applied is dropped.
   */
  settingsChanged(snapshot: SettingsSnapshot): void {
    if (this.accept(snapshot)) {
      this.reconcile();
    }
  }

  /** Keeps `snapshot` as the latest from Rust, unless one at least as new is kept already. */
  private accept(snapshot: SettingsSnapshot): boolean {
    if (snapshot.rev <= this.rev) {
      return false;
    }
    this.rev = snapshot.rev;
    this.fromRust = snapshot.settings;
    return true;
  }

  /**
   * The latest settings Rust sent, under the changes made here whose saves it hasn't answered
   * yet, oldest first, then the ones not sent yet.
   */
  private reconcile(): void {
    const from = this.fromRust;
    if (from === null) {
      return;
    }
    const next: Settings = { ...from };
    for (const s of this.sending) {
      Object.assign(next, s.patch);
    }
    Object.assign(next, this.unsaved);
    // The roots are the library's (library-controller.ts).
    next.libraryRoots = this.state.settings.libraryRoots;
    const current = this.state.settings;
    const changes = Object.fromEntries(
      Object.entries(next).filter(
        ([k, v]) => JSON.stringify(v) !== JSON.stringify(current[k as keyof Settings]),
      ),
    ) as Partial<Settings>;
    if (Object.keys(changes).length > 0) {
      this.applyChanges(changes);
    }
  }

  /** Applies changed settings at once, keeping the reading position when the text moves. */
  private applyChanges(changes: Partial<Settings>): void {
    const anchor = REFLOWING.some((key) => key in changes) ? this.flowAnchor() : null;
    this.state.settings = { ...this.state.settings, ...changes };
    this.applySettings();
    if (anchor) {
      keepFlow(this.scroller, anchor);
    }
  }

  /** Sends the changes waiting to be saved; resolves once every save sent has been answered. */
  private async saveSettings(): Promise<void> {
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    const patch = this.unsaved;
    this.unsaved = {};
    if (Object.keys(patch).length > 0) {
      // Listed before it is sent: Rust tells the window its settings before it answers.
      const sent: Sending = { patch, done: Promise.resolve() };
      this.sending.push(sent);
      sent.done = this.backend.setSettings(patch).then(
        (snapshot) => {
          this.answered(sent, snapshot);
        },
        (e: unknown) => {
          this.toast(`Couldn't save the settings: ${String(e)}`);
          this.answered(sent, null);
        },
      );
    }
    await Promise.all(this.sending.map((s) => s.done));
  }

  /**
   * A save was answered, with the window's settings or (failed) nothing: its change covers
   * nothing any more, and whatever Rust sent while it was on its way shows through.
   */
  private answered(sent: Sending, snapshot: SettingsSnapshot | null): void {
    this.sending = this.sending.filter((s) => s !== sent);
    if (snapshot !== null) {
      this.accept(snapshot);
    }
    this.reconcile();
  }

  /**
   * Before the window turns to another workspace (`switch`), or Lectern quits or restarts for an
   * update: comment saves on their way finish, then unsaved comment text is confirmed; once that's
   * agreed, the settings and the reading position are saved. False when the user chose to stay.
   */
  async readyToLeave(action: Leaving): Promise<boolean> {
    if (this.commentsLoading) {
      await this.commentsLoading;
    }
    const comments = this.comments;
    if (comments) {
      await comments.settled();
      if (comments.hasUnsavedText()) {
        const { confirmLeave } = await import("./workspace-menu.js");
        if (!(await confirmLeave(this.layout.overlayRoot, action))) {
          return false;
        }
      }
    }
    const doc = this.state.doc;
    if (this.positionTimer !== null) {
      clearTimeout(this.positionTimer);
      this.positionTimer = null;
    }
    await Promise.all([
      this.saveSettings(),
      doc ? this.backend.savePosition(doc.path, this.view.captureAnchor()).catch(warn) : null,
    ]);
    return true;
  }

  /**
   * Quits Lectern with every window open (Ctrl+Q), once it may (see `readyToLeave`). When another
   * window holds comment text that isn't saved yet, Rust names it, and this asks before quitting
   * all the same. Asked again while it asks, it joins the Quit under way.
   */
  quit(): Promise<void> {
    this.quitting ??= this.quitNow().finally(() => {
      this.quitting = null;
    });
    return this.quitting;
  }

  private async quitNow(): Promise<void> {
    if (!(await this.readyToLeave("quit"))) {
      return;
    }
    try {
      const unsaved = await this.backend.quit(false);
      if (unsaved.length === 0) {
        return;
      }
      const { confirmQuitElsewhere } = await import("./workspace-menu.js");
      if (await confirmQuitElsewhere(this.layout.overlayRoot, unsaved)) {
        await this.backend.quit(true);
      }
    } catch (e) {
      this.toast(String(e));
    }
  }

  /**
   * Installs the update found (the update pill), once Lectern may restart: this window's own
   * unsaved comment text is confirmed and the settings and reading position saved (see
   * `readyToLeave`), then, when another window holds comment text that isn't saved yet, Rust names
   * it, and this asks before updating all the same. A portable copy only opens the Releases page,
   * so nothing is asked.
   */
  async installUpdate(portable: boolean): Promise<void> {
    if (portable) {
      await this.backend.installUpdate(false);
      return;
    }
    if (!(await this.readyToLeave("update"))) {
      return;
    }
    const unsaved = await this.backend.installUpdate(false);
    if (unsaved.length === 0) {
      return;
    }
    const { confirmUpdateElsewhere } = await import("./workspace-menu.js");
    if (await confirmUpdateElsewhere(this.layout.overlayRoot, unsaved)) {
      await this.backend.installUpdate(true);
    }
  }

  /**
   * The window's close button was pressed while it holds comment text that isn't saved, so Rust
   * kept it open: this asks, and closes it once the user lets the text go. Pressed again while it
   * asks, it joins the question.
   */
  private closeRequested(): Promise<void> {
    this.closing ??= this.askToClose().finally(() => {
      this.closing = null;
    });
    return this.closing;
  }

  private async askToClose(): Promise<void> {
    try {
      const { confirmDiscard } = await import("./workspace-menu.js");
      if (await confirmDiscard(this.layout.overlayRoot)) {
        await this.backend.closeWindow();
      }
    } catch (e) {
      this.toast(String(e));
    }
  }

  /**
   * Tells Rust when this window starts or stops holding comment text that isn't saved yet: now,
   * after the event that may have changed it, and again once the comments' saves are done.
   */
  private checkUnsaved(): void {
    setTimeout(() => {
      this.tellUnsaved();
      void this.comments?.settled().then(() => {
        this.tellUnsaved();
      });
    }, 0);
  }

  private tellUnsaved(): void {
    const unsaved = this.comments?.hasUnsavedText() ?? false;
    if (unsaved !== this.unsavedTold) {
      this.unsavedTold = unsaved;
      quietly(this.backend.setUnsaved(unsaved));
    }
    // Comments kept, hidden, for their text after the feature was switched off go once it's saved
    // or let go.
    if (!unsaved && this.comments && !this.state.settings.reviewComments) {
      this.syncComments();
    }
  }

  systemDark(): boolean {
    return this.darkQuery?.matches ?? false;
  }

  /**
   * The block at the top of the pane and how far down it the top falls, to hold across a reflow.
   * Null at the very top, or with no document.
   */
  flowAnchor(): FlowAnchor | null {
    return this.state.doc === null ? null : flowAnchor(this.scroller, this.layout.doc);
  }

  on(change: Change, cb: () => void): () => void {
    const set = this.listeners.get(change) ?? new Set();
    this.listeners.set(change, set);
    set.add(cb);
    return () => {
      set.delete(cb);
    };
  }

  /** Tells the listeners of `on` that something changed. */
  emit(change: Change): void {
    for (const cb of [...(this.listeners.get(change) ?? [])]) {
      cb();
    }
  }

  /** The current navigation's number, for a caller that awaits something before navigating. */
  get navigation(): number {
    return this.nav.latest;
  }

  /** Shows an open's result: the document, why it didn't open, or (for null) the welcome screen. */
  show(result: OpenResult | null, opts: OpenOptions = {}, position?: SavedPosition): void {
    if (result?.status === "ok") {
      this.showDoc(result.doc, opts, position);
    } else {
      this.state.doc = null;
      this.state.updated = null;
      this.state.error = result?.error ?? null;
      this.setTitle(null);
      this.renderCrumbs([]);
      this.layout.properties.hidden = true;
      this.layout.banner.hidden = true;
      this.outline.clear();
      if (result) {
        this.showError(result.error);
      } else {
        this.showWelcome();
      }
      this.scroller.scrollTop = 0;
    }
    this.updateProgress();
    this.emit("doc");
  }

  private showDoc(doc: DocPayload, opts: OpenOptions, position?: SavedPosition): void {
    this.state.doc = doc;
    this.state.error = null;
    this.view.render(doc);
    this.showMeta(doc);
    this.showBanner(doc.lossy);
    this.outline.render(doc.outline, this.layout.doc);
    if (position) {
      this.view.restore(position);
    } else if (opts.anchor !== undefined && this.view.scrollToAnchor(opts.anchor, opts.slug)) {
      // Scrolled to the anchor.
    } else if (opts.line !== undefined && this.view.scrollToLine(opts.line)) {
      // Scrolled to the line.
    } else if (doc.position) {
      // Where the reader left it last time.
      this.view.restore(doc.position);
    } else {
      this.scroller.scrollTop = 0;
    }
  }

  /**
   * What surrounds the body: the title, breadcrumbs and properties (with the task count and when
   * the file last changed while open), and the document's place at the top of the recent files.
   */
  showMeta(doc: DocPayload): void {
    this.state.recent = [
      { path: doc.path, title: doc.title, openedMs: Date.now() },
      ...this.state.recent.filter((r) => !samePath(r.path, doc.path)),
    ].slice(0, MAX_RECENT);
    this.setTitle(doc.title);
    this.renderCrumbs(doc.breadcrumbs);
    renderProperties(this.layout.properties, doc, Date.now(), this.state.updated);
  }

  /** The breadcrumbs, each opening the chooser on its folder; an open chooser follows them. */
  private renderCrumbs(crumbs: Crumb[]): void {
    renderBreadcrumbs(
      this.layout.breadcrumbs,
      crumbs,
      this.state.library.roots,
      (index, anchor) => void this.actions.showChooser(index, anchor, true),
    );
    this.actions.refreshChooser();
  }

  /** The native window title for the document titled `docTitle`, or for none. */
  private setTitle(docTitle: string | null): void {
    this.docTitle = docTitle;
    this.syncTitle();
  }

  /** Sets the native window title as workspaces.ts words it, once it knows the workspaces. */
  syncTitle(): void {
    if (!this.workspaces.ready) {
      return;
    }
    const title = this.workspaces.title(this.docTitle);
    if (title !== this.title) {
      this.title = title;
      quietly(this.backend.setTitle(title));
    }
  }

  /** The welcome screen; a blank window's lists the workspaces to choose from first. */
  private showWelcome(): void {
    renderWelcome(
      this.layout.doc,
      this.state.recent,
      {
        openFile: () => void this.openFile(),
        addFolder: () => void this.addFolder(),
        openRecent: (path) => void this.open(path, { push: true }),
        removeRecent: (path) => void this.removeRecent(path),
      },
      this.workspaces.isBlank ? (slot) => void this.workspaces.fillChooser(slot) : undefined,
    );
  }

  private showError(error: OpenError): void {
    renderError(this.layout.doc, error, {
      retry: () => void this.open(error.path),
      forget: () => void this.removeRecent(error.path),
      openWithDefaultApp: () => void this.openWithDefaultApp(error.path),
      reveal: () => void this.reveal(error.path),
      search: () => void this.actions.showQuickOpen(stem(error.path)),
    });
  }

  /** Drops `path` from the recent files for good, then shows the welcome screen. */
  private async removeRecent(path: string): Promise<void> {
    try {
      this.state.recent = await this.backend.removeRecent(path);
    } catch (e) {
      this.toast(String(e));
      return;
    }
    if (this.state.doc === null) {
      this.show(null);
    }
  }

  /** Hands a file Lectern can't show to Rust, which opens a viewable one with its app. */
  private async openWithDefaultApp(path: string): Promise<void> {
    try {
      const target = { kind: "file", target: path, line: null, anchor: null } as const;
      const result = await this.backend.follow(target);
      if (result.action === "notFound") {
        this.toast(result.message);
      }
    } catch (e) {
      this.toast(String(e));
    }
  }

  async reveal(path: string): Promise<void> {
    try {
      await this.backend.revealInExplorer(path);
    } catch (e) {
      this.toast(String(e));
    }
  }

  private showBanner(lossy: boolean): void {
    const banner = this.layout.banner;
    banner.hidden = !lossy;
    if (!lossy) {
      banner.replaceChildren();
      return;
    }
    const dismiss = h(
      "button",
      { type: "button", class: "banner-dismiss", "aria-label": "Dismiss" },
      "×",
    );
    dismiss.addEventListener("click", () => {
      banner.hidden = true;
    });
    banner.replaceChildren(
      h(
        "div",
        { class: "banner" },
        h("span", {}, "This file isn't valid UTF-8, so some characters were replaced."),
        dismiss,
      ),
    );
  }

  /**
   * Ctrl+Alt+M: a comment on the text selected in the note, else on the block at the top of the
   * view, once the comments module is there.
   */
  async addComment(): Promise<void> {
    if (this.commentsLoading) {
      await this.commentsLoading;
    }
    const comments = this.comments;
    if (comments && !comments.addFromSelection()) {
      comments.addAtTop();
    }
  }

  /** Whether comments show: the feature on and not hidden by the header toggle. */
  private commentsShown(): boolean {
    const s = this.state.settings;
    return s.reviewComments && s.commentsVisible;
  }

  /**
   * Whether the right panel shows: its setting, and in a narrow window, shown on purpose. Focus
   * mode hides it.
   */
  private rightPanelOpen(): boolean {
    if (this.actions.inFocusMode) {
      return false;
    }
    const app = this.layout.app;
    const narrow = typeof matchMedia === "function" && matchMedia(NARROW.outline).matches;
    return (
      !app.classList.contains("no-outline") && (!narrow || app.classList.contains("show-outline"))
    );
  }

  /**
   * Starts the review comments, stops them, or shows or hides them, as the settings have it. Not
   * before the first paint, which their module stays out of.
   */
  private syncComments(): void {
    if (!this.painted) {
      return;
    }
    if (!this.state.settings.reviewComments) {
      // Switched off, maybe in another window: comment text that isn't saved stays, hidden, until
      // it is saved or let go (`tellUnsaved`).
      if (this.comments?.hasUnsavedText()) {
        this.comments.setVisible(false);
        this.commentsKept = true;
        return;
      }
      this.comments?.dispose();
      this.comments = null;
      this.commentsKept = false;
      this.tellUnsaved();
    } else if (this.comments) {
      // Back on after they were kept: what loaded while the feature was off was refused, so the
      // note on screen loads again; its drafts stay.
      if (this.commentsKept) {
        this.commentsKept = false;
        quietly(this.comments.load());
      }
      this.comments.setVisible(this.commentsShown());
    } else {
      this.commentsLoading ??= this.startComments();
    }
  }

  private async startComments(): Promise<void> {
    try {
      const { CommentsController } = await import("./comments.js");
      // Not when switched off while it loaded, nor for an app a test has replaced.
      if (this.state.settings.reviewComments && this.layout.app.isConnected) {
        this.comments = new CommentsController({
          backend: this.backend,
          doc: () => (this.state.doc ? this.layout.doc : null),
          docPath: () => this.state.doc?.path ?? null,
          panel: this.rightPanel,
          scroller: this.scroller,
          // Kept while the feature is off, they say nothing: Rust refuses their loads then.
          toast: (message) => {
            if (this.state.settings.reviewComments) this.toast(message);
          },
          setBadge: (n) => {
            this.actions.setCommentCount(n);
          },
          // A line before every block (frontmatter, blank lines) is the note's top.
          jumpToLine: (line) => {
            if (!this.view.scrollToLine(line)) {
              this.scroller.scrollTop = 0;
            }
          },
          follow: (link) => {
            this.view.follow(link);
          },
          onDoc: (cb) => this.on("doc", cb),
          visible: () => this.commentsShown(),
          panelOpen: () => this.rightPanelOpen(),
          openPanel: () => {
            if (!this.rightPanelOpen()) this.actions.toggleSidebar("outline");
          },
          pulseBadge: () => {
            this.actions.pulseCommentBadge();
          },
          focusMode: () => this.actions.inFocusMode,
          showComments: () => {
            if (!this.state.settings.commentsVisible) {
              this.updateSettings({ commentsVisible: true });
            }
          },
        });
        quietly(this.comments.load());
      }
    } catch (e) {
      console.warn(e);
    } finally {
      this.commentsLoading = null;
    }
  }

  /** Applies the settings: the theme and reading ones (themes.ts), then the layout. */
  private applySettings(): void {
    const s = this.state.settings;
    applySettings(s, this.systemDark(), this.backend);
    const root = document.documentElement;
    root.style.setProperty("--library-width", `${String(s.libraryWidth)}px`);
    root.style.setProperty("--outline-width", `${String(s.outlineWidth)}px`);
    root.style.setProperty("--sidebar-font-size", `${String(s.sidebarFontSize)}px`);
    this.layout.app.classList.toggle("no-library", !s.libraryVisible);
    this.layout.app.classList.toggle("no-outline", !s.outlineVisible);
    this.layout.app.classList.toggle("no-comments", !this.commentsShown());
    this.rightPanel.setCommentsEnabled(this.commentsShown());
    this.emit("settings");
  }
}

/** A path's file name without its extension. */
function stem(path: string): string {
  const name = path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function warn(e: unknown): void {
  console.warn(e);
}
