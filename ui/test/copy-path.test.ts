// Copying paths in Windows or WSL form: what the shortcuts copy (a selection holding paths, else a
// focused link, else the open file), how a selection reads as paths, the toasts, and the
// selection's, the links', the ⋯ and the library's menus.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { pathsIn, pathTokens } from "../src/copy-path";
import { CONVERT_LIMIT } from "../src/generated/path-tokens";
import { PATH_TOKEN_VECTORS } from "../src/generated/path-tokens";
import { A, appRoot, fixtures, rendered } from "./helpers";

const PLAN = "S:\\Notes\\My Vault\\plan.md";
const REPORT = "S:\\Notes\\My Vault\\artifacts\\report.html";
const LONG = `S:\\Notes\\My Vault\\${"a-folder-with-a-long-name\\".repeat(4)}plan.md`;
const COMMAND = "bundle exec rubocop app/services/x spec/services/x";
const RELATIVE = "app/services/x/writing_type.rb";

const HTML = `
<h1 id="title">Title</h1>
<pre><code id="code">/home/me/projects/app/run.sh
./run.sh --fast</code></pre>
<pre><code id="cmd">${COMMAND}</code></pre>
<pre><code id="share-cmd">cp \\\\nas\\share\\x.md /mnt/s/Notes/x.md</code></pre>
<pre><code id="spaced">S:\\Notes\\My Vault\\plan.md</code></pre>
<pre><code id="two">cat /mnt/s/a.md /mnt/s/b.md</code></pre>
<pre><code id="big"></code></pre>
<p><code id="line-ref">spec/a_spec.rb:12</code> <code id="ticked">\`S:\\Notes\\plan.md:12\`</code> <code id="quoted">"app/x y.rb"</code></p>
<p id="prose">See plan.md for more.</p>
<p id="inline">Run <code id="rel">${RELATIVE}</code> now.</p>
<p>
<a href="#" class="code-link" data-kind="path" data-target="${PLAN}" id="to-plan"><code>plan.md</code></a>
<a data-written="../artifacts/report.html" href="#" class="link-doc" data-kind="doc" data-target="${REPORT}" id="to-report">report</a>
<a data-written="plan.md" href="#" class="link-doc" data-kind="doc" data-target="${PLAN}" id="to-sibling">sibling</a>
<a href="#" class="wikilink" data-kind="doc" data-target="${PLAN}" id="to-wiki">plan</a>
<a data-written="../notes/v1.0#archive.md" href="#" class="link-doc" data-kind="doc" data-target="S:\\Notes\\notes\\v1.0#archive.md" id="to-hash">archive</a>
<a data-written="docs/owners'" href="#" data-kind="path" data-target="S:\\Notes\\docs\\owners'" id="to-apostrophe">owners</a>
<a data-written="/home/me/notes/v1.0#archive.md" href="#" data-kind="path" data-target="\\\\wsl.localhost\\Ubuntu\\home\\me\\notes\\v1.0#archive.md" id="to-absolute">absolute</a>
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

/** Selects the text of `node` from `start` to `end`. */
function select(node: Node, start: number, end: number): void {
  const range = document.createRange();
  range.setStart(node, start);
  range.setEnd(node, end);
  const selection = document.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

/** Selects all the text of element `id`. */
function selectAll(id: string): string {
  const text = el(id).firstChild;
  if (!text) throw new Error(`no text in #${id}`);
  select(text, 0, text.textContent?.length ?? 0);
  return document.getSelection()?.toString() ?? "";
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

