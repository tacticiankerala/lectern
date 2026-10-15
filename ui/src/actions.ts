// What the user asks for: shortcuts, the header's buttons and menus (the recent files menu among
// them, recent-files.ts), the sidebar's and links' context menus, copying paths in Windows or WSL
// form, quick open, Preferences, find in page, full-text search, update checks, About and the
// breadcrumb chooser (all loaded on first use), focus mode, the sidebars' visibility and widths,
// showing or hiding the review comments, a new window and quitting.
import type { About } from "./about";
import type { App } from "./app";
import {
  COPIED,
  NO_FORM,
  TOAST_PATH_CHARS,
  inNoteOrComment,
  linkPath,
  middleTruncate,
  shortcutPath,
  type PathForm,
  type PathToCopy,
} from "./copy-path";
import type { CrumbChooser } from "./crumb-chooser";
import { h, quietly, samePath } from "./dom";
import type { FindBar } from "./find";
import type { DocPayload } from "./generated/DocPayload";
import type { PathForms } from "./generated/PathForms";
import { iconButton, ICONS } from "./icons";
import type { Action } from "./keymap";
import { NARROW } from "./library-controller";
import type { Menu, MenuEntry } from "./menu";
import { blockAt } from "./position";
import type { Preferences } from "./preferences";
import type { QuickOpen } from "./quick-open";
import { recentChoices, type RecentChoice } from "./recent-files";
import { FlowHold, keepFlow } from "./reflow";
import type { SearchPanel } from "./search-panel";
import { bumpFontSize, bumpSidebarFontSize, toggleThemeMode } from "./themes";
import type { Updater } from "./update";

/** The sidebars' widths, in pixels, as the resizers allow them. */
const WIDTHS = {
  library: { min: 180, max: 560 },
  outline: { min: 160, max: 480 },
} as const;

/** How long the library's column takes to open or close (chrome.css), with some to spare. */
const SLIDE_MS = 200;
/** How long the comments badge pulses when Claude writes while the panel is closed. */
const PULSE_MS = 2000;
/** Where find is, for a PDF in its frame: the viewer has its own. */
const PDF_FIND = "Click into the PDF and press Ctrl+F to search it.";

export class Actions {
  /** Loaded on first use. */
  quickOpen: QuickOpen | null = null;
  private preferences: Preferences | null = null;
  private search: SearchPanel | null = null;
  private find: FindBar | null = null;
  private updater: Updater | null = null;
  private about: About | null = null;
  private chooser: CrumbChooser | null = null;
  /** The menu open, if any. */
  private menu: Menu | null = null;
  private readonly moreButton: HTMLButtonElement;
  /** The clock: the recent files menu, off while it would list none. */
  private readonly recentButton: HTMLButtonElement;
  private readonly libraryButton: HTMLButtonElement;
  /** Shows or hides every comment surface; its badge counts the open comments. */
  private readonly commentsButton: HTMLButtonElement;
  private readonly commentsBadge: HTMLElement;
  private pulseTimer?: ReturnType<typeof setTimeout>;
  /** Below this width the library hides unless shown on purpose. */
  private readonly narrowLibrary =
    typeof matchMedia === "function" ? matchMedia(NARROW.library) : null;
  /** Ends the library column's slide. */
  private slideTimer?: ReturnType<typeof setTimeout>;
  private focusMode = false;
  private readonly hold: FlowHold;

