// Text files, HTML pages and PDFs beside notes: what the reader shows for each kind, and the ⋯
// menu's "Open in default app", which only they have.
import { describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, rendered } from "./helpers";

const PAGE = "C:\\V\\artifacts\\report.html";
const PDF = "C:\\V\\artifacts\\brief.pdf";
const TEXT = "C:\\V\\notes\\readme.txt";

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

describe("documents that aren't notes", () => {
  it("open with their kind", async () => {
    const { app } = await started(PAGE);
    expect(app.state.doc?.kind).toBe("html");
    expect(app.state.doc?.frameUrl).toBeNull();
    await app.open(TEXT);
    expect(app.state.doc?.kind).toBe("text");
    await app.open(A);
    expect(app.state.doc?.kind).toBe("markdown");
  });

  it("show a page or a PDF as coming, until its viewer lands", async () => {
    const { app } = await started(PAGE);
    const doc = () => document.getElementById("lx-doc");
    expect(doc()?.querySelector(".state")?.textContent).toBe("Coming in this branch");
    await app.open(PDF);
    expect(doc()?.querySelector(".state")?.textContent).toBe("Coming in this branch");
    // A text file shows its own text.
    await app.open(TEXT);
    expect(doc()?.querySelector(".state")).toBeNull();
    expect(doc()?.textContent).toBe("Plain words.");
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
