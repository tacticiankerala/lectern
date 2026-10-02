// A Backend that serves the fixtures `export_fixtures` rendered (ui/dev/fixtures.json), so the
// UI runs in a plain browser for Playwright. Tests drive it through `window.__fake`.
//
// Full-text search matches the fixtures' Markdown, which `build.mjs --fake` inlines, as Rust does:
// smart case, line by line.
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
import type { SavedPosition } from "../src/generated/SavedPosition";
import type { SearchHit } from "../src/generated/SearchHit";
import type { Segment } from "../src/generated/Segment";
import type { Settings } from "../src/generated/Settings";
import type { SettingsPatch } from "../src/generated/SettingsPatch";
import type { StartupPayload } from "../src/generated/StartupPayload";
import type { TreeNode } from "../src/generated/TreeNode";
import type { UpdateInfo } from "../src/generated/UpdateInfo";
import type { UserOpen } from "../src/generated/UserOpen";

export interface Fixtures {
  /** The fake library root, `C:\Fixtures\vault`. */
  root: string;
  tree: TreeNode;
  candidates: Candidate[];
  /** By path. */
  docs: Record<string, RenderedDoc>;
  /** Each document's Markdown, by path; `build.mjs --fake` adds it. */
  sources?: Record<string, string>;
}

export interface FakeOptions {
  /** A document "given on the command line": startup renders it as `initial`. */
  initial?: string;
  recent?: RecentEntry[];
  /**
   * Keeps the settings and reading positions in sessionStorage, so they survive a page reload as
   * on disk.
   */
  persist?: boolean;
}

/** The fake library's second root, on a share that never answers. */
export const OFFLINE_ROOT = "\\\\offline-nas\\share\\notes";
const SETTINGS_KEY = "lx-fake-settings";
const POSITIONS_KEY = "lx-fake-positions";
/**
 * As Rust: matching lines returned per file and in all, and the context kept around a line's first
 * hit.
 */
const MAX_HITS_PER_FILE = 5;
const MAX_HITS = 500;
const CONTEXT_CHARS = 60;

export interface PerfMark {
  name: string;
  ms?: number;
}

/** What tests reach through `window.__fake`. */
export interface FakeControl {
  emit(event: BackendEvent, payload: unknown): void;
  /**
   * Replaces a document's HTML (as if the file changed on disk), adds a document, or (with null)
   * deletes one. `source` is its Markdown, for full-text search.
   */
  setDoc(path: string, html: string | null, source?: string): void;
  /** Deletes a document. */
  remove(path: string): void;
  /** The HTML a document renders to, or null when there is no such document. */
  html(path: string): string | null;
  /** Drops files or folders on the window. */
  drop(paths: string[]): void;
  /** Every root `retryRoot` was asked to retry, in order. */
  readonly retried: string[];
  readonly marks: PerfMark[];
  /** Every `setFullscreen` call, in order. */
  readonly fullscreen: boolean[];
  /** Every title-bar colouring, as `[bg, fg, dark]`, in order. */
  readonly chromeColors: [string, string, boolean][];
  /** What `checkUpdate` finds. */
  update: UpdateInfo | null;
  /** When set, `checkUpdate` fails with this message, as Rust's command would. */
  updateError: string | null;
  /** Every `checkUpdate` and `installUpdate` call, in order. */
  readonly updateCalls: ("check" | "install")[];
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

/** Whether `path` is `root` or below it. */
function isUnder(path: string, root: string): boolean {
  return key(path) === key(root) || key(path).startsWith(key(root) + "\\");
}

const MARKDOWN = /\.(md|markdown)$/i;

/**
 * A matching line's snippet, as Rust cuts it: up to 60 characters either side of the first hit,
 * with `…` where the line goes on, every hit inside marked.
 */
function segments(line: string, hits: RegExpExecArray[]): Segment[] {
  const first = hits[0]?.index ?? 0;
  const start = Math.max(0, first - CONTEXT_CHARS);
  const firstEnd = first + (hits[0]?.[0].length ?? 0);
  const end = Math.min(line.length, firstEnd + CONTEXT_CHARS);
  const out: Segment[] = [];
  let at = start;
  const text = (to: number): void => {
    if (to > at) out.push({ text: line.slice(at, to), hit: false });
  };
  for (const hit of hits) {
    const from = hit.index;
    const to = from + hit[0].length;
    if (from < start || to > end) continue;
    text(from);
    out.push({ text: hit[0], hit: true });
    at = to;
  }
  text(end);
  // Hits and the text between them alternate, so the ellipses join the text at either end.
  if (start > 0) {
    if (out[0] && !out[0].hit) out[0].text = `…${out[0].text}`;
    else out.unshift({ text: "…", hit: false });
  }
  const last = out[out.length - 1];
  if (end < line.length) {
    if (last && !last.hit) last.text += "…";
    else out.push({ text: "…", hit: false });
  }
  return out;
}

export class FakeBackend implements Backend, FakeControl {
  readonly marks: PerfMark[] = [];
  readonly shown: number[] = [];
  /** Every native title set, in order. */
  readonly titles: string[] = [];
  readonly fullscreen: boolean[] = [];
  readonly chromeColors: [string, string, boolean][] = [];
  readonly retried: string[] = [];
  update: UpdateInfo | null = null;
  updateError: string | null = null;
  readonly updateCalls: ("check" | "install")[] = [];
  /** What `listSystemFonts` answers. */
  systemFonts = ["Calibri", "Cascadia Code", "Constantia", "Segoe UI"];
  private readonly docs = new Map<string, { path: string; doc: RenderedDoc; mtimeMs: number }>();
  /** Saved reading positions, by path key. */
  private readonly positions = new Map<string, SavedPosition>();
  /** Each document's Markdown, by path key, for full-text search. */
  private readonly sources = new Map<string, string>();
  /** Stands in for modification times: bumped by every change. */
  private clock = 0;
  private readonly listeners = new Map<BackendEvent, Set<(payload: unknown) => void>>();
  private readonly drops = new Set<(paths: string[]) => void>();
  private settings: Settings = { ...DEFAULT_SETTINGS };
  private library: LibraryPayload;
  private recent: RecentEntry[];

