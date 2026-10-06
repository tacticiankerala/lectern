// The library: the sidebar, the roots as Rust reports them, and quick open's candidates.
// The sidebar renders after the first paint, so it never holds it up.
import type { App } from "./app";
import type { Candidate } from "./generated/Candidate";
import type { LibraryPayload } from "./generated/LibraryPayload";
import { Sidebar } from "./sidebar";

/** Below these window widths a sidebar hides unless shown on purpose (chrome.css). */
export const NARROW = {
  library: "(max-width: 799.98px)",
  outline: "(max-width: 1099.98px)",
} as const;

export class LibraryController {
  readonly sidebar: Sidebar;
  /** Quick open's candidates, fetched once per library change. */
  private candidateCache: Promise<Candidate[]> | null = null;

  constructor(private readonly app: App) {
    this.sidebar = new Sidebar(app.layout.library, {
      open: (path) => void app.open(path, { push: true }),
      retry: (path) => void this.retryRoot(path),
      contextMenu: (e, path, isRoot) => void app.actions.contextMenu(e, path, isRoot),
      addFolder: () => void app.addFolder(),
    });
    app.on("library", () => {
      this.sidebar.setLibrary(app.state.library);
    });
    app.on("settings", () => {
      this.sidebar.setBadges(app.state.settings.showStatusBadges);
      this.sidebar.setCommentCounts(app.state.settings.reviewComments);
    });
    app.on("doc", () => {
      this.sidebar.setActive(app.state.doc?.path ?? null);
    });
  }

  /** Fills in the sidebar, right after the first paint. */
  start(): void {
    this.sidebar.setLibrary(this.app.state.library);
    this.sidebar.start();
  }

  /**
   * Listens for `index-ready`, once the window shows: indexing (which ends in it, also for the
   * folder of a file opened outside the library) starts after that.
   */
  watchIndex(): void {
    this.app.backend.on("index-ready", () => {
      this.invalidateCandidates();
    });
  }

  /** Takes a library from Rust; the roots it lists are the settings' roots too. */
  setLibrary(library: LibraryPayload): void {
    this.app.state.library = library;
    this.app.state.settings = {
      ...this.app.state.settings,
      libraryRoots: library.roots.map((r) => r.path),
    };
    this.invalidateCandidates();
    this.app.emit("library");
  }

  /** Quick open's files changed: fetched again on next use, or now if it is showing. */
  invalidateCandidates(): void {
    this.candidateCache = null;
    if (this.app.actions.quickOpen?.isOpen) {
      this.app.actions.quickOpen.refresh();
    }
  }

  async retryRoot(path: string): Promise<void> {
    try {
      this.setLibrary(await this.app.backend.retryRoot(path));
    } catch (e) {
      this.app.toast(String(e));
    }
  }

  async removeRoot(path: string): Promise<void> {
    try {
      this.setLibrary(await this.app.backend.removeRoot(path));
    } catch (e) {
      this.app.toast(String(e));
    }
  }

  /** Quick open's candidates: fetched once, then again after the library changes. */
  candidates(): Promise<Candidate[]> {
    this.candidateCache ??= this.app.backend.quickOpenCandidates().catch((e: unknown) => {
      this.candidateCache = null;
      throw e;
    });
    return this.candidateCache;
  }

  /** Reveals a folder in the library sidebar, showing the sidebar first if it is hidden. */
  revealFolder(path: string): void {
    const app = this.app.layout.app;
    if (!this.app.state.settings.libraryVisible) {
      this.app.updateSettings({ libraryVisible: true });
    }
    if (typeof matchMedia === "function" && matchMedia(NARROW.library).matches) {
      app.classList.add("show-library");
    }
    this.sidebar.reveal(path);
  }
}
