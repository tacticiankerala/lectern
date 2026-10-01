// A Backend that serves the fixtures `export_fixtures` rendered (ui/dev/fixtures.json), so the
// UI runs in a plain browser for Playwright. Tests drive it through `window.__fake`.
import type { Backend, BackendEvent } from "../src/backend";
import { DEFAULT_SETTINGS } from "../src/app";
import type { Candidate } from "../src/generated/Candidate";
import type { Crumb } from "../src/generated/Crumb";
import type { DocPayload } from "../src/generated/DocPayload";
import type { FileHits } from "../src/generated/FileHits";
import type { FollowResult } from "../src/generated/FollowResult";
import type { FollowTarget } from "../src/generated/FollowTarget";
import type { LibraryPayload } from "../src/generated/LibraryPayload";
import type { OpenResult } from "../src/generated/OpenResult";
import type { RecentEntry } from "../src/generated/RecentEntry";
import type { RenderedDoc } from "../src/generated/RenderedDoc";
import type { Settings } from "../src/generated/Settings";
import type { SettingsPatch } from "../src/generated/SettingsPatch";
import type { StartupPayload } from "../src/generated/StartupPayload";
import type { TreeNode } from "../src/generated/TreeNode";
import type { UpdateInfo } from "../src/generated/UpdateInfo";

export interface Fixtures {
  /** The fake library root, `C:\Fixtures\vault`. */
  root: string;
  tree: TreeNode;
  candidates: Candidate[];
  /** By path. */
  docs: Record<string, RenderedDoc>;
}

export interface FakeOptions {
  /** A document "given on the command line": startup renders it as `initial`. */
  initial?: string;
  recent?: RecentEntry[];
}

export interface PerfMark {
  name: string;
  ms?: number;
}

/** What tests reach through `window.__fake`. */
export interface FakeControl {
  emit(event: BackendEvent, payload: unknown): void;
  /** Replaces a document's HTML, adds a document, or (with null) deletes one. */
  setDoc(path: string, html: string | null): void;
  readonly marks: PerfMark[];
}

declare global {
  interface Window {
    __fake: FakeControl;
  }
}

/** Paths compare as on Windows: case-insensitively, either separator. */
function key(path: string): string {
  return path.toLowerCase().replaceAll("/", "\\");
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("\\") + 1);
}

export class FakeBackend implements Backend, FakeControl {
  readonly marks: PerfMark[] = [];
  readonly shown: number[] = [];
  /** Every native title set, in order. */
  readonly titles: string[] = [];
  private readonly docs = new Map<string, { path: string; doc: RenderedDoc }>();
  private readonly listeners = new Map<BackendEvent, Set<(payload: unknown) => void>>();
  private settings: Settings = { ...DEFAULT_SETTINGS };
  private readonly library: LibraryPayload;
  private recent: RecentEntry[];

  constructor(
    private readonly fixtures: Fixtures,
    private readonly options: FakeOptions = {},
  ) {
    this.recent = options.recent ?? [];
    for (const [path, doc] of Object.entries(fixtures.docs)) {
      this.docs.set(key(path), { path, doc });
    }
    this.library = {
      roots: [
        {
          path: fixtures.root,
          name: fixtures.tree.name,
          state: { state: "ready" },
          tree: fixtures.tree,
        },
      ],
    };
  }

  startup(): Promise<StartupPayload> {
    const initial = this.options.initial;
    return Promise.resolve({
      settings: this.settings,
      library: this.library,
      recent: this.recent,
      initial: initial === undefined ? null : this.open(initial),
      version: "0.0.0-fake",
      portable: false,
      startupNotice: null,
    });
  }

  openDocument(path: string): Promise<OpenResult> {
    return Promise.resolve(this.open(path));
  }

  openUserPath(path: string): Promise<OpenResult> {
    return this.openDocument(path);
  }

  removeRecent(path: string): Promise<RecentEntry[]> {
    this.recent = this.recent.filter((entry) => key(entry.path) !== key(path));
    return Promise.resolve(this.recent);
  }

  setTitle(title: string): Promise<void> {
    this.titles.push(title);
    document.title = title;
    return Promise.resolve();
  }

  getLibrary(): Promise<LibraryPayload> {
    return Promise.resolve(this.library);
  }

  addRoot(): Promise<LibraryPayload> {
    return this.getLibrary();
  }

  removeRoot(): Promise<LibraryPayload> {
    return this.getLibrary();
  }

  retryRoot(): Promise<LibraryPayload> {
    return this.getLibrary();
  }