  constructor(
    private readonly fixtures: Fixtures,
    private readonly options: FakeOptions = {},
  ) {
    this.recent = options.recent ?? [];
    for (const [path, doc] of Object.entries(fixtures.docs)) {
      this.docs.set(key(path), { path, doc, mtimeMs: 0 });
    }
    for (const [path, source] of Object.entries(fixtures.sources ?? {})) {
      this.sources.set(key(path), source);
    }
    this.library = {
      roots: [
        {
          path: fixtures.root,
          name: fixtures.tree.name,
          state: { state: "ready" },
          tree: fixtures.tree,
        },
        {
          path: OFFLINE_ROOT,
          name: baseName(OFFLINE_ROOT),
          state: { state: "unavailable", reason: `Couldn't reach ${OFFLINE_ROOT} within 3 s` },
          tree: null,
        },
      ],
    };
    if (options.persist) {
      try {
        const saved = sessionStorage.getItem(SETTINGS_KEY);
        if (saved !== null) {
          this.settings = { ...this.settings, ...(JSON.parse(saved) as Partial<Settings>) };
        }
        const positions = sessionStorage.getItem(POSITIONS_KEY);
        if (positions !== null) {
          for (const [k, p] of Object.entries(
            JSON.parse(positions) as Record<string, SavedPosition>,
          )) {
            this.positions.set(k, p);
          }
        }
      } catch {
        // Defaults, then.
      }
    }
  }

