// The Backend on Tauri: commands through `invoke` (arguments camelCased, as Rust expects them),
// events through `listen`, and the file and folder pickers through the dialog plugin.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Backend, BackendEvent } from "./backend";
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

/** The one window's label, as in tauri.conf.json. */
const MAIN_WINDOW = "main";

export class TauriBackend implements Backend {
  /** Listener registrations still on their way to Rust; `startup` waits for them. */
  private readonly registering: Promise<unknown>[] = [];

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
    return invoke("plugin:window|set_title", { label: MAIN_WINDOW, value: title });
  }

  /** The window plugin's `set_fullscreen`, invoked directly for the same reason as `setTitle`. */
  setFullscreen(on: boolean): Promise<void> {
    return invoke("plugin:window|set_fullscreen", { label: MAIN_WINDOW, value: on });
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

  getSettings(): Promise<Settings> {
    return invoke("get_settings");
  }

  setSettings(patch: SettingsPatch): Promise<Settings> {
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

  /** The updater commands arrive with Task 13. */
  checkUpdate(): Promise<UpdateInfo | null> {
    return invoke("check_update");
  }

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
      filters: [{ name: "Markdown", extensions: ["md", "markdown"] }],
    });
  }

  pickFolder(): Promise<string | null> {
    return open({ multiple: false, directory: true });
  }

  showWindow(): Promise<void> {
    return invoke("show_window");
  }

  /**
   * The webview's drop event, which `getCurrentWebview().onDragDropEvent` wraps, heard through
   * `listen`: the webview module would add to the bundle loaded before first paint. Not awaited
   * by `startup`, as nothing is dropped before the window shows.
   */
  onDragDrop(cb: (paths: string[]) => void): () => void {
    const unlisten = listen<{ paths: string[] }>("tauri://drag-drop", (e) => {
      cb(e.payload.paths);
    });
    return () => {
      void unlisten.then((stop) => {
        stop();
      });
    };
  }

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- callers name the payload type
  on<T>(event: BackendEvent, cb: (payload: T) => void): () => void {
    const unlisten = listen<T>(event, (e) => {
      cb(e.payload);
    });
    this.registering.push(unlisten);
    return () => {
      void unlisten.then((stop) => {
        stop();
      });
    };
  }
}