  constructor(private readonly app: App) {
    this.hold = new FlowHold(app.scroller);
    this.libraryButton = iconButton("lx-library-btn", "Library", ICONS.library);
    this.libraryButton.addEventListener("click", () => {
      this.toggleSidebar("library");
    });
    app.layout.header.prepend(this.libraryButton);
    // Whether the library shows follows the layout's classes (the setting, and a narrow window's
    // `show-library`) and the window's width.
    const sync = (): void => {
      this.syncLibraryButton();
    };
    new MutationObserver(sync).observe(app.layout.app, { attributeFilter: ["class"] });
    this.narrowLibrary?.addEventListener("change", sync);
    sync();
    this.commentsButton = iconButton("lx-comments-btn", "Comments", ICONS.comments, "Ctrl+Shift+M");
    this.commentsBadge = h("span", { class: "count-badge" });
    this.commentsBadge.hidden = true;
    this.commentsButton.append(this.commentsBadge);
    this.commentsButton.addEventListener("click", () => {
      this.run("toggle-comments");
    });
    // Next to Aa, which the app adds first.
    const reading = app.layout.headerActions.querySelector("#lx-reading-btn");
    if (reading) reading.after(this.commentsButton);
    else app.layout.headerActions.append(this.commentsButton);
    this.syncCommentsButton();
    app.on("settings", () => {
      this.syncCommentsButton();
    });
    const outlineButton = iconButton("lx-outline-btn", "Outline", ICONS.outline);
    outlineButton.addEventListener("click", () => {
      this.toggleSidebar("outline");
    });
    this.recentButton = iconButton("lx-recent-btn", "Recent files", ICONS.recent, "Ctrl+H");
    this.recentButton.setAttribute("aria-haspopup", "menu");
    this.recentButton.addEventListener("click", () => {
      void this.toggleRecentMenu();
    });
    this.syncRecentButton();
    app.on("doc", () => {
      this.syncRecentButton();
    });
    this.moreButton = iconButton("lx-more-btn", "More", ICONS.more);
    this.moreButton.setAttribute("aria-haspopup", "menu");
    this.moreButton.addEventListener("click", () => {
      void this.toggleMoreMenu();
    });
    app.layout.headerActions.append(outlineButton, this.recentButton, this.moreButton);
    app.on("library", () => {
      this.preferences?.refresh();
    });
    app.on("settings", () => {
      this.preferences?.syncSettings();
    });
    app.on("workspaces", () => {
      this.preferences?.syncWorkspaces();
    });
    // The find bar searches Lectern's own document or an HTML page's text, never a PDF.
    app.on("doc", () => {
      if (this.onPdf()) this.find?.close();
    });
    app.layout.app.addEventListener("contextmenu", (e) => {
      this.linkMenu(e);
    });
    for (const handle of app.layout.app.querySelectorAll<HTMLElement>(".resizer")) {
      const side = handle.dataset.for;
      if (side === "library" || side === "outline") {
        this.installResizer(handle, side);
      }
    }
  }

  /** Whether focus mode (F11) is on: no header, sidebars or progress bar. */
  get inFocusMode(): boolean {
    return this.focusMode;
  }

  /** Runs a shortcut; false when it had nothing to do. */
  run(action: Action): boolean {
    const s = this.app.state.settings;
    switch (action) {
      case "open-file":
        void this.app.openFile();
        return true;
      case "add-folder":
        void this.app.addFolder();
        return true;
      case "quick-open":
        void this.showQuickOpen();
        return true;
      case "recent-files":
        if (this.recentChoices().length === 0) return false;
        void this.toggleRecentMenu();
        return true;
      case "search":
        this.app.openSearch();
        return true;
      case "find":
        this.app.openFind();
        return true;
      case "find-next":
      case "find-prev":
        // F3 is also WebView2's own find shortcut, so it is always handled.
        if (this.onPdf()) {
          this.app.toast(PDF_FIND);
        } else if (this.find?.isOpen) {
          if (action === "find-next") this.find.next();
          else this.find.prev();
        } else {
          this.app.openFind();
        }
        return true;
      case "back":
      case "forward":
        void this.app.nav.travel(action);
        return true;
      case "toggle-library":
        this.toggleSidebar("library");
        return true;
      case "toggle-outline":
        this.toggleSidebar("outline");
        return true;
      case "open-editor":
        return this.openInEditor();
      case "copy-windows-path":
      case "copy-wsl-path": {
        const target = shortcutPath(this.app.layout.doc, this.app.nav.currentPath());
        if (target === null) return false;
        void this.copyPath(target, action === "copy-wsl-path" ? "wsl" : "windows");
        return true;
      }
      case "reload":
        // Handled either way: WebView2 would reload the whole page.
        this.reload();
        return true;
      case "preferences":
        void this.showPreferences();
        return true;
      case "font-up":
        this.app.updateSettings(bumpFontSize(s, 1), { debounce: true });
        return true;
      case "font-down":
        this.app.updateSettings(bumpFontSize(s, -1), { debounce: true });
        return true;
      case "font-reset":
        this.app.updateSettings(bumpFontSize(s, 0));
        return true;
      case "sidebar-font-up":
        this.app.updateSettings(bumpSidebarFontSize(s, 1), { debounce: true });
        return true;
      case "sidebar-font-down":
        this.app.updateSettings(bumpSidebarFontSize(s, -1), { debounce: true });
        return true;
      case "sidebar-font-reset":
        this.app.updateSettings(bumpSidebarFontSize(s, 0));
        return true;
      case "breadcrumbs":
        return this.chooseFromLastCrumb();
      case "toggle-theme":
        this.app.updateSettings(toggleThemeMode(s, this.app.systemDark()));
        return true;
      case "toggle-comments":
        if (!s.reviewComments) return false;
        this.app.updateSettings({ commentsVisible: !s.commentsVisible });
        return true;
      case "add-comment":
        // Out of focus mode first; the comments show hidden ones themselves.
        if (!s.reviewComments) return false;
        if (this.focusMode) {
          this.setFocusMode(false);
          // Adding a comment moves the reading position on purpose: the old one isn't held.
          this.hold.stop();
        }
        void this.app.addComment();
        return true;
      case "focus":
        this.setFocusMode(!this.focusMode);
        return true;
      case "new-window":
        void this.app.workspaces.newWindow();
        return true;
      case "quit":
        void this.app.quit();
        return true;
      case "escape":
        for (const overlay of [
          this.menu,
          this.app.workspaces.menu,
          this.chooser,
          this.quickOpen,
          this.search,
          this.preferences,
          this.about,
          this.app.panel,
          this.find,
        ]) {
          if (overlay?.isOpen) {
            overlay.close();
            return true;
          }
        }
        if (this.focusMode) {
          this.setFocusMode(false);
          return true;
        }
        return false;
    }
  }

