// What the UI needs from the host: the Tauri app (backend-tauri.ts) or, for Playwright, a fake
// that serves pre-rendered fixtures (dev/backend-fake.ts).
import type { Candidate } from "./generated/Candidate";
import type { FileHits } from "./generated/FileHits";
import type { FollowResult } from "./generated/FollowResult";
import type { FollowTarget } from "./generated/FollowTarget";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { OpenResult } from "./generated/OpenResult";
import type { OpenWhere } from "./generated/OpenWhere";
import type { RecentEntry } from "./generated/RecentEntry";
import type { ReviewOp } from "./generated/ReviewOp";
import type { ReviewPayload } from "./generated/ReviewPayload";
import type { SavedPosition } from "./generated/SavedPosition";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { SettingsSnapshot } from "./generated/SettingsSnapshot";
import type { StartupPayload } from "./generated/StartupPayload";
import type { UpdateInfo } from "./generated/UpdateInfo";
import type { UserOpen } from "./generated/UserOpen";
import type { WorkspaceOutcome } from "./generated/WorkspaceOutcome";
import type { WorkspaceSummary } from "./generated/WorkspaceSummary";

export type BackendEvent =
  | "doc-changed"
  | "doc-removed"
  | "open-request"
  | "library-updated"
  | "index-ready"
  | "review-changed"
  | "settings-changed"
  | "workspaces-changed"
  | "close-requested";

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
  /**
   * The review comments of the open note `path`, from its sidecar. Rejects with a message for the
   * reader.
   */
  loadReview(path: string): Promise<ReviewPayload>;
  /** Changes the open note's sidecar; answers with its review as it now is. */
  reviewOp(path: string, op: ReviewOp): Promise<ReviewPayload>;
  /** This window's settings, with their revision (`SettingsSnapshot`). */
  getSettings(): Promise<SettingsSnapshot>;
  /**
   * Changes settings; answers with this window's settings, with their revision. Rust tells the
   * window its settings (`settings-changed`) before it answers.
   */
  setSettings(patch: SettingsPatch): Promise<SettingsSnapshot>;
  savePosition(path: string, position: SavedPosition): Promise<void>;
  setChromeColors(bg: string, fg: string, dark: boolean): Promise<void>;
  listSystemFonts(): Promise<string[]>;
  /**
   * Asks GitHub Releases for a newer Lectern. `automatic`: the check after startup, which then
   * runs no more this session (`StartupPayload.primary`).
   */
  checkUpdate(automatic: boolean): Promise<UpdateInfo | null>;
  /**
   * Installs the update found (Lectern then exits and restarts), or, for a portable copy, opens the
   * Releases page. Unless `force`, an installed copy doesn't while another window holds comment text
   * that isn't saved yet: the answer names those windows, as `quit` does. Empty when it went ahead.
   */
  installUpdate(force: boolean): Promise<string[]>;
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
  /** Every workspace, in creation order, with this window's marked current. */
  listWorkspaces(): Promise<WorkspaceSummary[]>;
  /** The name a new workspace is offered ("Workspace 2", the first number free). */
  suggestWorkspaceName(): Promise<string>;
  /** Opens a blank window, which lists the workspaces. */
  newWindow(): Promise<void>;
  /**
   * Opens workspace `id` here (`reload`: this window turned to it, and its page must reload) or in
   * a new window (`opened`); a workspace another window shows brings that window forward
   * (`focused`). Rejects with a message for the reader.
   */
  openWorkspace(id: string, where: OpenWhere): Promise<WorkspaceOutcome>;
  /** Adds a workspace named `name`, holding the folder `root` when given, and opens it likewise. */
  createWorkspace(name: string, where: OpenWhere, root?: string): Promise<WorkspaceOutcome>;
  /** Renames workspace `id`; answers with the workspaces. Rejects with a message for the reader. */
  renameWorkspace(id: string, name: string): Promise<WorkspaceSummary[]>;
  /** Forgets workspace `id`, never its files; answers with the workspaces left. */
  deleteWorkspace(id: string): Promise<WorkspaceSummary[]>;
  /**
   * Gives this window's workspace a theme of its own (`own`), starting from the shared one, or has
   * it follow the shared theme again; answers with the window's settings, with their revision.
   */
  setWorkspaceTheme(own: boolean): Promise<SettingsSnapshot>;
  /**
   * Quits Lectern with every window open, for the next launch to bring them all back. Unless
   * `force`, another window holding comment text that isn't saved yet stops it: the answer names
   * those windows (by workspace, or "a blank window"). Empty when Lectern quits.
   */
  quit(force: boolean): Promise<string[]>;
  /** Tells Rust whether this window holds comment text that isn't saved yet. */
  setUnsaved(on: boolean): Promise<void>;
  /**
   * Closes this window once its unsaved comment text was let go (`close-requested`): it isn't
   * asked again.
   */
  closeWindow(): Promise<void>;
  /**
   * Stops every listener `on` and `onDragDrop` registered, before the page reloads: Rust keeps a
   * page's listeners until its window closes otherwise.
   */
  stopListening(): Promise<void>;
  /** Calls `cb` with the paths of files or folders dropped on the window; returns the unsubscribe. */
  onDragDrop(cb: (paths: string[]) => void): () => void;
  /** Subscribes to a backend event; returns the unsubscribe function. */
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- callers name the payload type
  on<T>(event: BackendEvent, cb: (payload: T) => void): () => void;
}
