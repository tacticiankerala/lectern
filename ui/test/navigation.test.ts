import { describe, expect, it, vi } from "vitest";
import { App } from "../src/app";
import type { Candidate } from "../src/generated/Candidate";
import type { RecentEntry } from "../src/generated/RecentEntry";
import { A, B, C, GatedBackend, ROOT, appRoot, fixtures, settle } from "./helpers";

async function started(initial: string | null, recent: RecentEntry[] = []) {
  const fake = new GatedBackend(fixtures(), initial === null ? { recent } : { initial, recent });
  const app = new App(fake, appRoot());
  fake.startupGate.resolve();
  await app.start();
  return { fake, app };
}

function button(label: string): HTMLButtonElement {
  const el = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  if (!el) throw new Error(`no ${label} button`);
  return el;
}

function toasts(): string[] {
  return [...document.querySelectorAll("#lx-toasts .toast")].map((t) => t.textContent);
}

describe("App history", () => {
  it("records a pushed navigation once it lands somewhere else", async () => {
    const { app } = await started(A);
    expect(button("Back").disabled).toBe(true);
    await app.open(B, { push: true });
    expect(button("Back").disabled).toBe(false);
    // Opening the document already shown records nothing more.
    await app.open(B, { push: true });
    button("Back").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(A);
    });
    expect(button("Back").disabled).toBe(true);
    expect(button("Forward").disabled).toBe(false);
  });

  it("records nothing for a navigation that fails or is superseded", async () => {
    const { fake, app } = await started(A);
    vi.spyOn(fake, "openDocument").mockRejectedValueOnce(new Error("offline"));
    await app.open(B, { push: true });
    expect(app.state.doc?.path).toBe(A);
    expect(button("Back").disabled).toBe(true);
    fake.slowPath = B;
    const slow = app.open(B, { push: true });
    await app.open(C);
    fake.openGate.resolve();
    await slow;
    expect(app.state.doc?.path).toBe(C);
    expect(button("Back").disabled).toBe(true);
  });

  it("goes back on the mouse's back button, stopping WebView2's own navigation", async () => {
    const { app } = await started(A);
    await app.open(B, { push: true });
    const up = new MouseEvent("mouseup", { button: 3, bubbles: true, cancelable: true });
    document.getElementById("lx-doc")?.dispatchEvent(up);
    expect(up.defaultPrevented).toBe(true);
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(A);
    });
    const aux = new MouseEvent("auxclick", { button: 4, bubbles: true, cancelable: true });
    document.body.dispatchEvent(aux);
    expect(aux.defaultPrevented).toBe(true);
  });
});

describe("App recent files", () => {
  it("keeps the recent list current as documents open", async () => {
    const { app } = await started(null, [{ path: B, title: "Bee", openedMs: 0 }]);
    await app.open(A);
    await app.open(B.toUpperCase());
    expect(app.state.recent.map((r) => r.path.toLowerCase())).toEqual([
      B.toLowerCase(),
      A.toLowerCase(),
    ]);
  });
});

describe("App user paths", () => {
  it("opens a dropped file through openUserPath, pushing the history", async () => {
    const { fake, app } = await started(A);
    const user = vi.spyOn(fake, "openUserPath");
    fake.drop([B]);
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
    expect(user).toHaveBeenCalledWith(B);
    expect(button("Back").disabled).toBe(false);
  });

  it("adds a dropped folder to the library and says so", async () => {
    const { app, fake } = await started(A);
    fake.drop(["D:\\Elsewhere"]);
    await vi.waitFor(() => {
      expect(toasts()).toContain("Added D:\\Elsewhere to the library");
    });
    expect(app.state.library.roots.map((r) => r.path)).toContain("D:\\Elsewhere");
    expect(app.state.settings.libraryRoots).toContain("D:\\Elsewhere");
    // The document on screen stays: the folder has no README.
    expect(app.state.doc?.path).toBe(A);
  });

  it("opens the README of a folder inside a root without adding it", async () => {
    const readme = `${ROOT}\\notes\\README.md`;
    const fake = new GatedBackend(
      fixtures({
        [readme]: {
          html: "<h1>Notes</h1>",
          outline: [],
          frontmatter: null,
          tasks: { done: 0, total: 0 },
          title: "Notes",
          wordCount: 0,
          hasUnresolvedWikilinks: false,
        },
      }),
      { initial: A },
    );
    const app = new App(fake, appRoot());
    fake.startupGate.resolve();
    await app.start();
    const roots = app.state.library.roots.length;
    await app.openUserPath(`${ROOT}\\notes`);
    expect(app.state.doc?.path).toBe(readme);
    expect(app.state.library.roots).toHaveLength(roots);
    await settle();
    expect(toasts()).toEqual([]);
  });
});

