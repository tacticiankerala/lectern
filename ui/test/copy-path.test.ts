// Copying a path in Windows or WSL form: what the shortcuts copy (a selected path, else a focused
// path link, else the open file), the toasts, and the ⋯, library and link menus.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, rendered } from "./helpers";

const PLAN = "S:\\Notes\\My Vault\\plan.md";
const LONG = `S:\\Notes\\My Vault\\${"a-folder-with-a-long-name\\".repeat(4)}plan.md`;

const HTML = `
<h1 id="title">Title</h1>
<pre><code id="code">/home/me/projects/app/run.sh
./run.sh --fast</code></pre>
<p id="prose">See plan.md for more.</p>
<p>
<a href="#" class="code-link" data-kind="path" data-target="${PLAN}" id="to-plan"><code>plan.md</code></a>
<a href="#" class="code-link" data-kind="path" data-target="\\\\nas\\share\\x.md" id="to-share"><code>x.md</code></a>
<a href="#" class="code-link" data-kind="path" data-target="${LONG}" id="to-long"><code>plan.md</code></a>
<a href="#" class="wikilink broken" data-kind="broken" title="No note named gone" id="to-gone">/home/me/gone.md</a>
<a href="#" class="wikilink broken" data-kind="broken" title="No note named nowhere" id="to-nowhere">nowhere</a>
<a href="https://example.com/" data-kind="external" id="to-web">web</a>
</p>`;

const NO_WSL = "No WSL path for this location. A path mapping in Preferences can add one.";

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
function press(key: "C" | "L"): boolean {
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

/** Selects the text of `node` from `start` to `end`. */
function select(node: Node, start: number, end: number): void {
  const range = document.createRange();
  range.setStart(node, start);
  range.setEnd(node, end);
  const selection = document.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function lastToast(): { message: string; detail: string | null } {
  const toast = document.querySelector("#lx-toasts .toast:last-child");
  return {
    message: toast?.firstChild?.textContent ?? "",
    detail: toast?.querySelector(".toast-detail")?.textContent ?? null,
  };
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

describe("the copy path shortcuts", () => {
  it("copy the open file's Windows path, or its WSL path, and say which", async () => {
    await started();
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the Windows path", detail: A });
    });
    expect(press("L")).toBe(true);
    await vi.waitFor(() => {
      expect(writeText).toHaveBeenLastCalledWith("/mnt/c/V/a.md");
    });
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the WSL path", detail: "/mnt/c/V/a.md" });
    });
  });

  it("copy a selected path first, even with a path link focused", async () => {
    await started();
    el("to-plan").focus();
    const code = el("code").firstChild;
    if (!code) throw new Error("no code text");
    select(code, 0, "/home/me/projects/app/run.sh\n".length);
    expect(press("C")).toBe(true);
    expect(await copied()).toBe("\\\\wsl.localhost\\Ubuntu\\home\\me\\projects\\app\\run.sh");
  });

  it("send the open file and a focused link's target as exact paths, and a selection as text", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    expect(press("C")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith(A, true);
    el("to-plan").focus();
    expect(press("C")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith(PLAN, true);
    const code = el("code").firstChild;
    if (!code) throw new Error("no code text");
    select(code, 0, "/home/me/projects/app/run.sh".length);
    expect(press("C")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith("/home/me/projects/app/run.sh", false);
  });

  it("copy a focused path link when nothing is selected", async () => {
    await started();
    el("to-plan").focus();
    expect(press("L")).toBe(true);
    expect(await copied()).toBe("/mnt/s/Notes/My Vault/plan.md");
  });

  it("take a selection over two lines, or one that isn't an absolute path, as no path", async () => {
    await started();
    const code = el("code").firstChild;
    if (!code) throw new Error("no code text");
    select(code, 0, code.textContent?.length ?? 0);
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
    writeText.mockClear();
    const prose = el("prose").firstChild;
    if (!prose) throw new Error("no prose text");
    select(prose, 4, 11);
    expect(document.getSelection()?.toString()).toBe("plan.md");
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(A);
  });

  it("do nothing with no file open, nothing selected and no link focused", async () => {
    const { app } = await started(null);
    expect(app.state.doc).toBeNull();
    expect(press("C")).toBe(false);
    expect(press("L")).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(writeText).not.toHaveBeenCalled();
  });

  it("copy nothing where a form doesn't exist, and say how to get a WSL one", async () => {
    const { fake } = await started();
    el("to-share").focus();
    expect(press("L")).toBe(true);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: NO_WSL, detail: null });
    });
    vi.spyOn(fake, "pathForms").mockResolvedValue({ windows: null, wsl: null });
    expect(press("C")).toBe(true);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "No Windows path for this location.", detail: null });
    });
    expect(writeText).not.toHaveBeenCalled();
  });

  it("shorten a long path in the middle in the toast, and copy it whole", async () => {
    await started();
    el("to-long").focus();
    expect(press("C")).toBe(true);
    expect(await copied()).toBe(LONG);
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied the Windows path");
    });
    const shown = lastToast().detail ?? "";
    expect(shown.length).toBeLessThan(LONG.length);
    const [head = "", tail = ""] = shown.split("…");
    expect(LONG.startsWith(head) && head.length > 10).toBe(true);
    expect(LONG.endsWith(tail) && tail.endsWith("\\plan.md")).toBe(true);
  });
});

