// The app: its state, startup, and opening documents into the layout.
//
// Contracts with the Rust side (Task 8):
// - The `open-request` and `doc-changed` listeners are registered before `startup`: Rust holds
//   second launches until then and sends them as events. One arriving before startup finishes is
//   applied after the first document.
// - Navigations (startup's `initial`, opens, links, launches) are numbered, and the answer to an
//   older one is dropped rather than rendered.
// - `doc-changed` for the open document reloads it in place, keeping the reading position.
//   Refreshes are numbered apart from navigations: one never cancels a navigation, and one that
//   arrives while a navigation is in flight waits for it, applying only if it lands on that path.
// - `startupNotice` shows once, as a toast.
import type { Backend } from "./backend";
import { DocView } from "./doc-view";
import { buildLayout, h, nextPaint, samePath, type Layout } from "./dom";
import type { DocChanged } from "./generated/DocChanged";
import type { DocPayload } from "./generated/DocPayload";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { OpenError } from "./generated/OpenError";
import type { OpenRequest } from "./generated/OpenRequest";
import type { OpenResult } from "./generated/OpenResult";
import type { RecentEntry } from "./generated/RecentEntry";
import type { SavedPosition } from "./generated/SavedPosition";
import type { Settings } from "./generated/Settings";
import { Outline } from "./outline";
import { trackProgress } from "./progress";
import { renderProperties } from "./properties";
import { Toasts } from "./toast";
import { renderError, renderWelcome } from "./welcome";

/** The settings defaults, as Rust has them; shown until `startup` answers. */
export const DEFAULT_SETTINGS: Settings = {
  themeMode: "system",
  lightTheme: "paper",
  darkTheme: "graphite",
  bodyFont: "Segoe UI Variable Text",
  codeFont: "JetBrains Mono",
  fontSize: 18,
  lineHeight: 1.65,
  measure: 72,
  codeWrap: false,
  libraryVisible: true,
  outlineVisible: true,
  libraryWidth: 280,
  outlineWidth: 240,
  libraryRoots: [],
  pathMappings: [],
  editor: { mode: "auto" },
  autoUpdate: true,
};

const BODY_FALLBACK = '"Segoe UI", system-ui, sans-serif, "Segoe UI Emoji"';
const CODE_FALLBACK = '"Cascadia Code", Consolas, ui-monospace, monospace';

export interface AppState {
  settings: Settings;
  library: LibraryPayload;
  doc: DocPayload | null;
  error: OpenError | null;
  recent: RecentEntry[];
  portable: boolean;
  version: string;
}

export interface OpenOptions {
  /** A heading to scroll to: tried as an exact id, then as `slug`. */
  anchor?: string;
  slug?: string;
  /** A source line to scroll to. */
  line?: number;
  /** Push the current document onto the history first (Task 11). */
  push?: boolean;
}

export type Change = "doc" | "settings" | "library";

export class App {
  readonly state: AppState = {
    settings: DEFAULT_SETTINGS,
    library: { roots: [] },
    doc: null,
    error: null,
    recent: [],
    portable: false,
    version: "",
  };
  readonly layout: Layout;
  readonly view: DocView;
  /** Opens the search panel with a query. Task 12 provides it. */
  openSearch: (query: string) => void = () => undefined;
  private readonly toasts: Toasts;
  private readonly outline: Outline;
  private readonly updateProgress: () => void;
  private readonly listeners = new Map<Change, Set<() => void>>();
  /** Bumped by every navigation; the answer to an older one is dropped. */
  private navigations = 0;
  /** The navigation waiting for its answer, if any. */
  private pendingNavigation: number | null = null;
  /** Bumped by every refresh, so only the latest one renders. */
  private refreshes = 0;
  /** Paths changed or removed while a navigation was in flight, settled once it lands. */
  private readonly deferred = new Set<string>();
  /** The native window title last set. */
  private title = "";
  private started = false;
  /** The latest `open-request` that arrived before startup finished. */
  private queued: OpenRequest | null = null;