function buttonNamed(text: string): HTMLButtonElement | undefined {
  return [...document.querySelectorAll<HTMLButtonElement>("#lx-doc button")].find(
    (b) => b.textContent === text,
  );
}

function press(init: KeyboardEventInit): void {
  (document.activeElement ?? document.body).dispatchEvent(
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
  );
}

/** Opens quick open with Ctrl+P and waits for its lazily loaded module. */
async function quickOpen(): Promise<HTMLInputElement> {
  press({ key: "p", ctrlKey: true });
  let input: HTMLInputElement | null = null;
  await vi.waitFor(() => {
    input = document.querySelector<HTMLInputElement>(".qo-backdrop:not([hidden]) input");
    expect(input).not.toBeNull();
  });
  return input as unknown as HTMLInputElement;
}

const candidate = (path: string): Candidate => ({
  path,
  name: path.slice(path.lastIndexOf("\\") + 1),
  rel: path.slice(ROOT.length + 1).replaceAll("\\", "/"),
  root: ROOT,
});

describe("App history, travelling", () => {
  it("steps back twice in a row from the travel in flight, committing only what landed", async () => {
    const { fake, app } = await started(A);
    await app.open(B, { push: true });
    await app.open(C, { push: true });
    fake.slowPath = B;
    button("Back").click();
    // The second Back goes on from B, the first one's target, though C is still on screen.
    button("Back").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(A);
    });
    fake.openGate.resolve();
    await settle();
    expect(app.state.doc?.path).toBe(A);
    expect(button("Back").disabled).toBe(true);
    button("Forward").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
    button("Forward").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(C);
    });
    expect(button("Forward").disabled).toBe(true);
  });

  it("keeps the entry of a back that another navigation superseded", async () => {
    const { fake, app } = await started(A);
    await app.open(B, { push: true });
    fake.slowPath = A;
    button("Back").click();
    await app.open(C, { push: true });
    fake.openGate.resolve();
    await settle();
    expect(app.state.doc?.path).toBe(C);
    button("Back").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
    button("Back").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(A);
    });
    expect(button("Back").disabled).toBe(true);
  });

  it("goes back from the welcome screen", async () => {
    const { app } = await started(A);
    await app.open(B, { push: true });
    await app.open("C:\\V\\gone.md", { push: true });
    buttonNamed("Remove from recent")?.click();
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-doc .welcome")).not.toBeNull();
    });
    expect(button("Back").disabled).toBe(false);
    button("Back").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
  });
});

describe("App quick open candidates", () => {
  it("fetches them once, and again after index-ready", async () => {
    const { fake } = await started(A);
    const fetches = vi.spyOn(fake, "quickOpenCandidates").mockResolvedValue([candidate(A)]);
    await quickOpen();
    press({ key: "Escape" });
    await quickOpen();
    press({ key: "Escape" });
    await vi.waitFor(() => {
      expect(fetches).toHaveBeenCalledTimes(1);
    });
    fake.emit("index-ready", ROOT);
    await quickOpen();
    await vi.waitFor(() => {
      expect(fetches).toHaveBeenCalledTimes(2);
    });
  });

  it("refreshes an open picker when the library changes, keeping the query", async () => {
    const { fake } = await started(A);
    const fetches = vi.spyOn(fake, "quickOpenCandidates").mockResolvedValue([candidate(A)]);
    const input = await quickOpen();
    input.value = "cmd";
    input.dispatchEvent(new Event("input"));
    await vi.waitFor(() => {
      expect(document.querySelector(".qo-empty")?.textContent).toBe("No matches");
    });
    fetches.mockResolvedValue([candidate(A), candidate(C)]);
    fake.emit("library-updated", await fake.getLibrary());
    await vi.waitFor(() => {
      expect([...document.querySelectorAll(".qo-name")].map((n) => n.textContent)).toEqual([
        "c.md",
      ]);
    });
    expect(input.value).toBe("cmd");
  });
});