describe("the copy path menus", () => {
  it("the ⋯ menu copies the open file's path in either form, with the keys shown", async () => {
    await started();
    el("lx-more-btn").click();
    const items = await menuItems();
    const copy = items.filter((item) => item.textContent.startsWith("Copy"));
    expect(labels(copy)).toEqual(["Copy Windows path", "Copy WSL path"]);
    expect(copy.map((item) => item.querySelector("kbd")?.textContent)).toEqual([
      "Ctrl+Shift+C",
      "Ctrl+Shift+L",
    ]);
    copy[1]?.click();
    expect(await copied()).toBe("/mnt/c/V/a.md");
  });

  it("the library's menu offers both forms in place of Copy path, of the row's exact path", async () => {
    const { app } = await started();
    // A `#` in a folder's name is part of it, not an anchor.
    const folder = "S:\\Notes\\v1.0#archive";
    await app.actions.contextMenu(new MouseEvent("contextmenu"), folder, false);
    const items = await menuItems();
    expect(labels(items)).toEqual(["Reveal in Explorer", "Copy Windows path", "Copy WSL path"]);
    items[2]?.click();
    expect(await copied()).toBe("/mnt/s/Notes/v1.0#archive");
  });
});

describe("a link's right-click menu", () => {
  it("opens, copies either form or reveals a path link", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    const follow = vi.spyOn(fake, "follow");
    const reveal = vi.spyOn(fake, "revealInExplorer");
    expect(rightClick(el("to-plan")).defaultPrevented).toBe(true);
    let items = await menuItems();
    expect(labels(items)).toEqual([
      "Open",
      "Copy Windows path",
      "Copy WSL path",
      "Reveal in Explorer",
    ]);
    items[2]?.click();
    expect(await copied()).toBe("/mnt/s/Notes/My Vault/plan.md");
    expect(pathForms).toHaveBeenLastCalledWith(PLAN, true);
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied the WSL path");
    });

    rightClick(el("to-plan"));
    items = await menuItems();
    items[3]?.click();
    expect(reveal).toHaveBeenCalledWith(PLAN);

    rightClick(el("to-plan"));
    items = await menuItems();
    items[0]?.click();
    expect(follow).toHaveBeenCalledWith({ kind: "path", target: PLAN, line: null, anchor: null });
  });

  it("offers only the copy items on a broken link written as an absolute path", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    expect(rightClick(el("to-gone")).defaultPrevented).toBe(true);
    const items = await menuItems();
    expect(labels(items)).toEqual(["Copy Windows path", "Copy WSL path"]);
    items[0]?.click();
    expect(await copied()).toBe("\\\\wsl.localhost\\Ubuntu\\home\\me\\gone.md");
    // Its text, which core cleans up as written.
    expect(pathForms).toHaveBeenLastCalledWith("/home/me/gone.md", false);
  });

  it("leaves other links alone", async () => {
    await started();
    expect(rightClick(el("to-nowhere")).defaultPrevented).toBe(false);
    expect(rightClick(el("to-web")).defaultPrevented).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.querySelector(MENU)).toBeNull();
  });
});
