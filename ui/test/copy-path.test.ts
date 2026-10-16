// Copying the open file's path as Lectern holds it: Ctrl+Shift+C, the ⋯ menu and the library's
// menu. Links and selections in a note keep the browser's own right-click menu. And the WSL
// distribution setting, which decides where Linux paths in notes open.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, rendered } from "./helpers";

const PLAN = "S:\\Notes\\My Vault\\plan.md";

const HTML = `
<h1 id="title">Title</h1>
<pre><code id="code">/home/me/projects/app/run.sh</code></pre>
<p>
<a href="#" class="code-link" data-kind="path" data-target="${PLAN}" id="to-plan"><code>plan.md</code></a>
<a href="#" class="link-doc" data-kind="doc" data-target="${PLAN}" id="to-doc">plan</a>
<a href="#" class="wikilink" data-kind="doc" data-target="${PLAN}" id="to-wiki">plan</a>
<a href="#" class="wikilink broken" data-kind="broken" title="No note named gone" id="to-gone">/home/me/gone.md</a>
</p>`;

let writeText: ReturnType<typeof vi.fn<(text: string) => Promise<void>>>;

beforeEach(() => {
  writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
});

afterEach(() => {
  document.getSelection()?.removeAllRanges();
  Reflect.deleteProperty(navigator, "clipboard");
});

/** The app, with `initial` open (null: no file). */
async function started(initial: string | null = A) {
  const fake = new FakeBackend(
    fixtures({ [A]: rendered("Aye", HTML) }),
    initial === null ? {} : { initial },
  );
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

function el(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`no #${id}`);
  return found;
}

/** Presses Ctrl+Shift+`key`; returns whether the app handled it. */
function press(key: "C" | "W" | "L"): boolean {
  const e = new KeyboardEvent("keydown", {
    key,
    ctrlKey: true,
    shiftKey: true,
    bubbles: true,
    cancelable: true,
  });
  document.body.dispatchEvent(e);
  return e.defaultPrevented;
}

/** Selects all the text of element `id`. */
function selectAll(id: string): void {
  const text = el(id).firstChild;
  if (!text) throw new Error(`no text in #${id}`);
  const range = document.createRange();
  range.setStart(text, 0);
  range.setEnd(text, text.textContent?.length ?? 0);
  const selection = document.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function lastToast(): string {
  return document.querySelector("#lx-toasts .toast:last-child")?.textContent ?? "";
}

async function copied(): Promise<string> {
  await vi.waitFor(() => {
    expect(writeText).toHaveBeenCalled();
  });
  return writeText.mock.lastCall?.[0] ?? "";
}

/** The popup menu (menu.ts), not the comments' own. */
const MENU = "#lx-overlay-root .menu";

/** The open menu's items, once it shows. */
async function menuItems(): Promise<HTMLButtonElement[]> {
  await vi.waitFor(() => {
    expect(document.querySelector(MENU)).not.toBeNull();
  });
  return [...document.querySelectorAll<HTMLButtonElement>(`${MENU} .menu-item`)];
}

function labels(items: HTMLElement[]): string[] {
  return items.map((item) => item.querySelector(".menu-label")?.textContent ?? "");
}

function rightClick(target: HTMLElement): MouseEvent {
  const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
  target.dispatchEvent(e);
  return e;
}

describe("Copy path", () => {
  it("copies the open file's path as Lectern holds it with Ctrl+Shift+C", async () => {
    await started();
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
    await vi.waitFor(() => {
      expect(lastToast()).toBe("Copied the path");
    });
  });

  it("copies the open file, whatever is selected or focused in the note", async () => {
    await started();
    selectAll("code");
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
    writeText.mockClear();
    document.getSelection()?.removeAllRanges();
    el("to-plan").focus();
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
  });

  it("leaves Ctrl+Shift+W and Ctrl+Shift+L alone", async () => {
    await started();
    expect(press("W")).toBe(false);
    expect(press("L")).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(writeText).not.toHaveBeenCalled();
  });

  it("does nothing with no file open", async () => {
    const { app } = await started(null);
    expect(app.state.doc).toBeNull();
    expect(press("C")).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(writeText).not.toHaveBeenCalled();
  });

  it("is in the ⋯ menu with its key, and copies the open file's path", async () => {
    await started();
    el("lx-more-btn").click();
    const items = await menuItems();
    const copy = items.filter((item) => item.textContent.startsWith("Copy"));
    expect(labels(copy)).toEqual(["Copy path"]);
    expect(copy[0]?.querySelector("kbd")?.textContent).toBe("Ctrl+Shift+C");
    expect(copy[0]?.disabled).toBe(false);
    copy[0]?.click();
    expect(await copied()).toBe(A);
    await vi.waitFor(() => {
      expect(lastToast()).toBe("Copied the path");
    });
  });

  it("is off in the ⋯ menu with no file open", async () => {
    await started(null);
    el("lx-more-btn").click();
    const items = await menuItems();
    const copy = items.find(
      (item) => item.querySelector(".menu-label")?.textContent === "Copy path",
    );
    expect(copy?.disabled).toBe(true);
  });

  it("is in the library's menu, and copies the row's path as stored", async () => {
    const { app } = await started();
    const folder = "S:\\Notes\\v1.0#archive";
    await app.actions.contextMenu(new MouseEvent("contextmenu"), folder, false);
    const items = await menuItems();
    expect(labels(items)).toEqual(["Reveal in Explorer", "Copy path"]);
    items[1]?.click();
    expect(await copied()).toBe(folder);
    await vi.waitFor(() => {
      expect(lastToast()).toBe("Copied the path");
    });
  });
});

describe("a right-click in a note", () => {
  it("leaves the browser's own menu on a link or a selected path", async () => {
    await started();
    for (const id of ["to-plan", "to-doc", "to-wiki", "to-gone"]) {
      expect(rightClick(el(id)).defaultPrevented, id).toBe(false);
    }
    selectAll("code");
    expect(rightClick(el("code")).defaultPrevented).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.querySelector(MENU)).toBeNull();
  });
});