  quickOpenCandidates(): Promise<Candidate[]> {
    return Promise.resolve(this.fixtures.candidates);
  }

  search(): Promise<FileHits[]> {
    return Promise.resolve([]);
  }

  /** `doc` targets open, with the anchor passed through as written, as Rust does. */
  follow(target: FollowTarget): Promise<FollowResult> {
    if (target.kind === "doc") {
      return Promise.resolve({
        action: "openDoc",
        path: target.target,
        anchor: target.anchor,
        line: target.line,
      });
    }
    return Promise.resolve({ action: "notFound", message: `Couldn't find ${target.target}` });
  }

  revealInExplorer(): Promise<void> {
    return Promise.resolve();
  }

  openInEditor(): Promise<void> {
    return Promise.resolve();
  }

  getSettings(): Promise<Settings> {
    return Promise.resolve(this.settings);
  }

  setSettings(patch: SettingsPatch): Promise<Settings> {
    const defined = Object.entries(patch).filter(([, v]) => v != null);
    this.settings = { ...this.settings, ...Object.fromEntries(defined) };
    return Promise.resolve(this.settings);
  }

  savePosition(): Promise<void> {
    return Promise.resolve();
  }

  setChromeColors(): Promise<void> {
    return Promise.resolve();
  }

  listSystemFonts(): Promise<string[]> {
    return Promise.resolve([]);
  }

  checkUpdate(): Promise<UpdateInfo | null> {
    return Promise.resolve(null);
  }

  installUpdate(): Promise<void> {
    return Promise.resolve();
  }

  perfMark(name: string, ms?: number): void {
    this.marks.push(ms === undefined ? { name } : { name, ms });
  }

  pickFile(): Promise<string | null> {
    return Promise.resolve(null);
  }

  pickFolder(): Promise<string | null> {
    return Promise.resolve(null);
  }

  showWindow(): Promise<void> {
    this.shown.push(this.marks.length);
    return Promise.resolve();
  }

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- callers name the payload type
  on<T>(event: BackendEvent, cb: (payload: T) => void): () => void {
    const set = this.listeners.get(event) ?? new Set();
    this.listeners.set(event, set);
    const listener = cb as (payload: unknown) => void;
    set.add(listener);
    return () => {
      set.delete(listener);
    };
  }

  emit(event: BackendEvent, payload: unknown): void {
    for (const listener of [...(this.listeners.get(event) ?? [])]) {
      listener(payload);
    }
  }

  setDoc(path: string, html: string | null): void {
    const k = key(path);
    if (html === null) {
      this.docs.delete(k);
      return;
    }
    const old = this.docs.get(k);
    const doc: RenderedDoc = old
      ? { ...old.doc, html }
      : {
          html,
          outline: [],
          frontmatter: null,
          tasks: { done: 0, total: 0 },
          title: baseName(path).replace(/\.(md|markdown)$/i, ""),
          wordCount: 0,
          hasUnresolvedWikilinks: false,
        };
    this.docs.set(k, { path: old?.path ?? path, doc });
  }

  private open(path: string): OpenResult {
    const found = this.docs.get(key(path));
    if (!found) {
      return { status: "err", error: { kind: "notFound", message: `Couldn't find ${path}`, path } };
    }
    return { status: "ok", doc: this.payload(found.path, found.doc) };
  }

  private payload(path: string, doc: RenderedDoc): DocPayload {
    return {
      path,
      title: doc.title,
      html: doc.html,
      outline: doc.outline,
      frontmatter: doc.frontmatter,
      tasks: doc.tasks,
      wordCount: doc.wordCount,
      mtimeMs: 0,
      lossy: false,
      position: null,
      breadcrumbs: this.breadcrumbs(path),
      rootPath: this.fixtures.root,
    };
  }

  /** The root, each folder below it, then the file; folders link their README when it exists. */
  private breadcrumbs(path: string): Crumb[] {
    const root = this.fixtures.root;
    const file: Crumb = { name: baseName(path), path, readme: null };
    if (!key(path).startsWith(key(root) + "\\")) {
      return [file];
    }
    const folder = (name: string, dir: string): Crumb => {
      const readme = `${dir}\\README.md`;
      return { name, path: dir, readme: this.docs.has(key(readme)) ? readme : null };
    };
    const crumbs = [folder(this.fixtures.tree.name, root)];
    let dir = root;
    for (const part of path
      .slice(root.length + 1)
      .split("\\")
      .slice(0, -1)) {
      dir = `${dir}\\${part}`;
      crumbs.push(folder(part, dir));
    }
    crumbs.push(file);
    return crumbs;
  }
}