  /**
   * Focus mode: no header, sidebars or progress bar, full screen, at the same reading spot, held
   * through the native resize.
   */
  private setFocusMode(on: boolean): void {
    this.focusMode = on;
    this.app.panel.close();
    const anchor = this.app.flowAnchor();
    document.body.classList.toggle("focus", on);
    if (anchor) {
      keepFlow(this.app.scroller, anchor);
      this.hold.hold(anchor);
    }
    quietly(this.app.backend.setFullscreen(on));
  }

  /** Quick open, with `query` typed in when given. */
  async showQuickOpen(query?: string): Promise<void> {
    this.menu?.close(false);
    const { QuickOpen } = await import("./quick-open.js");
    this.quickOpen ??= new QuickOpen(this.app.layout.overlayRoot, {
      candidates: () => this.app.library.candidates(),
      recent: () => this.app.state.recent,
      open: (path) => void this.app.open(path, { push: true }),
    });
    this.closeOverlays(this.quickOpen);
    this.quickOpen.open(query);
  }

  /** Full-text search, for `query` when given, else for the last query. */
  async showSearch(query?: string): Promise<void> {
    this.menu?.close(false);
    const { SearchPanel } = await import("./search-panel.js");
    this.search ??= new SearchPanel(this.app.layout.overlayRoot, {
      search: (q) => this.app.backend.search(q),
      open: (path, line, q) => void this.openResult(path, line, q),
    });
    this.closeOverlays(this.search);
    this.search.open(query);
  }

  /**
   * Opens a search result at its line, then finds the query in it once it is on screen, starting
   * from that line's block: the hit may be in Markdown that doesn't render (a link's destination).
   */
  private async openResult(path: string, line: number | null, query: string): Promise<void> {
    await this.app.open(path, line === null ? { push: true } : { line, push: true });
    const doc = this.app.state.doc;
    if (doc && samePath(doc.path, path)) {
      const near = line === null ? null : blockAt(this.app.layout.doc, line);
      await this.showFind(query, near ?? undefined);
    }
  }

  /**
   * The find bar, searching for `prefill` when given, from `near` (see `FindBar.open`): over the
   * document, or an HTML page's text. With a PDF, which the viewer searches, it says where find
   * is instead. A navigation while its module loads drops it, as a followed link is dropped: it
   * was asked for in the document left.
   */
  async showFind(prefill?: string, near?: Element): Promise<void> {
    this.menu?.close(false);
    if (this.onPdf()) {
      this.app.toast(PDF_FIND);
      return;
    }
    const navigation = this.app.navigation;
    const { FindBar } = await import("./find.js");
    if (navigation !== this.app.navigation || this.onPdf()) {
      return;
    }
    this.find ??= new FindBar(this.app.layout.docPane, {
      doc: () => (this.app.state.doc ? this.app.layout.doc : null),
      page: () => this.app.view.page,
      scroller: this.app.scroller,
      placeAt: (el, at) => {
        this.app.view.placeAt(el, at);
      },
      // Again when the document renders, and when an HTML page's text changes.
      onRender: (cb) => {
        this.app.on("doc", cb);
        this.app.on("page", cb);
      },
    });
    this.closeOverlays(this.find);
    this.find.open(prefill, near);
  }

