// The find bar's module loads on first use. A page or a PDF opened while it loads gets no finder:
// the find bar only searches Lectern's own document. A file of its own, as the module is held back
// for every test in it.
import { describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, rendered } from "./helpers";

const PAGE = "C:\\V\\artifacts\\report.html";
const PDF = "C:\\V\\artifacts\\brief.pdf";

/** Holds `find.ts` back until the test lets it load. */
const gate = vi.hoisted(() => {
  let open = (): void => undefined;
  const loaded = new Promise<void>((resolve) => {
    open = resolve;
  });
  return {
    loaded,
    open: () => {
      open();
    },
  };
});

vi.mock("../src/find", async (importOriginal) => {
  await gate.loaded;
  return importOriginal();
});

async function started() {
  const fake = new FakeBackend(
    {
      ...fixtures({
        [PAGE]: rendered("report.html", ""),
        [PDF]: rendered("brief.pdf", ""),
      }),
      kinds: { [PAGE]: "html", [PDF]: "pdf" },
    },
    { initial: A },
  );
  const app = new App(fake, appRoot());
  await app.start();
  return app;
}

describe("find, while its module loads", () => {
  it("never opens over a page or a PDF opened meanwhile", async () => {
    const app = await started();
    for (const framed of [PDF, PAGE]) {
      await app.open(A);
      // A query with no match: jsdom can't measure ranges to scroll to one.
      const finding = app.actions.showFind("nowhere");
      await app.open(framed);
      gate.open();
      await finding;
      expect(document.querySelector(".find-bar:not([hidden])"), framed).toBeNull();
      expect(document.activeElement?.closest(".find-bar") ?? null, framed).toBeNull();
    }
    // Asked for on a note, and still on it once loaded, it opens.
    await app.open(A);
    await app.actions.showFind("nowhere");
    expect(document.querySelector(".find-bar:not([hidden])")).not.toBeNull();
  });
});
