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
  it("sends an HTML page's text with its review commands, and none for a note", async () => {
    const backend = new TauriBackend();
    const page = [{ text: "Results", headingLevel: 2 }, { text: "Conversion dipped." }];
    const op = { op: "reply", id: 1, text: "Weekly." } as const;
    await backend.loadReview("C:\\V\\a.md");
    await backend.loadReview("C:\\V\\report.html", page);
    await backend.reviewOp("C:\\V\\report.html", op, page);
    await backend.reviewOp("C:\\V\\a.md", op);
    expect(tauri.invoke.mock.calls).toEqual([
      ["load_review", { path: "C:\\V\\a.md", page: null }],
      ["load_review", { path: "C:\\V\\report.html", page }],
      ["review_op", { path: "C:\\V\\report.html", op, page }],
      ["review_op", { path: "C:\\V\\a.md", op, page: null }],
    ]);
  });

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

  it("numbers its opens, and tells Rust whether it left its document, lost it, or keeps it", async () => {
    const backend = new TauriBackend();
    await backend.openDocument("C:\\V\\a.md", 1);
    await backend.openUserPath("C:\\V\\notes", 2);
    await backend.leaveDocument(false, 3);
    await backend.leaveDocument(true, 4);
    await backend.keepDocument("C:\\V\\a.md", 5);
    expect(tauri.invoke.mock.calls).toEqual([
      ["open_document", { path: "C:\\V\\a.md", generation: 1 }],
      ["open_user_path", { path: "C:\\V\\notes", generation: 2 }],
      ["leave_document", { missing: false, generation: 3 }],
      ["leave_document", { missing: true, generation: 4 }],
      ["keep_document", { path: "C:\\V\\a.md", generation: 5 }],
    ]);
  });

  it("tells Rust where the keyboard focus is, and hears the keys and loads of frames", async () => {
    const backend = new TauriBackend();
    await backend.setFrameFocus("page");
    await backend.setFrameFocus("none");
    expect(tauri.invoke.mock.calls).toEqual([
      ["set_frame_focus", { focus: "page" }],
      ["set_frame_focus", { focus: "none" }],
    ]);
    backend.on("frame-key", () => undefined);
    backend.on("page-loads", () => undefined);
    expect(tauri.listen.mock.calls.map(([event, , options]) => [event, options])).toEqual([
      ["frame-key", { target: TARGET }],
      ["page-loads", { target: TARGET }],
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

  it("offers every kind of document in the file picker", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    await new TauriBackend().pickFile();
    expect(vi.mocked(open).mock.calls).toEqual([
      [
        {
          multiple: false,
          directory: false,
          filters: [
            {
              name: "Documents",
              extensions: ["md", "markdown", "mdown", "mkd", "txt", "html", "htm", "pdf"],
            },
          ],
        },
      ],
    ]);
  });
});