/** Presses Ctrl+Shift+`key` with the clipboard's calls so far forgotten, so `copied` waits for its copy. */
function pressAgain(key: "C" | "W"): boolean {
  writeText.mockClear();
  return press(key);
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

/** The right-click menu on `target`, and its item labelled `label`, clicked. */
async function choose(target: HTMLElement, label: string): Promise<void> {
  expect(rightClick(target).defaultPrevented).toBe(true);
  const items = await menuItems();
  const item = items.find((i) => i.querySelector(".menu-label")?.textContent === label);
  if (!item) throw new Error(`no ${label} in ${labels(items).join(", ")}`);
  writeText.mockClear();
  item.click();
}

describe("a selection read as paths", () => {
  it("finds the words and quoted runs core does", () => {
    for (const vector of PATH_TOKEN_VECTORS) {
      const found = pathTokens(vector.text).map(({ start, end }) => vector.text.slice(start, end));
      expect(found, JSON.stringify(vector.text)).toEqual(vector.tokens);
    }
  });

  it("is one path when nothing but whitespace, quotes or backticks surround its only path", () => {
    for (const text of [RELATIVE, ` ${RELATIVE}\n`, '"/mnt/s/My Vault/a.md"', "`spec\\a.rb:12`"]) {
      expect(pathsIn(text), text).toEqual({ kind: "text", path: text });
    }
    for (const text of [COMMAND, "--config=app/x.yml", "see app/x.rb", "a/b\nc/d"]) {
      expect(pathsIn(text), text).toEqual({ kind: "paths", text });
    }
    for (const text of ["notes.md", "see https://example.com/a/b", ""]) {
      expect(pathsIn(text), text).toBeNull();
    }
  });

  it("is one path when a single line starts as an absolute path and doesn't read as a command", () => {
    for (const text of [
      "S:\\Notes\\My Vault\\plan.md",
      "S:/Notes/My Vault/plan.md",
      " /mnt/s/Notes/My Vault/plan.md\n",
      "S:\\Notes\\Report - final.md",
      "/home/me/notes/a - b -- c.md",
      '"\\\\nas\\share\\My Notes\\x.md"',
    ]) {
      expect(pathsIn(text), text).toEqual({ kind: "text", path: text });
    }
    for (const text of [
      "/mnt/s/a.md /mnt/s/b.md",
      "S:\\My Vault\\a.md C:\\x.md",
      "/home/me/run.sh --fast",
      "/home/me/run.sh -v",
      "S:\\x\\a b.md | less",
      "/x/a b.md > out.txt",
      "/x/a b.md; ls",
      "/x/a b.md && ls",
      "/x/a b.md < in.txt",
      "cat /mnt/s/a.md /mnt/s/b.md",
      "/a/b c\n/d/e f",
    ]) {
      expect(pathsIn(text), text).toEqual({ kind: "paths", text });
    }
  });

  it("reads a long line of quoted runs in linear time", () => {
    const line = '"a/b" '.repeat(100_000);
    const started = performance.now();
    const found = pathTokens(line);
    const elapsed = performance.now() - started;
    expect(found).toHaveLength(100_000);
    expect(elapsed).toBeLessThan(200);
    // So does a long word of runs back to back, and of flags with quoted values.
    for (const word of ['"a/b"'.repeat(100_000), '-x="a/b"'.repeat(60_000)]) {
      const start = performance.now();
      expect(pathTokens(word).length).toBeGreaterThan(0);
      expect(performance.now() - start).toBeLessThan(200);
    }
  });
});

describe("the copy path shortcuts", () => {
  it("copy the open file's WSL path with Ctrl+Shift+C and its Windows path with Ctrl+Shift+W", async () => {
    await started();
    expect(press("C")).toBe(true);
    expect(await copied()).toBe("/mnt/c/V/a.md");
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the WSL path", detail: "/mnt/c/V/a.md" });
    });
    writeText.mockClear();
    expect(press("W")).toBe(true);
    expect(await copied()).toBe(A);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the Windows path", detail: A });
    });
    // Ctrl+Shift+L copied the WSL path up to v0.4.0.
    expect(press("L")).toBe(false);
  });

  it("copy a selected path first, even with a path link focused", async () => {
    await started();
    el("to-plan").focus();
    const code = el("code").firstChild;
    if (!code) throw new Error("no code text");
    select(code, 0, "/home/me/projects/app/run.sh\n".length);
    expect(press("W")).toBe(true);
    // The line break selected with it stays.
    expect(await copied()).toBe("\\\\wsl.localhost\\Ubuntu\\home\\me\\projects\\app\\run.sh\n");
  });

  it("send the open file and a wikilink's target as exact paths, a link's path as written, and a selected path as text", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    const writtenForms = vi.spyOn(fake, "writtenForms");
    expect(press("W")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith(A, true);
    el("to-plan").focus();
    expect(press("W")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith(PLAN, true);
    el("to-absolute").focus();
    expect(press("W")).toBe(true);
    expect(writtenForms).toHaveBeenLastCalledWith("/home/me/notes/v1.0#archive.md");
    el("to-report").focus();
    expect(press("W")).toBe(true);
    expect(writtenForms).toHaveBeenLastCalledWith("../artifacts/report.html");
    expect(pathForms).toHaveBeenCalledTimes(2);
    const code = el("code").firstChild;
    if (!code) throw new Error("no code text");
    select(code, 0, "/home/me/projects/app/run.sh".length);
    expect(press("W")).toBe(true);
    expect(pathForms).toHaveBeenLastCalledWith("/home/me/projects/app/run.sh", false);
  });

  it("copy a focused link written with a relative path as written, in either form", async () => {
    await started();
    el("to-report").focus();
    expect(press("W")).toBe(true);
    expect(await copied()).toBe("..\\artifacts\\report.html");
    expect(pressAgain("C")).toBe(true);
    expect(await copied()).toBe("../artifacts/report.html");
  });

  it("convert a selected relative path in inline code", async () => {
    await started();
    selectAll("rel");
    expect(press("W")).toBe(true);
    expect(await copied()).toBe("app\\services\\x\\writing_type.rb");
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied the Windows path");
    });
  });

  it("convert each path in a selected command or block, never the open file", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    selectAll("cmd");
    expect(press("W")).toBe(true);
    const windows = "bundle exec rubocop app\\services\\x spec\\services\\x";
    expect(await copied()).toBe(windows);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied with 2 Windows paths", detail: windows });
    });
    expect(pressAgain("C")).toBe(true);
    expect(await copied()).toBe(COMMAND);
    // Two lines, each path converted, the line break kept.
    selectAll("code");
    expect(pressAgain("W")).toBe(true);
    expect(await copied()).toBe(
      "\\\\wsl.localhost\\Ubuntu\\home\\me\\projects\\app\\run.sh\n.\\run.sh --fast",
    );
    expect(pathForms).not.toHaveBeenCalled();
  });

  it("copy a selected absolute path with spaces as one path, so a mapping reaches it", async () => {
    const { fake } = await started();
    await fake.setSettings({
      pathMappings: [{ from: "/home/me/vault", to: "S:\\Notes\\My Vault" }],
    });
    const convertPaths = vi.spyOn(fake, "convertPaths");
    selectAll("spaced");
    expect(press("C")).toBe(true);
    expect(await copied()).toBe("/home/me/vault/plan.md");
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({
        message: "Copied the WSL path",
        detail: "/home/me/vault/plan.md",
      });
    });
    expect(convertPaths).not.toHaveBeenCalled();
    // Two paths on the line still convert one by one.
    selectAll("two");
    expect(pressAgain("W")).toBe(true);
    expect(await copied()).toBe("cat S:\\a.md S:\\b.md");
    expect(convertPaths).toHaveBeenCalledTimes(1);
  });

  it("refuse a selection too long to read, and never copy the open file instead", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    const convertPaths = vi.spyOn(fake, "convertPaths");
    el("big").textContent = '"a/b" '.repeat(Math.ceil(CONVERT_LIMIT / 6) + 1);
    selectAll("big");
    expect(press("C")).toBe(true);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({
        message: "The selection is too long to convert.",
        detail: null,
      });
    });
    expect(pathForms).not.toHaveBeenCalled();
    expect(convertPaths).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();
  });

  it("keep everything around a selected path but its format: a line, backticks, quotes", async () => {
    await started();
    selectAll("line-ref");
    expect(press("W")).toBe(true);
    expect(await copied()).toBe("spec\\a_spec.rb:12");
    selectAll("ticked");
    expect(pressAgain("C")).toBe(true);
    expect(await copied()).toBe("`/mnt/s/Notes/plan.md:12`");
    selectAll("quoted");
    expect(pressAgain("W")).toBe(true);
    expect(await copied()).toBe('"app\\x y.rb"');
  });

  it("say how many paths had no form, and copy them as written", async () => {
    await started();
    selectAll("share-cmd");
    expect(press("C")).toBe(true);
    expect(await copied()).toBe("cp \\\\nas\\share\\x.md /mnt/s/Notes/x.md");
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied with 1 WSL path; 1 had no WSL form");
    });
  });

  it("take a selection without a path as no selection: the open file", async () => {
    await started();
    const prose = el("prose").firstChild;
    if (!prose) throw new Error("no prose text");
    select(prose, 4, 11);
    expect(document.getSelection()?.toString()).toBe("plan.md");
    expect(press("W")).toBe(true);
    expect(await copied()).toBe(A);
  });

  it("do nothing with no file open, nothing selected and no link focused", async () => {
    const { app } = await started(null);
    expect(app.state.doc).toBeNull();
    expect(press("C")).toBe(false);
    expect(press("W")).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(writeText).not.toHaveBeenCalled();
  });

  it("copy nothing where a form doesn't exist, and say how to get a WSL one", async () => {
    const { fake } = await started();
    el("to-share").focus();
    expect(press("C")).toBe(true);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: NO_WSL, detail: null });
    });
    vi.spyOn(fake, "pathForms").mockResolvedValue({ windows: null, wsl: null });
    expect(press("W")).toBe(true);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "No Windows path for this location.", detail: null });
    });
    expect(writeText).not.toHaveBeenCalled();
  });

  it("shorten a long path in the middle in the toast, and copy it whole", async () => {
    await started();
    el("to-long").focus();
    expect(press("W")).toBe(true);
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

// jsdom drops the selection once the menu takes the focus, so each right-click selects again.
describe("a selection's right-click menu", () => {
  it("copies a selected path as selected, or as a Windows or WSL path", async () => {
    await started();
    selectAll("rel");
    expect(rightClick(el("rel")).defaultPrevented).toBe(true);
    expect(labels(await menuItems())).toEqual(["Copy", "Copy as Windows path", "Copy as WSL path"]);
    selectAll("rel");
    await choose(el("rel"), "Copy");
    expect(await copied()).toBe(RELATIVE);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied", detail: RELATIVE });
    });
    selectAll("rel");
    await choose(el("rel"), "Copy as Windows path");
    expect(await copied()).toBe("app\\services\\x\\writing_type.rb");
  });

  it("copies a selected command with each of its paths converted", async () => {
    await started();
    selectAll("cmd");
    expect(rightClick(el("cmd")).defaultPrevented).toBe(true);
    expect(labels(await menuItems())).toEqual([
      "Copy",
      "Copy with Windows paths",
      "Copy with WSL paths",
    ]);
    selectAll("cmd");
    await choose(el("cmd"), "Copy with Windows paths");
    expect(await copied()).toBe("bundle exec rubocop app\\services\\x spec\\services\\x");
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied with 2 Windows paths");
    });
    selectAll("cmd");
    await choose(el("cmd"), "Copy");
    expect(await copied()).toBe(COMMAND);
  });

  it("works on a selection in a review comment's card", async () => {
    const { app } = await started();
    const card = document.createElement("div");
    card.className = "comment-card";
    card.innerHTML = `<p id="in-card">Try ${COMMAND}</p>`;
    app.layout.app.append(card);
    selectAll("in-card");
    await choose(el("in-card"), "Copy with WSL paths");
    expect(await copied()).toBe(`Try ${COMMAND}`);
  });

  it("offers a selected absolute path with spaces as one path", async () => {
    await started();
    selectAll("spaced");
    expect(rightClick(el("spaced")).defaultPrevented).toBe(true);
    expect(labels(await menuItems())).toEqual(["Copy", "Copy as Windows path", "Copy as WSL path"]);
  });

  it("leaves the browser's menu for a selection too long to read", async () => {
    await started();
    el("big").textContent = '"a/b" '.repeat(Math.ceil(CONVERT_LIMIT / 6) + 1);
    selectAll("big");
    expect(rightClick(el("big")).defaultPrevented).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.querySelector(MENU)).toBeNull();
  });

  it("leaves the browser's menu for a selection without a path, or a click outside it", async () => {
    await started();
    const prose = el("prose").firstChild;
    if (!prose) throw new Error("no prose text");
    select(prose, 4, 11);
    expect(rightClick(el("prose")).defaultPrevented).toBe(false);
    selectAll("rel");
    expect(rightClick(el("title")).defaultPrevented).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.querySelector(MENU)).toBeNull();
  });
});