describe("the WSL distribution in Preferences", () => {
  /** Preferences, open, with its WSL distribution picker once the distributions are listed. */
  async function picker(fake?: (fake: FakeBackend) => void) {
    const launched = await started();
    fake?.(launched.fake);
    expect(launched.app.actions.run("preferences")).toBe(true);
    const section = await vi.waitFor(() => {
      const found = [...document.querySelectorAll<HTMLElement>(".prefs-section")].find(
        (s) => s.querySelector("h4")?.textContent === "WSL distribution",
      );
      if (!found) throw new Error("no WSL distribution section");
      return found;
    });
    const select = section.querySelector<HTMLSelectElement>("select");
    if (!select) throw new Error("no WSL distribution picker");
    return { ...launched, section, select };
  }

  function options(select: HTMLSelectElement): [string, string][] {
    return [...select.options].map((o) => [o.value, o.textContent]);
  }

  it("offers Windows' default first, then each installed one, and saves the one chosen", async () => {
    const { fake, section, select } = await picker();
    await vi.waitFor(() => {
      expect(options(select)).toEqual([
        ["", "Windows default (Ubuntu)"],
        ["Debian", "Debian"],
        ["Ubuntu", "Ubuntu"],
      ]);
    });
    expect(section.hidden).toBe(false);
    expect(select.getAttribute("aria-label")).toBe("WSL distribution");
    expect(select.value).toBe("");
    select.value = "Debian";
    select.dispatchEvent(new Event("change"));
    await vi.waitFor(() => {
      expect(fake.sharedSettings().wslDistro).toBe("Debian");
    });
    // Back to Windows' default.
    select.value = "";
    select.dispatchEvent(new Event("change"));
    await vi.waitFor(() => {
      expect(fake.sharedSettings().wslDistro).toBeNull();
    });
  });

  it("shows a distribution chosen in another window", async () => {
    const { fake, select } = await picker();
    await vi.waitFor(() => {
      expect(select.options).toHaveLength(3);
    });
    fake.settingsElsewhere({ wslDistro: "Debian" });
    expect(select.value).toBe("Debian");
  });

  it("keeps a chosen distribution that is no longer installed in the list", async () => {
    const { select } = await picker((fake) => {
      fake.settingsElsewhere({ wslDistro: "Alpine" });
    });
    await vi.waitFor(() => {
      expect(options(select)).toEqual([
        ["", "Windows default (Ubuntu)"],
        ["Debian", "Debian"],
        ["Ubuntu", "Ubuntu"],
        ["Alpine", "Alpine (not installed)"],
      ]);
    });
    expect(select.value).toBe("Alpine");
  });

  it("is hidden without WSL, as listed each time Preferences opens", async () => {
    const { fake, app, section } = await picker();
    await vi.waitFor(() => {
      expect(section.hidden).toBe(false);
    });
    expect(app.actions.run("escape")).toBe(true);
    fake.wslDistros = { default: null, installed: [] };
    const listed = vi.spyOn(fake, "listWslDistros");
    expect(app.actions.run("preferences")).toBe(true);
    await vi.waitFor(() => {
      expect(listed).toHaveBeenCalled();
    });
    await vi.waitFor(() => {
      expect(section.hidden).toBe(true);
    });
  });
});
