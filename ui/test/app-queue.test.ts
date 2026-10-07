import { describe, expect, it, vi } from "vitest";
import { App } from "../src/app";
import type { DocChanged } from "../src/generated/DocChanged";
import type { OpenRequest } from "../src/generated/OpenRequest";
import type { RecentEntry } from "../src/generated/RecentEntry";
import { A, B, C, GatedBackend, appRoot, fixtures, settle } from "./helpers";

function setup(initial: string | null, recent: RecentEntry[] = []) {
  const fake = new GatedBackend(fixtures(), initial === null ? { recent } : { initial, recent });
  const app = new App(fake, appRoot());
  const opened: (string | null)[] = [];
  app.on("doc", () => opened.push(app.state.doc?.path ?? null));
  return { fake, app, opened };
}

function docText(): string {
  return document.getElementById("lx-doc")?.textContent ?? "";
}

describe("App startup and opening", () => {
  it("queues open-request received during startup", async () => {
    const { fake, app, opened } = setup(A);
    const started = app.start();
    // Rust holds second launches until startup, then sends them as events.
    fake.emit("open-request", { path: B, t0Ms: null, folder: false } satisfies OpenRequest);
    fake.startupGate.resolve();
    await started;
    expect(opened).toEqual([A, B]);
    expect(app.state.doc?.path).toBe(B);
    expect(docText()).toBe("Bee");
  });

  it("applies only the last of several queued requests", async () => {
    const { fake, app, opened } = setup(A);
    const started = app.start();
    fake.emit("open-request", { path: B, t0Ms: null, folder: false } satisfies OpenRequest);
    fake.emit("open-request", { path: C, t0Ms: null, folder: false } satisfies OpenRequest);
    fake.startupGate.resolve();
    await started;
    expect(opened).toEqual([A, C]);
  });

  it("drops the startup document when a newer open started meanwhile", async () => {
    const { fake, app, opened } = setup(A);
    const started = app.start();
    await app.open(B);
    fake.startupGate.resolve();
    await started;
    expect(opened).toEqual([B]);
    expect(docText()).toBe("Bee");
  });

  it("drops an open response that a newer open superseded", async () => {
    const { fake, app, opened } = setup(null);
    fake.startupGate.resolve();
    await app.start();
    fake.slowPath = A;
    const slow = app.open(A);
    await app.open(B);
    fake.openGate.resolve();
    await slow;
    expect(opened).toEqual([null, B]);
    expect(docText()).toBe("Bee");
  });

  it("reloads the current document in place on doc-changed, ignoring other paths", async () => {
    const { fake, app } = setup(A);
    fake.startupGate.resolve();
    await app.start();
    const open = vi.spyOn(fake, "openDocument");
    fake.emit("doc-changed", { path: B } satisfies DocChanged);
    expect(open).not.toHaveBeenCalled();
    fake.setDoc(A, "<p>Changed</p>");
    fake.emit("doc-changed", { path: A.toUpperCase() } satisfies DocChanged);
    await vi.waitFor(() => {
      expect(docText()).toBe("Changed");
    });
  });

  it("shows the startup notice once, as a toast", async () => {
    const { fake, app } = setup(A);
    fake.notice = "Your settings were reset.";
    fake.startupGate.resolve();
    await app.start();
    const toasts = document.querySelectorAll("#lx-toasts .toast");
    expect([...toasts].map((t) => t.textContent)).toEqual(["Your settings were reset."]);
  });

  it("shows the welcome screen without a document", async () => {
    const { fake, app } = setup(null);
    fake.startupGate.resolve();
    await app.start();
    const welcome = document.querySelector("#lx-doc .welcome");
    expect(welcome?.textContent).toContain("Open file");
    expect(welcome?.textContent).toContain("Add folder");
    expect(document.title).toBe("Lectern");
  });

  it("marks first paint before showing the window, then times opens", async () => {
    const { fake, app } = setup(A);
    fake.startupGate.resolve();
    await app.start();
    expect(fake.marks.map((m) => m.name)).toEqual(["first-paint"]);
    expect(fake.shown).toEqual([1]);
    await app.open(B);
    expect(fake.marks[1]?.name).toBe("doc-switch");
    expect(typeof fake.marks[1]?.ms).toBe("number");
    fake.emit("open-request", {
      path: C,
      t0Ms: Date.now() - 5,
      folder: false,
    } satisfies OpenRequest);
    await vi.waitFor(() => {
      expect(fake.marks.map((m) => m.name)).toContain("warm-open");
    });
    expect(fake.marks.find((m) => m.name === "warm-open")?.ms).toBeGreaterThanOrEqual(5);
  });
});

