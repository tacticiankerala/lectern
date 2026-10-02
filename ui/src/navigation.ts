// Navigation: opening documents, the numbering that drops stale answers, refreshes in place,
// and back and forward.
//
// - Navigations (startup's `initial`, opens, links, launches) are numbered, and the answer to an
//   older one is dropped rather than rendered.
// - `doc-changed` for the open document reloads it in place, keeping the reading position.
//   Refreshes are numbered apart from navigations: one never cancels a navigation, and one that
//   arrives while a navigation is in flight waits for it, applying only if it lands on that path.
// - Navigations that ask to `push` record the document being left, with its reading position, on
//   the history once they land somewhere else; back and forward restore that position. A travel
//   changes the history only once it lands, and a second one in flight goes on from its target.
import type { App, OpenOptions } from "./app";
import { nextPaint, samePath } from "./dom";
import type { OpenRequest } from "./generated/OpenRequest";
import type { OpenResult } from "./generated/OpenResult";
import { History, type HistoryEntry } from "./history";
import { iconButton, ICONS } from "./icons";

export class Navigation {
  private history = new History();
  /** A back or forward in flight: the history as it will be once it lands, and its target. */
  private travelling: { history: History; target: HistoryEntry; navigation: number } | null = null;
  private readonly backButton: HTMLButtonElement;
  private readonly forwardButton: HTMLButtonElement;
  /** Bumped by every navigation; the answer to an older one is dropped. */
  private navigations = 0;
  /** The navigation waiting for its answer, if any. */
  private pendingNavigation: number | null = null;
  /** Bumped by every refresh, so only the latest one renders. */
  private refreshes = 0;
  /** Paths changed or removed while a navigation was in flight, settled once it lands. */
  private readonly deferred = new Set<string>();

  constructor(private readonly app: App) {
    this.backButton = iconButton("lx-back", "Back", ICONS.back, "Alt+←");
    this.forwardButton = iconButton("lx-forward", "Forward", ICONS.forward, "Alt+→");
    this.backButton.addEventListener("click", () => void this.travel("back"));
    this.forwardButton.addEventListener("click", () => void this.travel("forward"));
    app.layout.historyNav.append(this.backButton, this.forwardButton);
    app.on("doc", () => {
      this.renderNav();
    });
    this.renderNav();
    // The mouse's back and forward buttons; WebView2 would otherwise navigate the page. Once a
    // test replaces the page's app, the old one stops listening.
    const root = app.layout.app;
    const page = root.ownerDocument;
    page.addEventListener("mouseup", (e) => {
      if (root.isConnected && (e.button === 3 || e.button === 4)) {
        e.preventDefault();
        void this.travel(e.button === 3 ? "back" : "forward");
      }
    });
    page.addEventListener("auxclick", (e) => {
      if (e.button === 3 || e.button === 4) {
        e.preventDefault();
      }
    });
  }