  private async showPreferences(): Promise<void> {
    this.menu?.close(false);
    const { Preferences } = await import("./preferences.js");
    const workspaces = this.app.workspaces;
    this.preferences ??= new Preferences(this.app.layout.overlayRoot, {
      settings: () => this.app.state.settings,
      library: () => this.app.state.library,
      version: () => this.app.state.version,
      portable: () => this.app.state.portable,
      update: (patch) => {
        this.app.updateSettings(patch);
      },
      addFolder: () => void this.app.addFolder(),
      removeRoot: (path) => void this.app.library.removeRoot(path),
      retryRoot: (path) => void this.app.library.retryRoot(path),
      workspace: () => workspaces.current,
      workspaces: () => workspaces.list,
      renameWorkspace: (id, name) => workspaces.rename(id, name),
      deleteWorkspace: (id) => void workspaces.remove(id),
      setOwnTheme: (own) => void workspaces.setOwnTheme(own),
    });
    this.closeOverlays(this.preferences);
    this.preferences.open();
  }

  /**
   * Checks for an update: on request (`manual`), always, saying what it found; else at most once
   * a day and quietly. An update shows as a pill in the header.
   */
  async checkForUpdates(manual: boolean): Promise<void> {
    const { Updater } = await import("./update.js");
    this.updater ??= new Updater(this.app.layout.headerActions, {
      checkUpdate: (automatic) => this.app.backend.checkUpdate(automatic),
      installUpdate: (portable) => this.app.installUpdate(portable),
      toast: (message) => {
        this.app.toast(message);
      },
    });
    await (manual ? this.updater.checkNow() : this.updater.checkAutomatically());
  }

  private async showAbout(): Promise<void> {
    this.menu?.close(false);
    const { About } = await import("./about.js");
    this.about ??= new About(this.app.layout.overlayRoot, {
      version: () => this.app.state.version,
      portable: () => this.app.state.portable,
      openLink: (url) => void this.openLink(url),
    });
    this.closeOverlays(this.about);
    this.about.open();
  }

  /**
   * The breadcrumb chooser on crumb `index`, hanging from its button `anchor`. With `toggle` (a
   * click), on the crumb it already hangs from, it closes instead.
   */
  async showChooser(index: number, anchor: HTMLElement, toggle = false): Promise<void> {
    if (toggle && this.chooser?.anchor === anchor) {
      this.chooser.close();
      return;
    }
    const { CrumbChooser, crumbFolder } = await import("./crumb-chooser.js");
    const doc = this.app.state.doc;
    // The crumbs may have been drawn again while the module loaded.
    const target =
      anchor.isConnected &&
      doc &&
      crumbFolder(this.app.state.library.roots, doc.breadcrumbs, index);
    if (!target) {
      return;
    }
    this.chooser ??= new CrumbChooser(this.app.layout.overlayRoot, {
      open: (path) => void this.app.open(path, { push: true }),
      badges: () => this.app.state.settings.showStatusBadges,
    });
    this.closeOverlays(this.chooser);
    this.chooser.open(anchor, target);
  }

  /** After the crumbs are drawn again: an open chooser follows them (crumb-chooser.ts). */
  refreshChooser(): void {
    this.chooser?.follow(this.app.layout.breadcrumbs, this.app.state.library.roots);
  }

  /** Ctrl+Shift+.: the chooser on the file's own crumb; false when it has none. */
  private chooseFromLastCrumb(): boolean {
    const crumb = this.app.layout.breadcrumbs.querySelector<HTMLElement>("button.crumb.current");
    if (crumb?.dataset.index === undefined) {
      return false;
    }
    void this.showChooser(Number(crumb.dataset.index), crumb);
    return true;
  }

  /**
   * Before `opening` (or a menu, given none) opens: closes the menu and every other overlay (the
   * workspaces' dropdown, the breadcrumb chooser, quick open, search, Preferences, About, the
   * reading panel), so only one is ever on top. The find bar stays: it sits above the document
   * rather than over it, as before.
   */
  closeOverlays(opening?: { close(): void }): void {
    this.menu?.close(false);
    for (const overlay of [
      this.app.workspaces.menu,
      this.chooser,
      this.quickOpen,
      this.search,
      this.preferences,
      this.about,
      this.app.panel,
    ]) {
      if (overlay !== opening) {
        overlay?.close();
      }
    }
  }