  startup(): Promise<StartupPayload> {
    const initial = this.options.initial;
    return Promise.resolve({
      settings: this.settings,
      library: structuredClone(this.library),
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

  /** As Rust decides: a file opens; a folder joins the library unless it nests with a root. */
  openUserPath(path: string): Promise<UserOpen> {
    if (MARKDOWN.test(path) || this.docs.has(key(path))) {
      return Promise.resolve({ doc: this.open(path), library: structuredClone(this.library) });
    }
    if (!this.library.roots.some((r) => isUnder(path, r.path) || isUnder(r.path, path))) {
      this.addFolder(path);
    }
    const readme = `${path}\\README.md`;
    return Promise.resolve({
      doc: this.docs.has(key(readme)) ? this.open(readme) : null,
      library: structuredClone(this.library),
    });
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

  setFullscreen(on: boolean): Promise<void> {
    this.fullscreen.push(on);
    return Promise.resolve();
  }

  getLibrary(): Promise<LibraryPayload> {
    return Promise.resolve(structuredClone(this.library));
  }

  addRoot(path: string): Promise<LibraryPayload> {
    if (!this.library.roots.some((r) => key(r.path) === key(path))) {
      this.addFolder(path);
    }
    return this.getLibrary();
  }

  removeRoot(path: string): Promise<LibraryPayload> {
    this.library.roots = this.library.roots.filter((r) => key(r.path) !== key(path));
    return this.getLibrary();
  }

  /** Records the retry; the root stays as it was. */
  retryRoot(path: string): Promise<LibraryPayload> {
    this.retried.push(path);
    return this.getLibrary();
  }

  /** A new root, still scanning: the fake never indexes it. */
  private addFolder(path: string): void {
    this.library.roots.push({
      path,
      name: baseName(path),
      state: { state: "scanning" },
      tree: null,
    });
  }

  quickOpenCandidates(): Promise<Candidate[]> {
    return Promise.resolve(this.fixtures.candidates);
  }

  /** As Rust: files whose name matches first, then by matching lines; at most 500 lines in all. */
  search(query: string): Promise<FileHits[]> {
    if (query.trim() === "" || /[\r\n]/.test(query)) {
      return Promise.resolve([]);
    }
    const source = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const flags = /\p{Uppercase}/u.test(query) ? "u" : "iu";
    // Global for every hit in a line; the file name is tested apart, as a global test would leave
    // `lastIndex` behind for the next file's lines.
    const re = new RegExp(source, `g${flags}`);
    const name = new RegExp(source, flags);
    const files = [...this.docs.values()].map(({ path, doc }) => {
      const text = this.sources.get(key(path));
      if (text === undefined) return null;
      const lines: SearchHit[] = [];
      let total = 0;
      text.split("\n").forEach((line, i) => {
        const hits = [...line.matchAll(re)];
        if (hits.length === 0) return;
        total++;
        if (lines.length < MAX_HITS_PER_FILE) {
          lines.push({ line: i + 1, segments: segments(line, hits) });
        }
      });
      if (total === 0) return null;
      const nameMatch = name.test(baseName(path).replace(/\.(md|markdown)$/i, ""));
      const rel = path.slice(this.fixtures.root.length + 1).replaceAll("\\", "/");
      return { path, title: doc.title, rel, nameMatch, hits: lines, total };
    });
    const found = files.filter((f) => f !== null);
    found.sort(
      (a, b) =>
        Number(b.nameMatch) - Number(a.nameMatch) ||
        b.total - a.total ||
        (a.path < b.path ? -1 : 1),
    );
    let budget = MAX_HITS;
    const results: FileHits[] = [];
    for (const file of found) {
      if (budget === 0) break;
      file.hits = file.hits.slice(0, budget);
      budget -= file.hits.length;
      results.push(file);
    }
    return Promise.resolve(results);
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
    if (this.options.persist) {
      try {
        sessionStorage.setItem(SETTINGS_KEY, JSON.stringify(this.settings));
      } catch {
        // Kept for this page only.
      }
    }
    return Promise.resolve(this.settings);
  }

  savePosition(path: string, position: SavedPosition): Promise<void> {
    this.positions.set(key(path), position);
    if (this.options.persist) {
      try {
        sessionStorage.setItem(POSITIONS_KEY, JSON.stringify(Object.fromEntries(this.positions)));
      } catch {
        // Kept for this page only.
      }
    }
    return Promise.resolve();
  }

  setChromeColors(bg: string, fg: string, dark: boolean): Promise<void> {
    this.chromeColors.push([bg, fg, dark]);
    return Promise.resolve();
  }

  listSystemFonts(): Promise<string[]> {
    return Promise.resolve(this.systemFonts);
  }

  checkUpdate(): Promise<UpdateInfo | null> {
    this.updateCalls.push("check");
    if (this.updateError !== null) {
      // Rust's commands fail with a message.
      // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
      return Promise.reject(this.updateError);
    }
    return Promise.resolve(this.update);
  }

  installUpdate(): Promise<void> {
    this.updateCalls.push("install");
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

  onDragDrop(cb: (paths: string[]) => void): () => void {
    this.drops.add(cb);
    return () => {
      this.drops.delete(cb);
    };
  }

  drop(paths: string[]): void {
    for (const cb of [...this.drops]) {
      cb(paths);
    }
  }

  emit(event: BackendEvent, payload: unknown): void {
    for (const listener of [...(this.listeners.get(event) ?? [])]) {
      listener(payload);
    }
  }

  setDoc(path: string, html: string | null, source?: string): void {
    const k = key(path);
    if (source !== undefined) {
      this.sources.set(k, source);
    }
    if (html === null) {
      this.docs.delete(k);
      this.sources.delete(k);
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
    this.docs.set(k, { path: old?.path ?? path, doc, mtimeMs: ++this.clock });
  }

  remove(path: string): void {
    this.setDoc(path, null);
  }

  html(path: string): string | null {
    return this.docs.get(key(path))?.doc.html ?? null;
  }

  private open(path: string): OpenResult {
    const found = this.docs.get(key(path));
    if (!found) {
      return { status: "err", error: { kind: "notFound", message: `Couldn't find ${path}`, path } };
    }
    return { status: "ok", doc: this.payload(found.path, found.doc, found.mtimeMs) };
  }

  private payload(path: string, doc: RenderedDoc, mtimeMs: number): DocPayload {
    return {
      path,
      title: doc.title,
      html: doc.html,
      outline: doc.outline,
      frontmatter: doc.frontmatter,
      tasks: doc.tasks,
      wordCount: doc.wordCount,
      mtimeMs,
      lossy: false,
      position: this.positions.get(key(path)) ?? null,
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