  /** The current navigation's number. */
  get latest(): number {
    return this.navigations;
  }

  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    await this.load(() => this.app.backend.openDocument(path), opts);
  }

  /**
   * Opens a file or folder the user chose (the pickers or a drop), as a launch would: a folder
   * joins the library unless it nests with a root, and opens its README. A folder that opens
   * nothing and adds nothing is revealed in the sidebar instead.
   */
  async openUserPath(path: string): Promise<void> {
    const before = new Set(this.app.state.library.roots.map((r) => r.path.toLowerCase()));
    const outcome = { added: null as string | null, opened: false };
    await this.load(
      async () => {
        const result = await this.app.backend.openUserPath(path);
        this.app.library.setLibrary(result.library);
        outcome.added =
          result.library.roots.find((r) => !before.has(r.path.toLowerCase()))?.path ?? null;
        outcome.opened = result.doc !== null;
        return result.doc;
      },
      { push: true },
    );
    if (outcome.added !== null) {
      this.app.toast(`Added ${outcome.added} to the library`);
    } else if (!outcome.opened) {
      this.app.library.revealFolder(path);
    }
  }

  /** Files and folders dropped on the window, opened in turn: the last document stays. */
  async dropped(paths: string[]): Promise<void> {
    for (const path of paths) {
      await this.openUserPath(path);
    }
  }

  /**
   * Fetches a document and shows it unless a newer navigation started meanwhile, then sends the
   * time from call to paint. `landed` runs once it is accepted, just before it shows. True when it
   * was shown.
   */
  private async load(
    fetch: () => Promise<OpenResult | null>,
    opts: OpenOptions,
    landed?: () => void,
  ): Promise<boolean> {
    const t0 = performance.now();
    const navigation = this.beginNavigation();
    let result: OpenResult | null;
    try {
      result = await fetch();
    } catch (e) {
      if (this.endNavigation(navigation)) {
        this.app.toast(String(e));
        this.settleDeferred();
      }
      return false;
    }
    if (!this.endNavigation(navigation)) {
      return false;
    }
    if (result === null) {
      // Nothing to open (a folder without a README): the document on screen stays.
      this.settleDeferred();
      return false;
    }
    landed?.();
    if (opts.push) {
      this.pushHistory(result);
    }
    this.app.show(result, opts, opts.position);
    this.settleDeferred();
    await nextPaint();
    this.app.backend.perfMark("doc-switch", performance.now() - t0);
    return true;
  }

  beginNavigation(): number {
    this.pendingNavigation = ++this.navigations;
    return this.navigations;
  }

  /** True when `navigation` is still the latest, which then stops being pending. */
  endNavigation(navigation: number): boolean {
    if (navigation !== this.navigations) {
      return false;
    }
    this.pendingNavigation = null;
    return true;
  }

  async openRequested(request: OpenRequest): Promise<void> {
    const shown = await this.load(() => this.app.backend.openDocument(request.path), {
      push: true,
    });
    if (shown && request.t0Ms !== null) {
      this.app.backend.perfMark("warm-open", Date.now() - request.t0Ms);
    }
  }

  /** Records the document on screen on the history, unless `result` is that same path. */
  private pushHistory(result: OpenResult): void {
    const leaving = this.currentEntry();
    const target = result.status === "ok" ? result.doc.path : result.error.path;
    if (leaving && !samePath(leaving.path, target)) {
      this.history.push(leaving);
    }
  }

  /** The path on screen and the reading position in it, for the history. */
  private currentEntry(): HistoryEntry | null {
    const path = this.currentPath();
    if (path === null) {
      return null;
    }
    return { path, position: this.app.state.doc ? this.app.view.captureAnchor() : null };
  }

  /**
   * Goes back or forward, to the position the document was left at. The history changes only
   * once the navigation lands; a second step while one is in flight goes on from its target.
   * From the welcome screen, nothing is left behind to come back to.
   */
  async travel(direction: "back" | "forward"): Promise<void> {
    const pending = this.pendingTravel();
    const draft = (pending?.history ?? this.history).clone();
    const from = pending ? pending.target : this.currentEntry();
    const target = direction === "back" ? draft.back(from) : draft.forward(from);
    if (target === null) {
      return;
    }
    const opts: OpenOptions = {};
    if (target.position) {
      opts.position = target.position;
    }
    const landing = this.load(
      () => this.app.backend.openDocument(target.path),
      opts,
      () => {
        this.history = draft;
      },
    );
    // `load` has begun its navigation by now.
    this.travelling = { history: draft, target, navigation: this.navigations };
    this.renderNav();
    await landing;
    this.renderNav();
  }

  /** The travel in flight, while it is still the latest navigation. */
  private pendingTravel(): { history: History; target: HistoryEntry } | null {
    const t = this.travelling;
    return t && t.navigation === this.navigations && this.pendingNavigation === t.navigation
      ? t
      : null;
  }

  /** The back and forward buttons, as of the travel in flight if there is one. */
  private renderNav(): void {
    const history = this.pendingTravel()?.history ?? this.history;
    this.backButton.disabled = !history.canBack();
    this.forwardButton.disabled = !history.canForward();
  }

  /** The path on screen: the document's, or the one that failed to open. */
  currentPath(): string | null {
    return this.app.state.doc?.path ?? this.app.state.error?.path ?? null;
  }

  private isCurrent(path: string): boolean {
    const current = this.currentPath();
    return current !== null && samePath(current, path);
  }

  /** `doc-changed`: reloads the document in place; mid-navigation, waits for it to land. */
  changed(path: string): void {
    if (this.pendingNavigation !== null) {
      this.deferred.add(path);
    } else if (this.isCurrent(path)) {
      void this.refresh(path);
    }
  }

  /** `doc-removed`: the not-found state; mid-navigation, a refresh once it lands finds out. */
  removed(path: string): void {
    if (this.pendingNavigation !== null) {
      this.deferred.add(path);
      return;
    }
    if (!this.isCurrent(path)) {
      return;
    }
    // Any refresh still in flight is now out of date.
    ++this.refreshes;
    this.app.show({
      status: "err",
      error: { kind: "notFound", message: "It was moved or deleted while open.", path },
    });
  }

  /** Refreshes a path changed during the navigation that just landed, if it landed there. */
  settleDeferred(): void {
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
  async refresh(path: string): Promise<void> {
    const refresh = ++this.refreshes;
    const navigation = this.navigations;
    const before = this.app.state.doc;
    const position = before ? this.app.view.captureAnchor() : undefined;
    let result: OpenResult;
    try {
      result = await this.app.backend.openDocument(path);
    } catch {
      return;
    }
    if (refresh !== this.refreshes || navigation !== this.navigations) {
      return;
    }
    if (before && result.status === "ok" && result.doc.html === before.html) {
      // Only what surrounds the body can have changed: the title, properties and breadcrumbs.
      this.app.state.doc = result.doc;
      this.app.showMeta(result.doc);
      return;
    }
    this.app.show(result, {}, position);
  }
}