  /** Opens a web page in the browser, through Rust, which opens only web and mail links. */
  private async openLink(url: string): Promise<void> {
    try {
      const result = await this.app.backend.follow({
        kind: "external",
        target: url,
        line: null,
        anchor: null,
      });
      if (result.action === "notFound") {
        this.app.toast(result.message);
      }
    } catch (e) {
      this.app.toast(String(e));
    }
  }

  /**
   * Opens a menu, replacing any other; it forgets itself once closed, and the button it hangs
   * from, if any, is no longer expanded.
   */
  private async openMenu(
    entries: MenuEntry[],
    at: HTMLElement | { x: number; y: number },
  ): Promise<void> {
    const { Menu } = await import("./menu.js");
    this.menu?.close(false);
    this.app.panel.close();
    const menu: Menu = new Menu(this.app.layout.overlayRoot, entries, at, () => {
      if (this.menu === menu) {
        this.menu = null;
        menu.anchor?.setAttribute("aria-expanded", "false");
      }
    });
    this.menu = menu;
  }

  /**
   * The recent files menu, from the clock or Ctrl+H, once every other overlay is closed; again, it
   * closes.
   */
  private async toggleRecentMenu(): Promise<void> {
    if (this.menu?.isOpen && this.menu.anchor === this.recentButton) {
      this.menu.close();
      return;
    }
    this.closeOverlays();
    await this.openMenu(this.recentEntries(), this.recentButton);
    this.recentButton.setAttribute("aria-expanded", "true");
  }

  /** The recent files the menu lists: every one but the file on screen, up to the setting. */
  private recentChoices(): RecentChoice[] {
    const { recent, library, settings } = this.app.state;
    return recentChoices(recent, this.app.nav.currentPath(), library.roots, settings.recentInMenu);
  }

  /** The recent files menu's entries: the files, each with its ×, then quick open. */
  private recentEntries(): MenuEntry[] {
    const choices = this.recentChoices();
    const files: MenuEntry[] = choices.map((choice) => ({
      label: choice.title,
      detail: choice.folder,
      title: `${choice.path}\nDelete removes it`,
      digit: choice.key ?? "",
      run: () => void this.app.open(choice.path, { push: true }),
      remove: {
        label: `Remove ${choice.title} from recent`,
        title: "Remove from recent",
        run: () => this.removeRecent(choice.path),
      },
    }));
    return [
      ...(files.length > 0 ? files : [{ note: "No other recent files" }]),
      { label: "All files…", keys: "Ctrl+P", footer: true, run: () => void this.showQuickOpen() },
    ];
  }

  /** Drops `path` from the recent files for good; answers the recent files menu's entries left. */
  private async removeRecent(path: string): Promise<MenuEntry[]> {
    await this.app.removeRecent(path);
    this.syncRecentButton();
    return this.recentEntries();
  }

  /** The clock is off while the menu would list no file. */
  private syncRecentButton(): void {
    this.recentButton.disabled = this.recentChoices().length === 0;
  }

  private async toggleMoreMenu(): Promise<void> {
    if (this.menu?.isOpen) {
      this.menu.close();
      return;
    }
    const doc = this.app.state.doc;
    const path = this.app.nav.currentPath();
    // A text file, a page or a PDF can also go to its default app; a note would come back here.
    const defaultApp: MenuEntry[] =
      doc && doc.kind !== "markdown"
        ? [
            {
              label: "Open in default app",
              run: () => void this.app.openWithDefaultApp(doc.path),
            },
          ]
        : [];
    await this.openMenu(
      [
        {
          label: "Open in editor",
          keys: "Ctrl+E",
          disabled: !editable(doc),
          run: () => this.openInEditor(),
        },
        ...defaultApp,
        {
          label: "Reveal in Explorer",
          disabled: path === null,
          run: () => {
            if (path !== null) void this.app.reveal(path);
          },
        },
        {
          label: "Copy Windows path",
          keys: "Ctrl+Shift+C",
          disabled: path === null,
          run: () => {
            if (path !== null) void this.copyPath({ path, literal: true }, "windows");
          },
        },
        {
          label: "Copy WSL path",
          keys: "Ctrl+Shift+L",
          disabled: path === null,
          run: () => {
            if (path !== null) void this.copyPath({ path, literal: true }, "wsl");
          },
        },
        {
          label: "Reload",
          keys: "F5",
          disabled: path === null,
          run: () => {
            this.reload();
          },
        },
        "separator",
        {
          label: "New window",
          keys: "Ctrl+N",
          run: () => void this.app.workspaces.newWindow(),
        },
        { label: "Preferences", keys: "Ctrl+,", run: () => void this.showPreferences() },
        {
          label: "Check for updates",
          run: () => {
            quietly(this.checkForUpdates(true));
          },
        },
        { label: "About Lectern", run: () => void this.showAbout() },
        "separator",
        { label: "Quit Lectern", keys: "Ctrl+Q", run: () => void this.app.quit() },
      ],
      this.moreButton,
    );
    this.moreButton.setAttribute("aria-expanded", "true");
  }

