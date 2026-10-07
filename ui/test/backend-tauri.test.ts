// The Tauri backend talks to its own window: its listeners are scoped to this webview window, its
// window commands name it, and its listeners stop before the page reloads.
import { beforeEach, describe, expect, it, vi } from "vitest";

type Stop = () => Promise<void>;

const tauri = vi.hoisted(() => {
  const stops: ReturnType<typeof vi.fn<Stop>>[] = [];
  return {
    invoke: vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>(() => Promise.resolve(null)),
    stops,
    listen: vi.fn<(event: string, cb: unknown, options?: unknown) => Promise<Stop>>(() => {
      const stop = vi.fn<Stop>(() => Promise.resolve());
      stops.push(stop);
      return Promise.resolve(stop);
    }),
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const { TauriBackend } = await import("../src/backend-tauri");

const LABEL = "win-3";
const TARGET = { kind: "WebviewWindow", label: LABEL };

beforeEach(() => {
  tauri.invoke.mockClear();
  tauri.listen.mockClear();
  tauri.stops.length = 0;
  window.__TAURI_INTERNALS__ = { metadata: { currentWebview: { label: LABEL } } };
});

describe("TauriBackend", () => {
  it("listens on its own window only", () => {
    const backend = new TauriBackend();
    backend.on("settings-changed", () => undefined);
    backend.on("close-requested", () => undefined);
    backend.onDragDrop(() => undefined);
    expect(tauri.listen.mock.calls.map(([event, , options]) => [event, options])).toEqual([
      ["settings-changed", { target: TARGET }],
      ["close-requested", { target: TARGET }],
      ["tauri://drag-drop", { target: TARGET }],
    ]);
  });

  it("sets its own window's title and full screen", async () => {
    const backend = new TauriBackend();
    await backend.setTitle("Plan — Work");
    await backend.setFullscreen(true);
    expect(tauri.invoke.mock.calls).toEqual([
      ["plugin:window|set_title", { label: LABEL, value: "Plan — Work" }],
      ["plugin:window|set_fullscreen", { label: LABEL, value: true }],
    ]);
  });

  it("passes the update and close flags", async () => {
    const backend = new TauriBackend();
    await backend.checkUpdate(true);
    await backend.installUpdate(false);
    await backend.closeWindow();
    expect(tauri.invoke.mock.calls).toEqual([
      ["check_update", { automatic: true }],
      ["install_update", { force: false }],
      ["close_window"],
    ]);
  });

  it("stops every listener before the page reloads, each once", async () => {
    const backend = new TauriBackend();
    const off = backend.on("doc-changed", () => undefined);
    backend.on("workspaces-changed", () => undefined);
    backend.onDragDrop(() => undefined);
    // One already stopped stays stopped.
    off();
    await vi.waitFor(() => {
      expect(tauri.stops[0]).toHaveBeenCalledTimes(1);
    });
    await backend.stopListening();
    expect(tauri.stops.map((stop) => stop.mock.calls.length)).toEqual([1, 1, 1]);
  });
});