describe("App metadata-only refresh", () => {
  it("updates the breadcrumbs when only they changed", async () => {
    const { fake, app } = await started(B);
    const body = document.getElementById("lx-doc")?.firstElementChild;
    const notes = () => app.state.doc?.breadcrumbs.find((c) => c.name === "notes");
    expect(notes()?.readme).toBeNull();
    const readme = `${ROOT}\\notes\\README.md`;
    fake.setDoc(readme, "<h1>Notes</h1>");
    fake.emit("doc-changed", { path: B });
    await vi.waitFor(() => {
      expect(notes()?.readme).toBe(readme);
    });
    // The body stayed as it was.
    expect(document.getElementById("lx-doc")?.firstElementChild).toBe(body);
    expect(
      [...document.querySelectorAll("#lx-breadcrumbs .crumb")].map((c) => c.textContent),
    ).toEqual(["V", "notes", "b.md"]);
  });
});

describe("App reading position and live reload", () => {
  const SAVED = { headingId: "bee", offset: 40, line: 1, fraction: 0.5 };

  it("goes back to a document's saved position, unless an anchor or line is asked for", async () => {
    const { fake, app } = await started(A);
    fake.setDoc(B, "<h1 id='bee' data-sourcepos='1:1-1:5'>Bee</h1>");
    await fake.savePosition(B, SAVED);
    const save = vi.spyOn(fake, "savePosition");
    const restore = vi.spyOn(app.view, "restore");
    const toLine = vi.spyOn(app.view, "scrollToLine");
    await app.open(B);
    expect(restore).toHaveBeenCalledWith(SAVED);
    // A goes back where it was left, as saved on the way to B.
    const left = save.mock.calls.find(([path]) => path === A)?.[1];
    expect(left).toBeDefined();
    await app.open(A);
    expect(restore).toHaveBeenLastCalledWith(left);
    restore.mockClear();
    await app.open(B, { line: 1 });
    expect(toLine).toHaveBeenCalledWith(1);
    expect(restore).not.toHaveBeenCalled();
  });

  it("saves the reading position in the document it leaves, and once scrolling stops", async () => {
    const { fake, app } = await started(A);
    const save = vi.spyOn(fake, "savePosition");
    await app.open(B, { push: true });
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0]?.[0]).toBe(A);
    app.scroller.dispatchEvent(new Event("scroll"));
    app.scroller.dispatchEvent(new Event("scroll"));
    await settle(450);
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]?.[0]).toBe(B);
  });

  it("notes Updated in the properties when the file changed on disk, quietly", async () => {
    const { fake, app } = await started(A);
    const note = () => document.querySelector("#lx-properties .props-updated")?.textContent;
    expect(note()).toBeUndefined();
    fake.setDoc(A, "<p>Changed on disk</p>");
    fake.emit("doc-changed", { path: A });
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-doc p")?.textContent).toBe("Changed on disk");
    });
    expect(note()).toMatch(/^Updated \d\d:\d\d$/);
    const first = app.state.updated;
    expect(first).not.toBeNull();
    expect(toasts()).toEqual([]);
    // Rust re-renders silently once the library index resolves the links: same file, new HTML.
    const doc = app.state.doc;
    if (!doc) throw new Error("no doc");
    fake.overrides[A] = { status: "ok", doc: { ...doc, html: "<p>Links resolved</p>" } };
    fake.emit("doc-changed", { path: A });
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-doc p")?.textContent).toBe("Links resolved");
    });
    expect(app.state.updated).toBe(first);
    // A change that leaves the body as it was refreshes the note too.
    fake.overrides = {};
    await settle(5);
    fake.setDoc(A, "<p>Links resolved</p>");
    fake.emit("doc-changed", { path: A });
    await vi.waitFor(() => {
      expect(app.state.updated).not.toBe(first);
    });
    expect(note()).toMatch(/^Updated \d\d:\d\d$/);
    // Another document carries no note.
    await app.open(B);
    expect(note()).toBeUndefined();
    expect(toasts()).toEqual([]);
  });
});
