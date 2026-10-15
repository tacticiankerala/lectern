// Text files, HTML pages and PDFs beside notes: what the reader shows for each kind, and the ⋯
// menu's "Open in default app", which only they have.
import { describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, rendered, settle } from "./helpers";

const PAGE = "C:\\V\\artifacts\\report.html";
const PDF = "C:\\V\\artifacts\\brief.pdf";
const TEXT = "C:\\V\\notes\\readme.txt";
/**
 * Where the fake serves the page and the PDF from: the dev server, as `lxpage` would, an open of
 * their own each (`/o/<token>/`), and for a page the HTML pages setting, for the dev server.
 */
const PAGE_URL = /^\/__page\/o\/t\d+-[0-9a-f]+\/report\.html\?lxmode=full$/;
const PDF_URL = /^\/__page\/o\/t\d+-[0-9a-f]+\/brief\.pdf$/;

/** The app with a page, a PDF and a text file beside the notes, and `initial` open. */
async function started(initial: string) {
  const fake = new FakeBackend(
    {
      ...fixtures({
        [PAGE]: rendered("report.html", ""),
        [PDF]: rendered("brief.pdf", ""),
        [TEXT]: rendered("readme", "<p>Plain words.</p>"),
      }),
      kinds: { [PAGE]: "html", [PDF]: "pdf", [TEXT]: "text" },
    },
    { initial },
  );
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

/** The ⋯ menu's labels, once it shows. */
async function moreMenu(): Promise<HTMLButtonElement[]> {
  document.querySelector<HTMLButtonElement>("#lx-more-btn")?.click();
  await vi.waitFor(() => {
    expect(document.querySelector("#lx-overlay-root .menu")).not.toBeNull();
  });
  return [...document.querySelectorAll<HTMLButtonElement>("#lx-overlay-root .menu .menu-item")];
}

function label(item: HTMLElement): string {
  return item.querySelector(".menu-label")?.textContent ?? "";
}

/** The frame a page or a PDF shows in. */
function frame(): HTMLIFrameElement | null {
  return document.querySelector<HTMLIFrameElement>("#lx-doc-pane > iframe.lx-frame");
}

/** The frame's `?v=`, which every load changes. */
function version(): number {
  return Number(/\?v=(\d+)/.exec(frame()?.getAttribute("src") ?? "")?.[1] ?? NaN);
}

function press(init: KeyboardEventInit): void {
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
  );
}

function toasts(): string[] {
  return [...document.querySelectorAll("#lx-toasts .toast")].map((t) => t.textContent);
}