describe("the copy path menus", () => {
  it("the ⋯ menu copies the open file's path as stored or in either form, with the keys shown", async () => {
    await started();
    el("lx-more-btn").click();
    const items = await menuItems();
    const copy = items.filter((item) => item.textContent.startsWith("Copy"));
    expect(labels(copy)).toEqual(["Copy", "Copy as Windows path", "Copy as WSL path"]);
    expect(copy.map((item) => item.querySelector("kbd")?.textContent)).toEqual([
      undefined,
      "Ctrl+Shift+W",
      "Ctrl+Shift+C",
    ]);
    copy[2]?.click();
    expect(await copied()).toBe("/mnt/c/V/a.md");
    el("lx-more-btn").click();
    const again = await menuItems();
    writeText.mockClear();
    again.find((item) => item.querySelector(".menu-label")?.textContent === "Copy")?.click();
    expect(await copied()).toBe(A);
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied", detail: A });
    });
  });

  it("the library's menu copies the row's exact path as stored or in either form", async () => {
    const { app } = await started();
    // A `#` in a folder's name is part of it, not an anchor.
    const folder = "S:\\Notes\\v1.0#archive";
    await app.actions.contextMenu(new MouseEvent("contextmenu"), folder, false);
    const items = await menuItems();
    expect(labels(items)).toEqual([
      "Reveal in Explorer",
      "Copy",
      "Copy as Windows path",
      "Copy as WSL path",
    ]);
    items[3]?.click();
    expect(await copied()).toBe("/mnt/s/Notes/v1.0#archive");
    await app.actions.contextMenu(new MouseEvent("contextmenu"), folder, false);
    writeText.mockClear();
    (await menuItems())[1]?.click();
    expect(await copied()).toBe(folder);
  });
});

