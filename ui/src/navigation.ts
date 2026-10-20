// Navigation: opening documents, the numbering that drops stale answers, refreshes in place,
// and back and forward.
//
// - Navigations (startup's `initial`, opens, links, launches) are numbered, and the answer to an
//   older one is dropped rather than rendered. One on its way can be dropped with nothing in its
//   place, the document on screen staying (an agent's open that a newer show took the place of).
// - Every open sent to Rust, refreshes included, and every leave or keep of the document, is
//   numbered too (`nextGeneration`), in the order sent: Rust may run them in another, and orders
//   them by the number, so an open numbered below the last leave or keep never becomes its
//   document, and a newer open stays it. The numbering starts from what Rust has seen (the
//   startup payload's `generation`), so a page reloaded over the same window state numbers above
//   everything the page before sent; nothing is sent before startup has seeded it.
// - Leaving the document for none (home, or a file deleted while open) tells Rust, so nothing acts
//   on that document any more. A file deleted while open is still watched, so once it comes back
//   it shows again by itself.
// - `doc-changed` for the open document reloads it in place, keeping the reading position; when
//   the file changed on disk, the properties strip notes the time. Refreshes are numbered apart
//   from navigations: one never cancels a navigation, and one that arrives while a navigation is
//   in flight waits for it, applying only if it lands on that path. A navigation may find where it
//   lands once its file has opened, before it shows (an agent's target found again in the file as
//   it is now): still in flight meanwhile, so a change then renders over where it landed. A move
//   within the document for an agent waits for a refresh on its way (`refreshed`), so it goes to
//   the revision the reader sees; never for one that a newer refresh or a navigation has left for
//   dropping.
// - A navigation that lands somewhere else saves the reading position in the document it leaves.
//   Those that ask to `push` also record it, with that position, on the history; back and forward
//   restore it. A travel changes the history only once it lands, and a second one in flight goes
//   on from its target. Before something moves within the document on screen for the reader (an
//   agent's show), where they are is marked on the history, so Back returns there.
import type { App, OpenOptions } from "./app";
import { nextPaint, samePath } from "./dom";
import type { OpenRequest } from "./generated/OpenRequest";
import type { OpenResult } from "./generated/OpenResult";
import { History, type HistoryEntry } from "./history";
import { iconButton, ICONS } from "./icons";

