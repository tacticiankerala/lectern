// What the UI needs from the host: the Tauri app (backend-tauri.ts) or, for Playwright, a fake
// that serves pre-rendered fixtures (dev/backend-fake.ts).
import type { Candidate } from "./generated/Candidate";
import type { FileHits } from "./generated/FileHits";
import type { FollowResult } from "./generated/FollowResult";
import type { FollowTarget } from "./generated/FollowTarget";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { OpenResult } from "./generated/OpenResult";
import type { RecentEntry } from "./generated/RecentEntry";
import type { SavedPosition } from "./generated/SavedPosition";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { StartupPayload } from "./generated/StartupPayload";
import type { UpdateInfo } from "./generated/UpdateInfo";
import type { UserOpen } from "./generated/UserOpen";

export type BackendEvent =
  | "doc-changed"
  | "doc-removed"
  | "open-request"
  | "library-updated"
  | "index-ready"
  | "update-available";

export interface Backend {
  /** Waits for every listener registered with `on` so far, then asks for the startup payload. */
  startup(): Promise<StartupPayload>;
  openDocument(path: string): Promise<OpenResult>;
  getLibrary(): Promise<LibraryPayload>;
  addRoot(path: string): Promise<LibraryPayload>;
  removeRoot(path: string): Promise<LibraryPayload>;
  retryRoot(path: string): Promise<LibraryPayload>;
  quickOpenCandidates(): Promise<Candidate[]>;
  search(query: string): Promise<FileHits[]>;
  follow(target: FollowTarget): Promise<FollowResult>;
  revealInExplorer(path: string): Promise<void>;
  openInEditor(path: string, line?: number): Promise<void>;
  getSettings(): Promise<Settings>;
  setSettings(patch: SettingsPatch): Promise<Settings>;
  savePosition(path: string, position: SavedPosition): Promise<void>;
  setChromeColors(bg: string, fg: string, dark: boolean): Promise<void>;
  listSystemFonts(): Promise<string[]>;
  checkUpdate(): Promise<UpdateInfo | null>;
  installUpdate(): Promise<void>;
  /** Fire and forget; Rust ignores marks when no perf log is configured. */
  perfMark(name: string, ms?: number): void;
  pickFile(): Promise<string | null>;
  pickFolder(): Promise<string | null>;
  /**
   * Opens a file or folder the user explicitly chose (the file dialog, Add folder or a drop) as a
   * launch argument would: Rust trusts its network host for the session; a file opens like
   * `openDocument`; a folder joins the library unless it nests with a root, and opens its README
   * when it has one.
   */
  openUserPath(path: string): Promise<UserOpen>;
  /** Drops `path` from the recent files, for good; returns the recent files left. */
  removeRecent(path: string): Promise<RecentEntry[]>;
  /** Sets the native window title (the title bar and the task switcher). */
  setTitle(title: string): Promise<void>;
  /** Puts the window in or out of full screen, for focus mode. */
  setFullscreen(on: boolean): Promise<void>;
  showWindow(): Promise<void>;
  /** Calls `cb` with the paths of files or folders dropped on the window; returns the unsubscribe. */
  onDragDrop(cb: (paths: string[]) => void): () => void;
  /** Subscribes to a backend event; returns the unsubscribe function. */
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- callers name the payload type
  on<T>(event: BackendEvent, cb: (payload: T) => void): () => void;
}