  constructor(
    readonly backend: Backend,
    root: HTMLElement,
  ) {
    this.layout = buildLayout(root);
    this.toasts = new Toasts(this.layout.toasts);
    this.view = new DocView(this.layout.doc, this);
    this.outline = new Outline(this.layout.outline, this.layout.docPane, (id) => {
      this.view.scrollToAnchor(id);
    });
    this.updateProgress = trackProgress(this.layout.docPane, this.layout.progress);
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
    this.backend.on<DocChanged>("doc-changed", (e) => {
      this.changed(e.path);
    });
    this.backend.on<DocChanged>("doc-removed", (e) => {
      this.removed(e.path);
    });
    this.backend.on<LibraryPayload>("library-updated", (library) => {
      this.state.library = library;
      this.emit("library");
    });

    const navigation = this.beginNavigation();
    let notice: string | null;
    let initial: OpenResult | null = null;
    try {
      const payload = await this.backend.startup();
      Object.assign(this.state, {
        settings: payload.settings,
        library: payload.library,
        recent: payload.recent,
        version: payload.version,
        portable: payload.portable,
      });
      this.applySettings();
      notice = payload.startupNotice;
      initial = payload.initial;
    } catch (e) {
      notice = `Lectern didn't start properly: ${String(e)}`;
    }
    if (this.endNavigation(navigation)) {
      this.show(initial);
      this.settleDeferred();
    }
    await nextPaint();
    this.backend.perfMark("first-paint");
    quietly(this.backend.showWindow());
    if (notice !== null) {
      this.toast(notice);
    }
    this.started = true;
    const queued = this.queued;
    this.queued = null;
    if (queued) {
      await this.openRequested(queued);
    }
  }

  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    await this.load(() => this.backend.openDocument(path), opts);
  }

  /** Asks for a file and opens it, trusting its host: the user chose it. */
  async openFile(): Promise<void> {
    const path = await this.backend.pickFile();
    if (path !== null) {
      await this.load(() => this.backend.openUserPath(path), { push: true });
    }
  }

  /** Asks for a folder and adds it to the library. */
  async addFolder(): Promise<void> {
    const path = await this.backend.pickFolder();
    if (path === null) {
      return;
    }
    try {
      this.state.library = await this.backend.addRoot(path);
      this.emit("library");
      this.toast(`Added ${path} to the library`);
    } catch (e) {
      this.toast(String(e));
    }
  }

  toast(message: string): void {
    this.toasts.show(message);
  }

  on(change: Change, cb: () => void): () => void {
    const set = this.listeners.get(change) ?? new Set();
    this.listeners.set(change, set);
    set.add(cb);
    return () => {
      set.delete(cb);
    };
  }

  private emit(change: Change): void {
    for (const cb of [...(this.listeners.get(change) ?? [])]) {
      cb();
    }
  }

  /** The current navigation's number, for a caller that awaits something before navigating. */
  get navigation(): number {
    return this.navigations;
  }

  /**
   * Fetches a document and shows it unless a newer navigation started meanwhile, then sends the
   * time from call to paint. True when it was shown.
   */
  private async load(fetch: () => Promise<OpenResult>, opts: OpenOptions): Promise<boolean> {
    const t0 = performance.now();
    const navigation = this.beginNavigation();
    let result: OpenResult;
    try {
      result = await fetch();
    } catch (e) {
      if (this.endNavigation(navigation)) {
        this.toast(String(e));
        this.settleDeferred();
      }
      return false;
    }
    if (!this.endNavigation(navigation)) {
      return false;
    }
    this.show(result, opts);
    this.settleDeferred();
    await nextPaint();
    this.backend.perfMark("doc-switch", performance.now() - t0);
    return true;
  }

  private beginNavigation(): number {
    this.pendingNavigation = ++this.navigations;
    return this.navigations;
  }

  /** True when `navigation` is still the latest, which then stops being pending. */
  private endNavigation(navigation: number): boolean {
    if (navigation !== this.navigations) {
      return false;
    }
    this.pendingNavigation = null;
    return true;
  }

  private async openRequested(request: OpenRequest): Promise<void> {
    const shown = await this.load(() => this.backend.openDocument(request.path), { push: true });
    if (shown && request.t0Ms !== null) {
      this.backend.perfMark("warm-open", Date.now() - request.t0Ms);
    }
  }

  /** The path on screen: the document's, or the one that failed to open. */
  private currentPath(): string | null {
    return this.state.doc?.path ?? this.state.error?.path ?? null;
  }

  private isCurrent(path: string): boolean {
    const current = this.currentPath();
    return current !== null && samePath(current, path);
  }

  /** `doc-changed`: reloads the document in place; mid-navigation, waits for it to land. */
  private changed(path: string): void {
    if (this.pendingNavigation !== null) {
      this.deferred.add(path);
    } else if (this.isCurrent(path)) {
      void this.refresh(path);
    }
  }

  /** `doc-removed`: the not-found state; mid-navigation, a refresh once it lands finds out. */
  private removed(path: string): void {
    if (this.pendingNavigation !== null) {
      this.deferred.add(path);
      return;
    }
    if (!this.isCurrent(path)) {
      return;
    }
    // Any refresh still in flight is now out of date.
    ++this.refreshes;
    this.show({
      status: "err",
      error: { kind: "notFound", message: "It was moved or deleted while open.", path },
    });
  }

  /** Refreshes a path changed during the navigation that just landed, if it landed there. */
  private settleDeferred(): void {
    const paths = [...this.deferred];
    this.deferred.clear();
    const path = paths.find((p) => this.isCurrent(p));
    if (path !== undefined) {
      void this.refresh(path);
    }
  }

  /**
   * Re-renders the document at `path` in place, keeping the reading position. A newer refresh,
   * or any navigation started meanwhile, makes the answer obsolete.
   */
  private async refresh(path: string): Promise<void> {
    const refresh = ++this.refreshes;
    const navigation = this.navigations;
    const before = this.state.doc;
    const position = before ? this.view.captureAnchor() : undefined;
    let result: OpenResult;
    try {
      result = await this.backend.openDocument(path);
    } catch {
      return;
    }
    if (refresh !== this.refreshes || navigation !== this.navigations) {
      return;
    }
    if (before && result.status === "ok" && result.doc.html === before.html) {
      // Only the frontmatter can have changed: the title and the properties.
      this.state.doc = result.doc;
      this.setTitle(result.doc.title);
      renderProperties(this.layout.properties, result.doc);
      return;
    }
    this.show(result, {}, position);
  }

  private show(result: OpenResult | null, opts: OpenOptions = {}, position?: SavedPosition): void {
    if (result?.status === "ok") {
      this.showDoc(result.doc, opts, position);
    } else {
      this.state.doc = null;
      this.state.error = result?.error ?? null;
      this.setTitle(null);
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
    this.setTitle(doc.title);
    renderProperties(this.layout.properties, doc);
    this.showBanner(doc.lossy);
    this.outline.render(doc.outline, this.layout.doc);
    if (position) {
      this.view.restore(position);
    } else if (opts.anchor !== undefined && this.view.scrollToAnchor(opts.anchor, opts.slug)) {
      // Scrolled to the anchor.
    } else if (opts.line !== undefined && this.view.scrollToLine(opts.line)) {
      // Scrolled to the line.
    } else {
      this.scroller.scrollTop = 0;
    }
  }

  /** The native window title: the document's, or just Lectern. */
  private setTitle(docTitle: string | null): void {
    const title = docTitle === null ? "Lectern" : `${docTitle} — Lectern`;
    if (title !== this.title) {
      this.title = title;
      quietly(this.backend.setTitle(title));
    }
  }

  private showWelcome(): void {
    renderWelcome(this.layout.doc, this.state.recent, {
      openFile: () => void this.openFile(),
      addFolder: () => void this.addFolder(),
      openRecent: (path) => void this.open(path, { push: true }),
      removeRecent: (path) => void this.removeRecent(path),
    });
  }

  private showError(error: OpenError): void {
    renderError(this.layout.doc, error, {
      retry: () => void this.open(error.path),
      forget: () => void this.removeRecent(error.path),
      openWithDefaultApp: () => void this.openWithDefaultApp(error.path),
      reveal: () => void this.reveal(error.path),
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

  private async reveal(path: string): Promise<void> {
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
   * Applies the reading and layout settings. Task 10 moves the reading part into
   * `themes.applySettings`, with the theme and the title-bar colours.
   */
  private applySettings(): void {
    const s = this.state.settings;
    const root = document.documentElement;
    root.style.setProperty("--font-size", `${String(s.fontSize)}px`);
    root.style.setProperty("--line-height", String(s.lineHeight));
    root.style.setProperty("--measure", s.measure === "full" ? "none" : `${String(s.measure)}ch`);
    root.style.setProperty("--body-font", `${fontName(s.bodyFont)}, ${BODY_FALLBACK}`);
    root.style.setProperty("--code-font", `${fontName(s.codeFont)}, ${CODE_FALLBACK}`);
    root.style.setProperty("--library-width", `${String(s.libraryWidth)}px`);
    root.style.setProperty("--outline-width", `${String(s.outlineWidth)}px`);
    root.classList.toggle("code-wrap", s.codeWrap);
    this.layout.app.classList.toggle("no-library", !s.libraryVisible);
    this.layout.app.classList.toggle("no-outline", !s.outlineVisible);
    this.emit("settings");
  }
}

/** A font family name as a CSS string. */
function fontName(name: string): string {
  return `"${name.replace(/["\\]/g, "")}"`;
}

/** Runs a promise for its effect, logging a failure instead of leaving it unhandled. */
function quietly(promise: Promise<unknown>): void {
  promise.catch((e: unknown) => {
    console.warn(e);
  });
}