/** A refresh while it is on its way (`Navigation.refreshing`). */
interface Refreshing {
  refresh: number;
  path: string;
  /** The navigation it started in: after another, its answer is dropped. */
  navigation: number;
}

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
  /** Numbers every open, leave and keep sent to Rust, in that order (see the top of this file). */
  private generation = 0;
  /** Resolves once `numberFrom` has seeded the numbering from the startup payload. */
  private readonly seeded: Promise<void>;
  private seed: () => void = () => undefined;
  /**
   * The latest refresh while it is on its way: its number and path (`drop` requeues it), and the
   * navigation it started in.
   */
  private refreshing: Refreshing | null = null;
  /** Resolves, and is renewed, whenever a refresh starts, answers or loses its place (`refreshMoved`). */
  private refreshChanged: Promise<void>;
  private refreshChange: () => void = () => undefined;
  /** Resolves, and is renewed, whenever a navigation begins (`beginNavigation`): `navigated`. */
  private navigationBegun: Promise<void>;
  private navigationBegin: () => void = () => undefined;

  constructor(private readonly app: App) {
    this.seeded = new Promise((resolve) => {
      this.seed = resolve;
    });
    this.refreshChanged = new Promise((resolve) => {
      this.refreshChange = resolve;
    });
    this.navigationBegun = new Promise((resolve) => {
      this.navigationBegin = resolve;
    });
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

  /**
   * The number of the open, leave or keep about to be sent to Rust: the next in order, once
   * startup has seeded the numbering (`numberFrom`). Nothing is numbered, or sent, before: an
   * open asked meanwhile waits for it.
   */
  async nextGeneration(): Promise<number> {
    await this.seeded;
    return ++this.generation;
  }

  /**
   * Numbers from `generation` on: what Rust has seen from this window already, when its page
   * loaded over the same window state (WebView2's own reload, a renderer failure), so the first
   * open sent is numbered above everything before it, and an open still on its way from the page
   * before never becomes Rust's document. Seeds the numbering: nothing is sent before it runs.
   */
  numberFrom(generation: number): void {
    this.generation = Math.max(this.generation, generation);
    this.seed();
  }

  async open(path: string, opts: OpenOptions = {}): Promise<void> {
    await this.load(
      async () => this.app.backend.openDocument(path, await this.nextGeneration()),
      opts,
    );
  }

  /**
   * Opens a file or folder the user chose (the pickers or a drop), as a launch would: a folder
   * joins the library unless it nests with a root, and opens its README. A folder that opens
   * nothing and adds nothing is revealed in the sidebar instead. A blank window has no library:
   * Rust says the path is a folder (`folder`), and a new workspace to hold it is asked for; true
   * then.
   */
  async openUserPath(path: string): Promise<boolean> {
    const before = new Set(this.app.state.library.roots.map((r) => r.path.toLowerCase()));
    const outcome = { added: null as string | null, opened: false, folder: false };
    await this.load(
      async () => {
        const result = await this.app.backend.openUserPath(path, await this.nextGeneration());
        this.app.library.setLibrary(result.library);
        outcome.added =
          result.library.roots.find((r) => !before.has(r.path.toLowerCase()))?.path ?? null;
        outcome.opened = result.doc !== null;
        outcome.folder = result.folder;
        return result.doc;
      },
      { push: true },
    );
    if (outcome.folder) {
      await this.app.workspaces.createWithFolder(path);
      return true;
    }
    if (outcome.added !== null) {
      this.app.toast(`Added ${outcome.added} to the library`);
    } else if (!outcome.opened) {
      this.app.library.revealFolder(path);
    }
    return false;
  }

  /**
   * Files and folders dropped on the window, opened in turn: the last document stays. In a blank
   * window, a folder asks for a new workspace to hold it instead, and the rest are left.
   */
  async dropped(paths: string[]): Promise<void> {
    for (const path of paths) {
      if (await this.openUserPath(path)) {
        return;
      }
    }
  }

  /**
   * Shows the welcome screen (a blank window's list of workspaces to choose from), leaving the
   * document on screen as an open would: its reading position saved, and on the history, so Back
   * returns to it. A navigation still on its way is dropped.
   */
  home(): void {
    this.endNavigation(this.beginNavigation());
    this.app.leaveDocument();
    const leaving = this.currentEntry();
    if (leaving) {
      if (leaving.position) {
        this.app.savePosition(leaving.path, leaving.position);
      }
      this.history.push(leaving);
    }
    this.app.state.updated = null;
    this.app.show(null);
    void this.settleDeferred();
  }

  /**
   * Fetches a document and shows it unless a newer navigation started meanwhile, then sends the
   * time from call to paint. Where it lands may be found once it has opened (`findLanding`), before
   * it shows, while this is still the navigation on its way. `landed` runs once it is accepted,
   * just before it shows. True when it was shown.
   */
  private async load(
    fetch: () => Promise<OpenResult | null>,
    opts: OpenOptions,
    landed?: () => void,
  ): Promise<boolean> {
    const t0 = performance.now();
    const navigation = this.beginNavigation();
    let result: OpenResult | null;
    let at = opts;
    try {
      result = await fetch();
      if (
        result?.status === "ok" &&
        opts.findLanding !== undefined &&
        navigation === this.navigations
      ) {
        at = { ...opts, ...(await opts.findLanding()) };
      }
    } catch (e) {
      if (this.endNavigation(navigation)) {
        this.app.toast(String(e));
        void this.settleDeferred();
      }
      return false;
    }
    if (!this.endNavigation(navigation)) {
      return false;
    }
    if (result === null) {
      // Nothing to open (a folder without a README): the document on screen stays.
      void this.settleDeferred();
      return false;
    }
    landed?.();
    this.leave(result, at.push === true);
    this.app.state.updated = null;
    this.app.show(result, at, at.position);
    void this.settleDeferred();
    await nextPaint();
    this.app.backend.perfMark("doc-switch", performance.now() - t0);
    return true;
  }

  beginNavigation(): number {
    this.pendingNavigation = ++this.navigations;
    // A refresh in flight is left for dropping: `refreshed` looks again.
    this.refreshMoved();
    const wake = this.navigationBegin;
    this.navigationBegun = new Promise((resolve) => {
      this.navigationBegin = resolve;
    });
    wake();
    return this.navigations;
  }

  /** Resolves once a navigation begins, after this call: a wait on the document ends there. */
  navigated(): Promise<void> {
    return this.navigationBegun;
  }

  /** True when `navigation` is still the latest, which then stops being pending. */
  endNavigation(navigation: number): boolean {
    if (navigation !== this.navigations) {
      return false;
    }
    this.pendingNavigation = null;
    return true;
  }

  /**
   * Drops `navigation` while it is still on its way and the latest, as a newer one would: its
   * answer is never shown, and the document on screen stays. True when it did; one that has
   * landed, or that a newer navigation took the place of, is left be. A path changed meanwhile,
   * and one whose refresh on its way is lost to the drop, are left for the caller to settle
   * (`settleDeferred`), once Rust has been told what the window keeps, so that refresh is
   * numbered after the keep and the caller can wait for it.
   */
  drop(navigation: number): boolean {
    if (this.pendingNavigation !== navigation) {
      return false;
    }
    const lost = this.refreshing;
    if (lost !== null && lost.refresh === this.refreshes) {
      this.deferred.add(lost.path);
    }
    this.endNavigation(this.beginNavigation());
    return true;
  }

  async openRequested(request: OpenRequest): Promise<void> {
    const shown = await this.load(
      async () => this.app.backend.openDocument(request.path, await this.nextGeneration()),
      { push: true },
    );
    if (shown && request.t0Ms !== null) {
      this.app.backend.perfMark("warm-open", Date.now() - request.t0Ms);
    }
  }

  /**
   * Leaves the path on screen for `result`'s, unless it is that same path: saves the reading
   * position in it and, with `push`, records it on the history.
   */
  private leave(result: OpenResult, push: boolean): void {
    const leaving = this.currentEntry();
    const target = result.status === "ok" ? result.doc.path : result.error.path;
    if (!leaving || samePath(leaving.path, target)) {
      return;
    }
    if (leaving.position) {
      this.app.savePosition(leaving.path, leaving.position);
    }
    if (push) {
      this.history.push(leaving);
    }
  }

  /**
   * Marks where the reader is in the document on screen on the history, so Back returns there:
   * before something moves within it for them.
   */
  markHere(): void {
    const here = this.currentEntry();
    if (here) {
      this.history.mark(here);
      this.renderNav();
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
      async () => this.app.backend.openDocument(target.path, await this.nextGeneration()),
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
    this.refreshMoved();
    // Missing, not left: Rust still watches it, so its return reloads it (`changed`).
    this.app.leaveDocument(true);
    this.app.show({
      status: "err",
      error: { kind: "notFound", message: "It was moved or deleted while open.", path },
    });
  }

  /**
   * Refreshes a path changed during the navigation that just landed, if it landed there; resolves
   * once that refresh has rendered, or been dropped, or at once for none.
   */
  settleDeferred(): Promise<void> {
    const paths = [...this.deferred];
    this.deferred.clear();
    const path = paths.find((p) => this.isCurrent(p));
    if (path === undefined) {
      this.refreshMoved();
      return Promise.resolve();
    }
    return this.refresh(path);
  }

  /**
   * Re-renders the document at `path` in place, keeping the reading position, and notes the time
   * in the properties strip when the file changed on disk (rather than, say, its links resolving
   * once the library is indexed). A newer refresh, or any navigation started meanwhile, makes the
   * answer obsolete. Resolves once it has rendered, or been dropped.
   */
  refresh(path: string): Promise<void> {
    const refresh = ++this.refreshes;
    const done = this.rerender(refresh, path);
    // On its way until its fetch answers, which takes this off (`rerender`).
    this.refreshing = { refresh, path, navigation: this.navigations };
    this.refreshMoved();
    return done;
  }

  /**
   * Resolves once no refresh of the document on screen is on its way (`refreshOnItsWay`); at once
   * when none is. Looks again whenever a refresh starts, answers or loses its place, so it never
   * waits for an answer that will be dropped.
   */
  async refreshed(): Promise<void> {
    while (this.refreshOnItsWay()) {
      await this.refreshChanged;
    }
  }

  /**
   * Whether a refresh of the document on screen is on its way: one in flight that can still
   * render (not one a newer refresh, or a navigation since, has left for dropping), or a change to
   * it deferred with no navigation in flight to settle it, which `settleDeferred` is about to (a
   * refresh lost to `drop`). A change deferred behind the reader's navigation is theirs: that
   * navigation settles it once it lands.
   */
  private refreshOnItsWay(): boolean {
    const refreshing = this.refreshing;
    if (
      refreshing !== null &&
      refreshing.refresh === this.refreshes &&
      refreshing.navigation === this.navigations
    ) {
      return true;
    }
    return this.pendingNavigation === null && [...this.deferred].some((p) => this.isCurrent(p));
  }

  /** Wakes `refreshed` to look again: a refresh started, answered or lost its place. */
  private refreshMoved(): void {
    const wake = this.refreshChange;
    this.refreshChanged = new Promise((resolve) => {
      this.refreshChange = resolve;
    });
    wake();
  }

  /** The refresh `refresh` of `path`: its fetch, then its render unless obsolete by then. */
  private async rerender(refresh: number, path: string): Promise<void> {
    const navigation = this.navigations;
    const before = this.app.state.doc;
    const position = before ? (this.app.view.captureAnchor() ?? undefined) : undefined;
    // This fetches the latest, so a change deferred for the path is covered by it.
    this.deferred.delete(path);
    let result: OpenResult;
    try {
      result = await this.app.backend.openDocument(path, await this.nextGeneration());
    } catch {
      return;
    } finally {
      if (this.refreshing?.refresh === refresh) this.refreshing = null;
      this.refreshMoved();
    }
    if (refresh !== this.refreshes || navigation !== this.navigations) {
      return;
    }
    if (before && result.status === "ok" && result.doc.mtimeMs !== before.mtimeMs) {
      this.app.state.updated = Date.now();
    }
    if (
      before &&
      result.status === "ok" &&
      result.doc.frameUrl === null &&
      result.doc.html === before.html
    ) {
      // Only what surrounds the body can have changed: the title, properties and breadcrumbs. A
      // page or a PDF in a frame is loaded again whatever, as Lectern never reads it.
      this.app.state.doc = result.doc;
      this.app.showMeta(result.doc);
      return;
    }
    this.app.show(result, {}, position);
  }
}