describe("documents that aren't notes", () => {
  it("open with their kind, and a page or a PDF with its frame's URL", async () => {
    const { app } = await started(PAGE);
    expect(app.state.doc?.kind).toBe("html");
    expect(app.state.doc?.frameUrl).toMatch(PAGE_URL);
    await app.open(PDF);
    expect(app.state.doc?.kind).toBe("pdf");
    expect(app.state.doc?.frameUrl).toMatch(PDF_URL);
    await app.open(TEXT);
    expect(app.state.doc?.kind).toBe("text");
    expect(app.state.doc?.frameUrl).toBeNull();
    await app.open(A);
    expect(app.state.doc?.kind).toBe("markdown");
    expect(app.state.doc?.frameUrl).toBeNull();
  });

  it("show a PDF in an unsandboxed frame filling the pane, and a page in a sandboxed one", async () => {
    const { app } = await started(PDF);
    const pane = app.layout.docPane;
    const doc = document.getElementById("lx-doc");
    expect(pane.classList.contains("framed")).toBe(true);
    expect(frame()?.hasAttribute("sandbox")).toBe(false);
    expect(frame()?.getAttribute("src")).toMatch(
      /^\/__page\/o\/t\d+-[0-9a-f]+\/brief\.pdf\?v=\d+#page=1$/,
    );
    expect(frame()?.title).toBe("brief.pdf");
    expect(doc?.childElementCount).toBe(0);
    await app.open(PAGE);
    expect(document.querySelectorAll("iframe")).toHaveLength(1);
    expect(frame()?.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame()?.getAttribute("src")).toMatch(
      /^\/__page\/o\/t\d+-[0-9a-f]+\/report\.html\?lxmode=full&v=\d+$/,
    );
    // A text file shows its own text, and the frame goes.
    await app.open(TEXT);
    expect(frame()).toBeNull();
    expect(pane.classList.contains("framed")).toBe(false);
    expect(doc?.textContent).toBe("Plain words.");
  });

  it("put the frame in the page, a page's in Lectern's colour scheme, before it loads anything", async () => {
    const src = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, "src");
    if (!src) throw new Error("no src property");
    const connected: [boolean, string][] = [];
    Object.defineProperty(HTMLIFrameElement.prototype, "src", {
      ...src,
      set(this: HTMLIFrameElement, value: string) {
        connected.push([this.isConnected, this.style.colorScheme]);
        src.set?.call(this, value);
      },
    });
    try {
      const { app } = await started(PDF);
      await app.open(PAGE);
    } finally {
      Object.defineProperty(HTMLIFrameElement.prototype, "src", src);
    }
    expect(connected).toEqual([
      [true, ""],
      [true, "light"],
    ]);
  });

  it("reload a PDF at a new URL on F5 and on a change on disk, at its first page", async () => {
    const { fake } = await started(PDF);
    const first = version();
    expect(first).toBeGreaterThan(0);
    press({ key: "F5" });
    await vi.waitFor(() => {
      expect(version()).toBeGreaterThan(first);
    });
    const second = version();
    fake.emit("doc-changed", { path: PDF });
    await vi.waitFor(() => {
      expect(version()).toBeGreaterThan(second);
    });
    expect(frame()?.getAttribute("src")).toMatch(/#page=1$/);
    expect(document.querySelectorAll("iframe")).toHaveLength(1);
  });

  it("leave the frame for an error or the welcome screen", async () => {
    const { app } = await started(PDF);
    await app.open("C:\\V\\artifacts\\gone.pdf");
    expect(frame()).toBeNull();
    expect(app.layout.docPane.classList.contains("framed")).toBe(false);
    expect(document.querySelector("#lx-doc .state")).not.toBeNull();
    await app.open(PDF);
    expect(frame()).not.toBeNull();
    app.nav.home();
    expect(frame()).toBeNull();
    expect(app.layout.docPane.classList.contains("framed")).toBe(false);
  });

  it("a PDF lists no outline here, finds nothing here, keeps no reading position and takes no comments", async () => {
    const { fake, app } = await started(PDF);
    const outline = () => document.getElementById("lx-outline-pane")?.textContent;
    expect(outline()).toBe("PDFs show their own outline in the viewer.");
    // Ctrl+F, and F3, say where to search instead of opening the find bar.
    press({ key: "f", ctrlKey: true });
    await vi.waitFor(() => {
      expect(toasts()).toEqual(["Click into the PDF and press Ctrl+F to search it."]);
    });
    press({ key: "F3" });
    await settle(20);
    expect(document.querySelector(".find-bar:not([hidden])")).toBeNull();
    expect((await fake.loadReview(PDF)).readOnly).toBe("PDFs don't take comments.");
    const save = vi.spyOn(fake, "savePosition");
    app.scroller.dispatchEvent(new Event("scroll"));
    await settle(450);
    await app.open(A, { push: true });
    expect(save).not.toHaveBeenCalled();
    // A note lists its outline again.
    expect(outline()).not.toContain("PDFs show");
    // Back to the PDF: it opens at its first page, with nothing to restore.
    const restore = vi.spyOn(app.view, "restore");
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(PDF);
    expect(frame()?.getAttribute("src")).toMatch(/#page=1$/);
    expect(restore).not.toHaveBeenCalled();
  });

  it("close the find bar when a PDF opens, and never step a finder over one; a page keeps it", async () => {
    const { app } = await started(A);
    const bar = () => document.querySelector<HTMLElement>(".find-bar");
    // A query with no match: jsdom can't measure ranges to scroll to one.
    await app.actions.showFind("nowhere");
    expect(bar()?.hidden).toBe(false);
    await app.open(PDF);
    expect(bar()?.hidden).toBe(true);
    // F3 and Shift+F3 say where to search, and don't bring the bar back.
    press({ key: "F3" });
    press({ key: "F3", shiftKey: true });
    await vi.waitFor(() => {
      expect(toasts()).toContain("Click into the PDF and press Ctrl+F to search it.");
    });
    expect(bar()?.hidden).toBe(true);
    await app.open(A);
    await app.actions.showFind("nowhere");
    expect(bar()?.hidden).toBe(false);
    // A page is searched through its text (pages.test.ts).
    await app.open(PAGE);
    expect(bar()?.hidden).toBe(false);
  });

  it("open a page or a text file in the editor, never a PDF", async () => {
    const { fake, app } = await started(PDF);
    const editor = vi.spyOn(fake, "openInEditor");
    expect(app.actions.run("open-editor")).toBe(false);
    press({ key: "e", ctrlKey: true });
    const items = await moreMenu();
    expect(label(items[0] ?? document.body)).toBe("Open in editor");
    expect(items[0]?.disabled).toBe(true);
    document.body.click();
    expect(editor).not.toHaveBeenCalled();
    for (const path of [PAGE, TEXT]) {
      await app.open(path);
      expect(app.actions.run("open-editor"), path).toBe(true);
      expect(editor).toHaveBeenLastCalledWith(path, undefined);
    }
  });

  it("⋯ offers Open in default app for a page, a PDF or a text file, never a note", async () => {
    const { fake, app } = await started(PAGE);
    const follow = vi.spyOn(fake, "follow");
    for (const path of [PAGE, PDF, TEXT]) {
      await app.open(path);
      const items = await moreMenu();
      expect(items.map(label).slice(0, 2), path).toEqual(["Open in editor", "Open in default app"]);
      items[1]?.click();
      expect(follow).toHaveBeenLastCalledWith({
        kind: "file",
        target: path,
        line: null,
        anchor: null,
      });
    }
    await app.open(A);
    expect((await moreMenu()).map(label)).not.toContain("Open in default app");
  });
});