describe("a link's right-click menu", () => {
  it("opens, copies the text, copies either form or reveals a path link", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    const follow = vi.spyOn(fake, "follow");
    const reveal = vi.spyOn(fake, "revealInExplorer");
    expect(rightClick(el("to-plan")).defaultPrevented).toBe(true);
    expect(labels(await menuItems())).toEqual([
      "Open",
      "Copy",
      "Copy as Windows path",
      "Copy as WSL path",
      "Reveal in Explorer",
    ]);
    await choose(el("to-plan"), "Copy as WSL path");
    expect(await copied()).toBe("/mnt/s/Notes/My Vault/plan.md");
    expect(pathForms).toHaveBeenLastCalledWith(PLAN, true);
    await vi.waitFor(() => {
      expect(lastToast().message).toBe("Copied the WSL path");
    });

    await choose(el("to-plan"), "Copy");
    expect(await copied()).toBe("plan.md");

    await choose(el("to-plan"), "Reveal in Explorer");
    expect(reveal).toHaveBeenCalledWith(PLAN);

    await choose(el("to-plan"), "Open");
    expect(follow).toHaveBeenCalledWith({ kind: "path", target: PLAN, line: null, anchor: null });
  });

  it("copies a link's relative path as written, never the absolute path it leads to", async () => {
    const { fake } = await started();
    const reveal = vi.spyOn(fake, "revealInExplorer");
    await choose(el("to-report"), "Copy as Windows path");
    expect(await copied()).toBe("..\\artifacts\\report.html");
    await choose(el("to-report"), "Copy as WSL path");
    expect(await copied()).toBe("../artifacts/report.html");
    await choose(el("to-report"), "Copy");
    expect(await copied()).toBe("report");
    await choose(el("to-report"), "Reveal in Explorer");
    expect(reveal).toHaveBeenCalledWith(REPORT);
  });

  it("copies a link's written path literally: a `#` or a trailing quote is part of it", async () => {
    const { fake } = await started();
    const writtenForms = vi.spyOn(fake, "writtenForms");
    await choose(el("to-hash"), "Copy as Windows path");
    expect(await copied()).toBe("..\\notes\\v1.0#archive.md");
    await choose(el("to-hash"), "Copy as WSL path");
    expect(await copied()).toBe("../notes/v1.0#archive.md");
    await choose(el("to-apostrophe"), "Copy as Windows path");
    expect(await copied()).toBe("docs\\owners'");
    expect(writtenForms).toHaveBeenLastCalledWith("docs/owners'");
  });

  it("copies a link's absolute written path as Rust maps it, the `#` kept", async () => {
    const { fake } = await started();
    const writtenForms = vi.spyOn(fake, "writtenForms");
    await choose(el("to-absolute"), "Copy as WSL path");
    expect(await copied()).toBe("/home/me/notes/v1.0#archive.md");
    expect(writtenForms).toHaveBeenLastCalledWith("/home/me/notes/v1.0#archive.md");
    await choose(el("to-absolute"), "Copy as Windows path");
    expect(await copied()).toBe("\\\\wsl.localhost\\Ubuntu\\home\\me\\notes\\v1.0#archive.md");
  });

  it("copies a link written with a bare file name as written, in either form", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    const writtenForms = vi.spyOn(fake, "writtenForms");
    await choose(el("to-sibling"), "Copy as Windows path");
    expect(await copied()).toBe("plan.md");
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the Windows path", detail: "plan.md" });
    });
    await choose(el("to-sibling"), "Copy as WSL path");
    expect(await copied()).toBe("plan.md");
    await vi.waitFor(() => {
      expect(lastToast()).toEqual({ message: "Copied the WSL path", detail: "plan.md" });
    });
    // So does the shortcut on it, focused. It is never resolved.
    el("to-sibling").focus();
    writeText.mockClear();
    expect(press("W")).toBe(true);
    expect(await copied()).toBe("plan.md");
    expect(pathForms).not.toHaveBeenCalled();
    expect(writtenForms).toHaveBeenLastCalledWith("plan.md");
  });

  it("copies where a wikilink leads, written with no path", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    await choose(el("to-wiki"), "Copy as WSL path");
    expect(await copied()).toBe("/mnt/s/Notes/My Vault/plan.md");
    await choose(el("to-wiki"), "Copy as Windows path");
    expect(await copied()).toBe(PLAN);
    expect(pathForms).toHaveBeenLastCalledWith(PLAN, true);
  });

  it("offers only the copy items on a broken link written as a path", async () => {
    const { fake } = await started();
    const pathForms = vi.spyOn(fake, "pathForms");
    expect(rightClick(el("to-gone")).defaultPrevented).toBe(true);
    expect(labels(await menuItems())).toEqual(["Copy", "Copy as Windows path", "Copy as WSL path"]);
    await choose(el("to-gone"), "Copy as Windows path");
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