  /** The sidebar's menu for a row: reveal, copy, and on a root's own row, remove it. */
  async contextMenu(e: MouseEvent, path: string, isRoot: boolean): Promise<void> {
    const entries: MenuEntry[] = [
      { label: "Reveal in Explorer", run: () => void this.app.reveal(path) },
      ...this.copyItems({ path, literal: true }),
    ];
    if (isRoot) {
      entries.push("separator", {
        label: "Remove from library",
        run: () => void this.app.library.removeRoot(path),
      });
    }
    await this.openMenu(entries, { x: e.clientX, y: e.clientY });
  }

  /**
   * A link's menu, in a note or a comment: a resolved one opens, copies its exact target in either
   * form or reveals it; a broken one written as an absolute path copies that text. Any other link
   * keeps the default menu.
   */
  private linkMenu(e: MouseEvent): void {
    const link = e.target instanceof Element ? e.target.closest("a") : null;
    const found = link && inNoteOrComment(link, this.app.layout.doc) ? linkPath(link) : null;
    if (!link || !found) {
      return;
    }
    e.preventDefault();
    const entries: MenuEntry[] = found.literal
      ? [
          {
            label: "Open",
            run: () => {
              this.app.view.follow(link);
            },
          },
          ...this.copyItems(found),
          { label: "Reveal in Explorer", run: () => void this.app.reveal(found.path) },
        ]
      : this.copyItems(found);
    void this.openMenu(entries, { x: e.clientX, y: e.clientY });
  }

  /** The menu items that copy `target` in Windows and in WSL form. */
  private copyItems(target: PathToCopy): MenuEntry[] {
    return [
      { label: "Copy Windows path", run: () => void this.copyPath(target, "windows") },
      { label: "Copy WSL path", run: () => void this.copyPath(target, "wsl") },
    ];
  }

  /**
   * Shows or hides a sidebar. In a window too narrow for it, it shows on request for the session
   * (`show-<side>`) without changing the setting, unless the setting hid it. The library's column
   * slides open or shut (chrome.css), only on these toggles: never while a resizer drags it.
   */
  toggleSidebar(side: "library" | "outline"): void {
    const app = this.app.layout.app;
    if (side === "library") {
      app.classList.add("sliding");
      clearTimeout(this.slideTimer);
      this.slideTimer = setTimeout(() => {
        app.classList.remove("sliding");
      }, SLIDE_MS);
    }
    const visibleKey = side === "library" ? "libraryVisible" : "outlineVisible";
    const visible = this.app.state.settings[visibleKey];
    const showClass = `show-${side}`;
    if (typeof matchMedia === "function" && matchMedia(NARROW[side]).matches) {
      const showing = visible && app.classList.contains(showClass);
      app.classList.toggle(showClass, !showing);
      if (!showing && !visible) {
        this.app.updateSettings({ [visibleKey]: true });
      }
    } else {
      app.classList.remove(showClass);
      this.app.updateSettings({ [visibleKey]: !visible });
    }
  }

  /** The open comments, on the comments button's badge; none hides it. */
  setCommentCount(n: number): void {
    this.commentsBadge.textContent = n > 0 ? String(n) : "";
    this.commentsBadge.hidden = n === 0;
  }

  /** A pulse on the comments badge for a moment: Claude wrote while the panel was closed. */
  pulseCommentBadge(): void {
    clearTimeout(this.pulseTimer);
    this.commentsBadge.classList.add("pulse");
    this.pulseTimer = setTimeout(() => {
      this.commentsBadge.classList.remove("pulse");
    }, PULSE_MS);
  }

