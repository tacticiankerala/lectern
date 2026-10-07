// The Backend on Tauri: commands through `invoke` (arguments camelCased, as Rust expects them),
// events through `listen`, and the file and folder pickers through the dialog plugin.
import { invoke } from "@tauri-apps/api/core";
import { listen, type EventTarget as TauriTarget } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Backend, BackendEvent } from "./backend";
import type { Candidate } from "./generated/Candidate";
import type { FileHits } from "./generated/FileHits";
import type { FollowResult } from "./generated/FollowResult";
import type { FollowTarget } from "./generated/FollowTarget";
import type { LibraryPayload } from "./generated/LibraryPayload";
import { MARKDOWN_EXTENSIONS } from "./generated/markdown-extensions";
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

declare global {
  interface Window {
    /** Tauri's, set before any script runs; `getCurrentWebviewWindow` reads the label here. */
    __TAURI_INTERNALS__: { metadata: { currentWebview: { label: string } } };
  }
}

/**
 * This window's label ("main" or `win-<n>`), read where `getCurrentWebviewWindow` reads it: the
 * webview module would add to the bundle loaded before first paint.
 */
function thisLabel(): string {
  return window.__TAURI_INTERNALS__.metadata.currentWebview.label;
}

export class TauriBackend implements Backend {
  /** Listener registrations still on their way to Rust; `startup` waits for them. */
  private readonly registering: Promise<unknown>[] = [];
  /** The window this UI runs in; the window commands act on it alone. */
  private readonly label = thisLabel();
  /**
   * This window's own target, as `getCurrentWebviewWindow().listen` uses it: an event Rust sends
   * to another window, or a file dropped on one, never reaches this one. `listen`'s default
   * target, `Any`, would hear them all.
   */
  private readonly target: TauriTarget = { kind: "WebviewWindow", label: this.label };

  async startup(): Promise<StartupPayload> {
    // Rust sends launches held until startup as events, so every listener must be in place first.
    await Promise.all(this.registering);
    return invoke<StartupPayload>("startup");
  }

  openDocument(path: string): Promise<OpenResult> {
    return invoke("open_document", { path });
  }

  openUserPath(path: string): Promise<UserOpen> {
    return invoke("open_user_path", { path });
  }

  removeRecent(path: string): Promise<RecentEntry[]> {
    return invoke("remove_recent", { path });
  }

  /**
   * The window plugin's own `set_title` command, which `getCurrentWindow().setTitle` wraps:
   * importing the Window class would add about 14 KB to the bundle loaded before first paint.
   */
  setTitle(title: string): Promise<void> {
    return invoke("plugin:window|set_title", { label: this.label, value: title });
  }

  /** The window plugin's `set_fullscreen`, invoked directly for the same reason as `setTitle`. */
  setFullscreen(on: boolean): Promise<void> {
    return invoke("plugin:window|set_fullscreen", { label: this.label, value: on });
  }

  getLibrary(): Promise<LibraryPayload> {
    return invoke("get_library");
  }

  addRoot(path: string): Promise<LibraryPayload> {
    return invoke("add_root", { path });
  }

  removeRoot(path: string): Promise<LibraryPayload> {
    return invoke("remove_root", { path });
  }

  retryRoot(path: string): Promise<LibraryPayload> {
    return invoke("retry_root", { path });
  }

  quickOpenCandidates(): Promise<Candidate[]> {
    return invoke("quick_open_candidates");
  }

  search(query: string): Promise<FileHits[]> {
    return invoke("search", { query });
  }

  follow(target: FollowTarget): Promise<FollowResult> {
    return invoke("follow", { target });
  }

  revealInExplorer(path: string): Promise<void> {
    return invoke("reveal_in_explorer", { path });
  }

  openInEditor(path: string, line?: number): Promise<void> {
    return invoke("open_in_editor", { path, line: line ?? null });
  }