describe("App error state", () => {
  it("reveals a binary file named like Markdown, which no default app would open", async () => {
    const { fake, app } = setup(null);
    const path = "C:\\V\\broken.md";
    fake.overrides[path] = { status: "err", error: { kind: "binary", message: "Not text", path } };
    fake.startupGate.resolve();
    await app.start();
    await app.open(path);
    const buttons = [...document.querySelectorAll("#lx-doc button")].map((b) => b.textContent);
    expect(buttons).toEqual(["Retry", "Remove from recent", "Reveal in Explorer"]);
    const reveal = vi.spyOn(fake, "revealInExplorer");
    const follow = vi.spyOn(fake, "follow");
    buttonNamed("Reveal in Explorer")?.click();
    expect(reveal).toHaveBeenCalledWith(path);
    expect(follow).not.toHaveBeenCalled();
  });

  it("shows a missing file with Retry, Search for it and Remove from recent", async () => {
    const { fake, app } = setup(null);
    fake.startupGate.resolve();
    await app.start();
    await app.open("C:\\V\\gone.md");
    const state = document.querySelector("#lx-doc .state-error");
    expect(state?.textContent).toContain("C:\\V\\gone.md");
    const buttons = [...(state?.querySelectorAll("button") ?? [])].map((b) => b.textContent);
    expect(buttons).toEqual(["Retry", "Search for it", "Remove from recent"]);
    expect(app.state.error?.kind).toBe("notFound");
    expect(app.state.doc).toBeNull();
    // Retry opens it again, which works once the file is back.
    fake.setDoc("C:\\V\\gone.md", "<p>Back</p>");
    state?.querySelector<HTMLButtonElement>("button")?.click();
    await vi.waitFor(() => {
      expect(docText()).toBe("Back");
    });
  });

  it("offers the default app for a binary file", async () => {
    const { fake, app } = setup(null);
    const path = "C:\\V\\photo.png";
    fake.overrides[path] = {
      status: "err",
      error: { kind: "binary", message: "Not text", path },
    };
    fake.startupGate.resolve();
    await app.start();
    await app.open(path);
    const follow = vi.spyOn(fake, "follow");
    const button = [...document.querySelectorAll<HTMLButtonElement>("#lx-doc button")].find(
      (b) => b.textContent === "Open with default app",
    );
    button?.click();
    expect(follow).toHaveBeenCalledWith({ kind: "file", target: path, line: null, anchor: null });
  });
});

function recentEntry(path: string, title: string): RecentEntry {
  return { path, title, openedMs: 0 };
}

function buttonNamed(text: string): HTMLButtonElement | undefined {
  return [...document.querySelectorAll<HTMLButtonElement>("#lx-doc button")].find(
    (b) => b.textContent === text,
  );
}

describe("App navigations and background refreshes", () => {
  it("never lets a refresh of the shown document cancel a pending navigation", async () => {
    const { fake, app } = setup(A);
    fake.startupGate.resolve();
    await app.start();
    fake.slowPath = B;
    const navigation = app.open(B);
    const open = vi.spyOn(fake, "openDocument");
    fake.emit("doc-changed", { path: A } satisfies DocChanged);
    fake.openGate.resolve();
    await navigation;
    await settle();
    expect(app.state.doc?.path).toBe(B);
    expect(docText()).toBe("Bee");
    // The navigation landed elsewhere, so A's refresh was dropped.
    expect(open.mock.calls.map(([path]) => path)).not.toContain(A);
  });

  it("applies a refresh of the document being opened once it lands", async () => {
    const { fake, app } = setup(A);
    fake.startupGate.resolve();
    await app.start();
    fake.slowPath = B;
    // Answered as B was when asked; then B changes before the answer arrives.
    const navigation = app.open(B);
    fake.setDoc(B, "<p>Bee, revised</p>");
    fake.emit("doc-changed", { path: B } satisfies DocChanged);
    fake.openGate.resolve();
    await navigation;
    await vi.waitFor(() => {
      expect(docText()).toBe("Bee, revised");
    });
  });

  it("sets the native title on every render, a title-only change included", async () => {
    const { fake, app } = setup(A);
    fake.startupGate.resolve();
    await app.start();
    expect(fake.titles.at(-1)).toBe("Aye — Lectern");
    await app.open("C:\\V\\gone.md");
    expect(fake.titles.at(-1)).toBe("Lectern");
    await app.open(A);
    expect(fake.titles.at(-1)).toBe("Aye — Lectern");
    // The same HTML with a new frontmatter title.
    const current = await fake.openDocument(A);
    if (current.status !== "ok") throw new Error("A didn't open");
    fake.overrides[A] = { status: "ok", doc: { ...current.doc, title: "Aye, renamed" } };
    fake.emit("doc-changed", { path: A } satisfies DocChanged);
    await vi.waitFor(() => {
      expect(fake.titles.at(-1)).toBe("Aye, renamed — Lectern");
    });
    expect(app.state.doc?.title).toBe("Aye, renamed");
  });
});

describe("App recent files", () => {
  it("removes a missing file from the recent files for good", async () => {
    const gone = "C:\\V\\gone.md";
    const { fake, app } = setup(null, [recentEntry(gone, "Gone"), recentEntry(A, "Aye")]);
    fake.startupGate.resolve();
    await app.start();
    await app.open(gone);
    const remove = vi.spyOn(fake, "removeRecent");
    buttonNamed("Remove from recent")?.click();
    expect(remove).toHaveBeenCalledWith(gone);
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-doc .welcome")).not.toBeNull();
    });
    const titles = [...document.querySelectorAll("#lx-doc .recent-title")].map(
      (t) => t.textContent,
    );
    expect(titles).toEqual(["Aye"]);
    expect(app.state.recent.map((r) => r.path)).toEqual([A]);
  });

  it("removes an entry from the welcome screen's recent list", async () => {
    const { fake, app } = setup(null, [recentEntry(A, "Aye"), recentEntry(B, "Bee")]);
    fake.startupGate.resolve();
    await app.start();
    const remove = vi.spyOn(fake, "removeRecent");
    document
      .querySelector<HTMLButtonElement>('#lx-doc [aria-label="Remove Aye from recent"]')
      ?.click();
    expect(remove).toHaveBeenCalledWith(A);
    await vi.waitFor(() => {
      const titles = [...document.querySelectorAll("#lx-doc .recent-title")].map(
        (t) => t.textContent,
      );
      expect(titles).toEqual(["Bee"]);
    });
  });
});