  /**
   * The comments button: there only while the feature is on, pressed while comments show, its
   * badge dimmed while they're hidden.
   */
  private syncCommentsButton(): void {
    const s = this.app.state.settings;
    this.commentsButton.hidden = !s.reviewComments;
    this.commentsButton.setAttribute("aria-pressed", String(s.commentsVisible));
    this.commentsBadge.classList.toggle("muted", !s.commentsVisible);
  }

  /** The library button: pressed while the library shows, its tooltip saying what a click does. */
  private syncLibraryButton(): void {
    const app = this.app.layout.app;
    const narrow = this.narrowLibrary?.matches === true;
    const showing =
      !app.classList.contains("no-library") && (!narrow || app.classList.contains("show-library"));
    this.libraryButton.setAttribute("aria-pressed", String(showing));
    this.libraryButton.title = `${showing ? "Hide" : "Show"} library (Ctrl+B)`;
  }

  /** Opens the document in the editor at the line being read; false without one an editor takes. */
  private openInEditor(): boolean {
    const doc = this.app.state.doc;
    if (!editable(doc)) {
      return false;
    }
    const line = this.app.view.captureAnchor()?.line ?? undefined;
    this.app.backend.openInEditor(doc.path, line).catch((e: unknown) => {
      this.app.toast(String(e));
    });
    return true;
  }

  /**
   * Copies `target` in `form`, as Rust works it out, and names it in a toast. A path with no such
   * form copies nothing, and the toast says so.
   */
  private async copyPath(target: PathToCopy, form: PathForm): Promise<void> {
    let forms: PathForms;
    try {
      forms = await this.app.backend.pathForms(target.path, target.literal);
    } catch (e) {
      this.app.toast(String(e));
      return;
    }
    const path = forms[form];
    if (path === null) {
      this.app.toast(NO_FORM[form]);
      return;
    }
    try {
      await navigator.clipboard.writeText(path);
    } catch {
      this.app.toast("Couldn't copy to the clipboard");
      return;
    }
    this.app.toast(COPIED[form], middleTruncate(path, TOAST_PATH_CHARS));
  }

  /** Whether the document is a PDF, which the find bar can't search: its viewer does. */
  private onPdf(): boolean {
    return this.app.state.doc?.kind === "pdf";
  }

  /** Reads the document again, keeping the place; a file that failed to open is retried. */
  private reload(): void {
    const doc = this.app.state.doc;
    if (doc) {
      void this.app.nav.refresh(doc.path);
    } else if (this.app.state.error) {
      void this.app.open(this.app.state.error.path);
    }
  }

  /**
   * Drag to resize a sidebar: the width follows the pointer, and is saved (debounced) when the
   * drag ends.
   */
  private installResizer(handle: HTMLElement, side: "library" | "outline"): void {
    handle.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) {
        return;
      }
      e.preventDefault();
      handle.setPointerCapture(e.pointerId);
      const startX = e.clientX;
      const start =
        side === "library"
          ? this.app.state.settings.libraryWidth
          : this.app.state.settings.outlineWidth;
      const { min, max } = WIDTHS[side];
      let width = start;
      const move = (ev: PointerEvent): void => {
        const delta = side === "library" ? ev.clientX - startX : startX - ev.clientX;
        width = Math.round(Math.min(max, Math.max(min, start + delta)));
        document.documentElement.style.setProperty(`--${side}-width`, `${String(width)}px`);
      };
      const end = (): void => {
        handle.removeEventListener("pointermove", move);
        handle.removeEventListener("pointerup", end);
        handle.removeEventListener("pointercancel", end);
        document.body.classList.remove("resizing");
        if (width !== start) {
          this.app.updateSettings(
            side === "library" ? { libraryWidth: width } : { outlineWidth: width },
            { debounce: true },
          );
        }
      };
      document.body.classList.add("resizing");
      handle.addEventListener("pointermove", move);
      handle.addEventListener("pointerup", end);
      handle.addEventListener("pointercancel", end);
    });
  }
}

/** Whether `doc` is one an editor opens (Ctrl+E): any document but a PDF. */
function editable(doc: DocPayload | null): doc is DocPayload {
  return doc !== null && doc.kind !== "pdf";
}