  loadReview(path: string): Promise<ReviewPayload> {
    return invoke("load_review", { path });
  }

  reviewOp(path: string, op: ReviewOp): Promise<ReviewPayload> {
    return invoke("review_op", { path, op });
  }

  getSettings(): Promise<SettingsSnapshot> {
    return invoke("get_settings");
  }

  setSettings(patch: SettingsPatch): Promise<SettingsSnapshot> {
    return invoke("set_settings", { patch });
  }

  savePosition(path: string, position: SavedPosition): Promise<void> {
    return invoke("save_position", { path, position });
  }

  setChromeColors(bg: string, fg: string, dark: boolean): Promise<void> {
    return invoke("set_chrome_colors", { bg, fg, dark });
  }

  listSystemFonts(): Promise<string[]> {
    return invoke("list_system_fonts");
  }

  /** Asks GitHub Releases for a newer Lectern (Rust's updater). */
  checkUpdate(): Promise<UpdateInfo | null> {
    return invoke("check_update");
  }

  /** Installs the update found and restarts, or, for a portable copy, opens the Releases page. */
  installUpdate(): Promise<void> {
    return invoke("install_update");
  }

  perfMark(name: string, ms?: number): void {
    invoke("perf_mark", { name, ms: ms ?? null }).catch((e: unknown) => {
      console.warn(e);
    });
  }

  pickFile(): Promise<string | null> {
    return open({
      multiple: false,
      directory: false,
      filters: [{ name: "Markdown", extensions: [...MARKDOWN_EXTENSIONS] }],
    });
  }

  pickFolder(): Promise<string | null> {
    return open({ multiple: false, directory: true });
  }

  showWindow(): Promise<void> {
    return invoke("show_window");
  }

  listWorkspaces(): Promise<WorkspaceSummary[]> {
    return invoke("list_workspaces");
  }

  suggestWorkspaceName(): Promise<string> {
    return invoke("suggest_workspace_name");
  }

  newWindow(): Promise<void> {
    return invoke("new_window");
  }

  openWorkspace(id: string, where: OpenWhere): Promise<WorkspaceOutcome> {
    return invoke("open_workspace", { id, where });
  }

  createWorkspace(name: string, where: OpenWhere, root?: string): Promise<WorkspaceOutcome> {
    return invoke("create_workspace", { name, where, root: root ?? null });
  }

  renameWorkspace(id: string, name: string): Promise<WorkspaceSummary[]> {
    return invoke("rename_workspace", { id, name });
  }

  deleteWorkspace(id: string): Promise<WorkspaceSummary[]> {
    return invoke("delete_workspace", { id });
  }

  setWorkspaceTheme(own: boolean): Promise<SettingsSnapshot> {
    return invoke("set_workspace_theme", { own });
  }

  quit(force: boolean): Promise<string[]> {
    return invoke("quit", { force });
  }

  setUnsaved(on: boolean): Promise<void> {
    return invoke("set_unsaved", { on });
  }

  /**
   * The window's drop event, which `getCurrentWebviewWindow().onDragDropEvent` wraps, heard
   * through `listen` on this window's target: the webview module would add to the bundle loaded
   * before first paint. Not awaited by `startup`, as nothing is dropped before the window shows.
   */
  onDragDrop(cb: (paths: string[]) => void): () => void {
    const unlisten = listen<{ paths: string[] }>(
      "tauri://drag-drop",
      (e) => {
        cb(e.payload.paths);
      },
      { target: this.target },
    );
    return () => {
      void unlisten.then((stop) => {
        stop();
      });
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- callers name the payload type
  on<T>(event: BackendEvent, cb: (payload: T) => void): () => void {
    const unlisten = listen<T>(
      event,
      (e) => {
        cb(e.payload);
      },
      { target: this.target },
    );
    this.registering.push(unlisten);
    return () => {
      void unlisten.then((stop) => {
        stop();
      });
    };
  }
}
