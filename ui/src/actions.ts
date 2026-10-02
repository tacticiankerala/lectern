// What the user asks for: shortcuts, the header's buttons and menus, the sidebar's context menu,
// quick open, Preferences, find in page and full-text search (all loaded on first use), focus mode
// and the sidebars' widths.
import type { App } from "./app";
import { quietly, samePath } from "./dom";
import type { FindBar } from "./find";
import { iconButton, ICONS } from "./icons";
import type { Action } from "./keymap";
import { NARROW } from "./library-controller";
import type { Menu, MenuEntry } from "./menu";
import { blockAt } from "./position";
import type { Preferences } from "./preferences";
import type { QuickOpen } from "./quick-open";
import { FlowHold, keepFlow } from "./reflow";
import type { SearchPanel } from "./search-panel";
import { bumpFontSize, toggleThemeMode } from "./themes";

/** The sidebars' widths, in pixels, as the resizers allow them. */
const WIDTHS = {
  library: { min: 180, max: 560 },
  outline: { min: 160, max: 480 },
} as const;

export class Actions {
  /** Loaded on first use. */
  quickOpen: QuickOpen | null = null;
  private preferences: Preferences | null = null;
  private search: SearchPanel | null = null;
  private find: FindBar | null = null;
  /** The menu open, if any. */
  private menu: Menu | null = null;
  private readonly moreButton: HTMLButtonElement;
  private focusMode = false;
  private readonly hold: FlowHold;

  constructor(private readonly app: App) {
    this.hold = new FlowHold(app.scroller);
    const outlineButton = iconButton("lx-outline-btn", "Outline", ICONS.outline);
    outlineButton.addEventListener("click", () => {
      this.toggleSidebar("outline");
    });
    this.moreButton = iconButton("lx-more-btn", "More", ICONS.more);
    this.moreButton.setAttribute("aria-haspopup", "menu");
    this.moreButton.addEventListener("click", () => {
      void this.toggleMoreMenu();
    });
    app.layout.headerActions.append(outlineButton, this.moreButton);
    app.on("library", () => {
      this.preferences?.refresh();
    });
    for (const handle of app.layout.app.querySelectorAll<HTMLElement>(".resizer")) {
      const side = handle.dataset.for;
      if (side === "library" || side === "outline") {
        this.installResizer(handle, side);
      }
    }
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
      case "search":
        this.app.openSearch();
        return true;
      case "find":
        this.app.openFind();
        return true;
      case "find-next":
      case "find-prev":
        // F3 is also WebView2's own find shortcut, so it is always handled.
        if (this.find?.isOpen) {
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
      case "copy-path":
        return this.copyPath();
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
      case "toggle-theme":
        this.app.updateSettings(toggleThemeMode(s, this.app.systemDark()));
        return true;
      case "focus":
        this.setFocusMode(!this.focusMode);
        return true;
      case "escape":
        for (const overlay of [
          this.menu,
          this.quickOpen,
          this.search,
          this.preferences,
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
    this.preferences?.close();
    this.search?.close();
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
    this.quickOpen?.close();
    this.preferences?.close();
    this.app.panel.close();
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

  /** The find bar, searching for `prefill` when given, from `near` (see `FindBar.open`). */
  async showFind(prefill?: string, near?: Element): Promise<void> {
    this.menu?.close(false);
    const { FindBar } = await import("./find.js");
    this.find ??= new FindBar(this.app.layout.docPane, {
      doc: () => (this.app.state.doc ? this.app.layout.doc : null),
      scroller: this.app.scroller,
      placeAt: (el, at) => {
        this.app.view.placeAt(el, at);
      },
      onRender: (cb) => {
        this.app.on("doc", cb);
      },
    });
    this.quickOpen?.close();
    this.search?.close();
    this.preferences?.close();
    this.app.panel.close();
    this.find.open(prefill, near);
  }

  private async showPreferences(): Promise<void> {
    this.menu?.close(false);
    const { Preferences } = await import("./preferences.js");
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
    });
    this.quickOpen?.close();
    this.search?.close();
    this.app.panel.close();
    this.preferences.open();
  }

  /** Opens a menu, replacing any other; it forgets itself once closed. */
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
        this.moreButton.setAttribute("aria-expanded", "false");
      }
    });
    this.menu = menu;
  }

  private async toggleMoreMenu(): Promise<void> {
    if (this.menu?.isOpen) {
      this.menu.close();
      return;
    }
    const doc = this.app.state.doc;
    const path = this.app.nav.currentPath();
    await this.openMenu(
      [
        { label: "Open in editor", keys: "Ctrl+E", disabled: !doc, run: () => this.openInEditor() },
        {
          label: "Reveal in Explorer",
          disabled: path === null,
          run: () => {
            if (path !== null) void this.app.reveal(path);
          },
        },
        {
          label: "Copy path",
          keys: "Ctrl+Shift+C",
          disabled: path === null,
          run: () => this.copyPath(),
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
        { label: "Preferences", keys: "Ctrl+,", run: () => void this.showPreferences() },
        {
          label: "Check for updates",
          run: () => {
            this.app.checkForUpdates();
          },
        },
        {
          label: "About Lectern",
          run: () => {
            this.app.toast(`Lectern ${this.app.state.version}: a Markdown reader. MIT licence.`);
          },
        },
      ],
      this.moreButton,
    );
    this.moreButton.setAttribute("aria-expanded", "true");
  }

  /** The sidebar's menu for a row: reveal, copy, and on a root's own row, remove it. */
  async contextMenu(e: MouseEvent, path: string, isRoot: boolean): Promise<void> {
    const entries: MenuEntry[] = [
      { label: "Reveal in Explorer", run: () => void this.app.reveal(path) },
      { label: "Copy path", run: () => void this.copyText(path) },
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
   * Shows or hides a sidebar. In a window too narrow for it, it shows on request for the session
   * (`show-<side>`) without changing the setting, unless the setting hid it.
   */
  toggleSidebar(side: "library" | "outline"): void {
    const app = this.app.layout.app;
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

  /** Opens the document in the editor at the line being read; false without a document. */
  private openInEditor(): boolean {
    const doc = this.app.state.doc;
    if (!doc) {
      return false;
    }
    const line = this.app.view.captureAnchor().line ?? undefined;
    this.app.backend.openInEditor(doc.path, line).catch((e: unknown) => {
      this.app.toast(String(e));
    });
    return true;
  }

  /** Copies the path on screen; false when there is none. */
  private copyPath(): boolean {
    const path = this.app.nav.currentPath();
    if (path === null) {
      return false;
    }
    void this.copyText(path);
    return true;
  }

  private async copyText(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      this.app.toast("Copied the path");
    } catch {
      this.app.toast("Couldn't copy to the clipboard");
    }
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
