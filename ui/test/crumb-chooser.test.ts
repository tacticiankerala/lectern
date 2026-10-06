import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { CrumbChooser, chooserRows, crumbFolder, type CrumbFolder } from "../src/crumb-chooser";
import type { Crumb } from "../src/generated/Crumb";
import type { RootView } from "../src/generated/RootView";
import type { TreeNode } from "../src/generated/TreeNode";
import { appRoot, fixtures, rendered, settle } from "./helpers";

const ROOT = "C:\\V";

function file(path: string): TreeNode {
  const name = path.slice(path.lastIndexOf("\\") + 1);
  return { name, path, isDir: false, children: [], readme: null, status: null, comments: null };
}

function dir(path: string, children: TreeNode[], status: string | null = null): TreeNode {
  const name = path.slice(path.lastIndexOf("\\") + 1);
  const readme = children.find((c) => c.name === "README.md")?.path ?? null;
  return { name, path, isDir: true, children, readme, status, comments: null };
}

// V: notes/ (a README with a status), plans/ (q1/ and a roadmap), then two files. Folders come
// first, then files, as the tree has them.
const TREE: TreeNode = dir(ROOT, [
  dir(
    `${ROOT}\\notes`,
    [file(`${ROOT}\\notes\\b.md`), file(`${ROOT}\\notes\\c.md`), file(`${ROOT}\\notes\\README.md`)],
    "active",
  ),
  dir(`${ROOT}\\plans`, [
    dir(`${ROOT}\\plans\\q1`, [file(`${ROOT}\\plans\\q1\\goals.md`)]),
    file(`${ROOT}\\plans\\Roadmap.md`),
  ]),
  file(`${ROOT}\\a.md`),
  file(`${ROOT}\\zeta.md`),
]);

const ROADMAP = `${ROOT}\\plans\\Roadmap.md`;

function rootView(tree: TreeNode | null, path = ROOT): RootView {
  return { path, name: "V", state: { state: "ready" }, tree, truncated: false };
}

function crumbs(...paths: string[]): Crumb[] {
  return paths.map((path) => ({
    name: path.slice(path.lastIndexOf("\\") + 1),
    path,
    readme: null,
  }));
}

const ROADMAP_CRUMBS = crumbs(ROOT, `${ROOT}\\plans`, ROADMAP);

describe("crumbFolder", () => {
  const roots = [rootView(TREE)];

  it("lists the root's children for the root crumb", () => {
    const target = crumbFolder(roots, ROADMAP_CRUMBS, 0);
    expect(target?.trail.map((n) => n.name)).toEqual(["V"]);
    expect(target?.current).toBeNull();
  });

  it("lists a folder crumb's folder", () => {
    const target = crumbFolder(roots, ROADMAP_CRUMBS, 1);
    expect(target?.trail.map((n) => n.name)).toEqual(["V", "plans"]);
    expect(target?.current).toBeNull();
  });

  it("lists the file's own folder for the last crumb, marking the file", () => {
    const target = crumbFolder(roots, ROADMAP_CRUMBS, 2);
    expect(target?.trail.map((n) => n.name)).toEqual(["V", "plans"]);
    expect(target?.current).toBe(ROADMAP);
  });

  it("lists as far down as the tree goes towards a folder it doesn't have yet", () => {
    const fresh = `${ROOT}\\plans\\new\\idea.md`;
    const target = crumbFolder(
      roots,
      crumbs(ROOT, `${ROOT}\\plans`, `${ROOT}\\plans\\new`, fresh),
      3,
    );
    expect(target?.trail.map((n) => n.name)).toEqual(["V", "plans"]);
    expect(target?.current).toBe(fresh);
  });

  it("takes the deepest root holding the folder, whatever the roots' order", () => {
    const nested = dir(`${ROOT}\\plans`, [file(`${ROOT}\\plans\\only-nested.md`), file(ROADMAP)]);
    const nestedCrumbs = crumbs(`${ROOT}\\plans`, ROADMAP);
    const target = crumbFolder(
      [rootView(TREE), rootView(nested, `${ROOT}\\plans`)],
      nestedCrumbs,
      1,
    );
    expect(target?.trail).toHaveLength(1);
    expect(target?.trail[0]).toBe(nested);
    // A nested root not scanned yet leaves the folder to the root above it.
    const unscanned = [rootView(TREE), rootView(null, `${ROOT}\\plans`)];
    expect(crumbFolder(unscanned, nestedCrumbs, 1)?.trail.map((n) => n.name)).toEqual([
      "V",
      "plans",
    ]);
  });

  it("matches paths as Windows does, whatever their case", () => {
    const shouting = crumbs("c:\\v", "C:\\V\\PLANS", "c:\\v\\plans\\roadmap.md");
    expect(crumbFolder(roots, shouting, 2)?.trail.map((n) => n.name)).toEqual(["V", "plans"]);
  });

  it("has nothing for a document outside every root, or a root not scanned yet", () => {
    const outside = crumbs("D:\\Elsewhere", "D:\\Elsewhere\\notes.md");
    expect(crumbFolder(roots, outside, 0)).toBeNull();
    expect(crumbFolder(roots, outside, 1)).toBeNull();
    expect(crumbFolder([rootView(null)], ROADMAP_CRUMBS, 1)).toBeNull();
    // A document whose only crumb is itself has no folder to list.
    expect(crumbFolder(roots, crumbs(ROADMAP), 0)).toBeNull();
  });
});

describe("chooserRows", () => {
  it("lists folders first, then files, in the tree's order", () => {
    expect(chooserRows(TREE, "").map((n) => n.name)).toEqual(["notes", "plans", "a.md", "zeta.md"]);
  });

  it("filters by name, ignoring case", () => {
    const plans = TREE.children[1];
    if (!plans) throw new Error("no plans");
    expect(chooserRows(plans, "ROAD").map((n) => n.name)).toEqual(["Roadmap.md"]);
    expect(chooserRows(TREE, " a ").map((n) => n.name)).toEqual(["plans", "a.md", "zeta.md"]);
    expect(chooserRows(TREE, "nothing like it")).toEqual([]);
  });
});

describe("CrumbChooser", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  function setup(index: number, badges = true) {
    const target = crumbFolder([rootView(TREE)], ROADMAP_CRUMBS, index);
    if (!target) throw new Error("no target");
    return mount(target, badges);
  }

  function mount(target: CrumbFolder, badges = true) {
    document.body.innerHTML = "<button id='anchor'>crumb</button><div id='overlays'></div>";
    const anchor = document.getElementById("anchor") as HTMLElement;
    const host = { open: vi.fn(), badges: () => badges };
    const chooser = new CrumbChooser(document.getElementById("overlays") as HTMLElement, host);
    chooser.open(anchor, target);
    const panel = document.querySelector<HTMLElement>(".crumb-chooser");
    const input = panel?.querySelector<HTMLInputElement>("input");
    if (!panel || !input) throw new Error("no chooser");
    const names = () => [...panel.querySelectorAll(".cc-item .cc-name")].map((n) => n.textContent);
    const chosen = () =>
      panel.querySelector('.cc-item[aria-selected="true"] .cc-name')?.textContent;
    const where = () => panel.querySelector(".cc-path")?.textContent;
    const press = (key: string) => {
      const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      input.dispatchEvent(e);
      return e;
    };
    const type = (text: string) => {
      input.value = text;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    };
    return { anchor, host, chooser, panel, input, names, chosen, where, press, type };
  }

  it("opens on the file's crumb listing its folder, the file marked and chosen", () => {
    const { anchor, chooser, panel, input, names, chosen, where } = setup(2);
    expect(chooser.isOpen).toBe(true);
    expect(panel.hidden).toBe(false);
    expect(anchor.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(input);
    expect(where()).toBe("V / plans");
    expect(names()).toEqual(["q1", "Roadmap.md"]);
    expect(chosen()).toBe("Roadmap.md");
    const current = panel.querySelector(".cc-item.current .cc-name");
    expect(current?.textContent).toBe("Roadmap.md");
  });

  it("starts on the first row for a folder's crumb, with badges when they show", () => {
    const { panel, names, chosen } = setup(0);
    expect(names()).toEqual(["notes", "plans", "a.md", "zeta.md"]);
    expect(chosen()).toBe("notes");
    expect(panel.querySelector(".cc-item.current")).toBeNull();
    expect(panel.querySelector(".cc-item.dir .badge.status-active")?.textContent).toBe("active");
    expect(panel.querySelectorAll(".cc-item.dir .cc-chevron")).toHaveLength(2);
  });

  it("leaves the badges out when they are turned off", () => {
    const { panel } = setup(0, false);
    expect(panel.querySelector(".badge")).toBeNull();
  });

  it("moves through the rows with ↑ and ↓, wrapping round", () => {
    const { press, chosen } = setup(0);
    press("ArrowDown");
    expect(chosen()).toBe("plans");
    press("ArrowUp");
    press("ArrowUp");
    expect(chosen()).toBe("zeta.md");
  });

  it("filters as the user types, ignoring case", () => {
    const { type, names, chosen } = setup(0);
    type("ZET");
    expect(names()).toEqual(["zeta.md"]);
    expect(chosen()).toBe("zeta.md");
    type("nothing like it");
    expect(names()).toEqual([]);
  });

  it("goes into a folder on Enter and stays open, and back up on Backspace", () => {
    const { chooser, press, type, names, chosen, where, input } = setup(2);
    press("ArrowUp");
    expect(chosen()).toBe("q1");
    press("Enter");
    expect(chooser.isOpen).toBe(true);
    expect(where()).toBe("V / plans / q1");
    expect(names()).toEqual(["goals.md"]);
    // A filter takes Backspace for itself; empty, Backspace goes up to the folder just left.
    type("go");
    press("Backspace");
    expect(where()).toBe("V / plans / q1");
    type("");
    press("Backspace");
    expect(where()).toBe("V / plans");
    expect(chosen()).toBe("q1");
    expect(input.value).toBe("");
  });

  it("goes in with → and up with ←, and the Up button goes up until the root", () => {
    const { panel, press, where, chosen } = setup(0);
    press("ArrowDown");
    press("ArrowRight");
    expect(where()).toBe("V / plans");
    // → on a file does nothing.
    press("ArrowDown");
    expect(chosen()).toBe("Roadmap.md");
    press("ArrowRight");
    expect(where()).toBe("V / plans");
    press("ArrowLeft");
    expect(where()).toBe("V");
    expect(chosen()).toBe("plans");
    const up = panel.querySelector<HTMLButtonElement>(".cc-up");
    expect(up?.disabled).toBe(true);
    press("ArrowRight");
    expect(up?.disabled).toBe(false);
    up?.click();
    expect(where()).toBe("V");
  });

  it("← moves the caret in a filter, going up only from an empty filter or its start", () => {
    const { input, press, type, where } = setup(1);
    type("road");
    input.setSelectionRange(4, 4);
    expect(press("ArrowLeft").defaultPrevented).toBe(false);
    expect(where()).toBe("V / plans");
    // Text selected from the start is the filter's too.
    input.setSelectionRange(0, 2);
    expect(press("ArrowLeft").defaultPrevented).toBe(false);
    expect(where()).toBe("V / plans");
    // The caret at the start, nothing selected: up a folder.
    input.setSelectionRange(0, 0);
    expect(press("ArrowLeft").defaultPrevented).toBe(true);
    expect(where()).toBe("V");
    expect(input.value).toBe("");
  });

  it("opens a file on Enter and closes, giving focus back to the crumb", () => {
    const { host, chooser, panel, press, anchor } = setup(0);
    press("ArrowDown");
    press("ArrowDown");
    press("Enter");
    expect(host.open).toHaveBeenCalledWith(`${ROOT}\\a.md`);
    expect(chooser.isOpen).toBe(false);
    expect(panel.hidden).toBe(true);
    expect(anchor.getAttribute("aria-expanded")).toBe("false");
  });

  it("works the same with the mouse: a folder goes in, a file opens", () => {
    const { host, chooser, panel, where } = setup(0);
    const row = (name: string) =>
      [...panel.querySelectorAll<HTMLElement>(".cc-item")].find(
        (el) => el.querySelector(".cc-name")?.textContent === name,
      );
    row("plans")?.click();
    expect(where()).toBe("V / plans");
    expect(chooser.isOpen).toBe(true);
    row("Roadmap.md")?.click();
    expect(host.open).toHaveBeenCalledWith(ROADMAP);
    expect(chooser.isOpen).toBe(false);
  });

  it("lists at most 1,000 rows of a huge folder, while the filter searches them all", () => {
    const notes = Array.from({ length: 130_000 }, (_, i) => file(`${ROOT}\\note-${String(i)}.md`));
    const huge: TreeNode = { ...dir(ROOT, []), children: notes };
    const { panel, names, chosen, type } = mount({ trail: [huge], current: null });
    const more = () => panel.querySelector(".cc-more")?.textContent ?? null;
    expect(panel.querySelectorAll(".cc-item")).toHaveLength(1000);
    expect(names()[999]).toBe("note-999.md");
    expect(more()).toBe("+ 129,000 more, type to filter");
    expect(chosen()).toBe("note-0.md");
    // The filter looks past the first 1,000.
    type("note-129999");
    expect(names()).toEqual(["note-129999.md"]);
    expect(more()).toBeNull();
    // Its results are capped the same way.
    type("9");
    expect(panel.querySelectorAll(".cc-item")).toHaveLength(1000);
    const matches = notes.filter((n) => n.name.includes("9")).length;
    expect(more()).toBe(`+ ${(matches - 1000).toLocaleString("en-US")} more, type to filter`);
  });

  it("chooses the first row when the file to mark lies past the rows shown", () => {
    const notes = Array.from({ length: 1500 }, (_, i) => file(`${ROOT}\\note-${String(i)}.md`));
    const big: TreeNode = { ...dir(ROOT, []), children: notes };
    const { chosen } = mount({ trail: [big], current: `${ROOT}\\note-1200.md` });
    expect(chosen()).toBe("note-0.md");
  });

  it("closes on Esc, focus going back to the crumb, and on a click outside", () => {
    const { chooser, press, anchor } = setup(1);
    press("Escape");
    expect(chooser.isOpen).toBe(false);
    expect(document.activeElement).toBe(anchor);
    const target = crumbFolder([rootView(TREE)], ROADMAP_CRUMBS, 1);
    if (!target) throw new Error("no target");
    chooser.open(anchor, target);
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(chooser.isOpen).toBe(false);
  });
});

describe("App breadcrumbs", () => {
  const PLAN = `${ROOT}\\plans\\Roadmap.md`;

  async function started(initial: string) {
    const fx = fixtures({ [PLAN]: rendered("Roadmap", "<h1 id='roadmap'>Roadmap</h1>") });
    fx.tree = TREE;
    const fake = new FakeBackend(fx, { initial });
    const app = new App(fake, appRoot());
    await app.start();
    const crumbEls = () => [...document.querySelectorAll<HTMLElement>("#lx-breadcrumbs .crumb")];
    return { fake, app, crumbEls };
  }

  it("makes every crumb of a library document a button that opens the chooser", async () => {
    const { app, crumbEls } = await started(PLAN);
    expect(crumbEls().map((c) => c.tagName)).toEqual(["BUTTON", "BUTTON", "BUTTON"]);
    const last = crumbEls()[2];
    expect(last?.getAttribute("aria-current")).toBe("page");
    last?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".crumb-chooser")?.hasAttribute("hidden")).toBe(false);
    });
    expect(last?.getAttribute("aria-expanded")).toBe("true");
    const names = [...document.querySelectorAll(".cc-item .cc-name")].map((n) => n.textContent);
    expect(names).toEqual(["q1", "Roadmap.md"]);
    // A second click on the same crumb closes it.
    last?.click();
    expect(document.querySelector(".crumb-chooser")?.hasAttribute("hidden")).toBe(true);
    expect(app.state.doc?.path).toBe(PLAN);
  });

  it("opens a file from the chooser, pushing the history", async () => {
    const { app, crumbEls } = await started(PLAN);
    crumbEls()[0]?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".cc-item")).not.toBeNull();
    });
    const a = [...document.querySelectorAll<HTMLElement>(".cc-item")].find(
      (el) => el.querySelector(".cc-name")?.textContent === "a.md",
    );
    a?.click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(`${ROOT}\\a.md`);
    });
    expect(document.querySelector<HTMLButtonElement>('button[aria-label="Back"]')?.disabled).toBe(
      false,
    );
  });

  it("opens the chooser on the last crumb with Ctrl+Shift+.", async () => {
    const { crumbEls } = await started(PLAN);
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: ">",
        code: "Period",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    await vi.waitFor(() => {
      expect(crumbEls()[2]?.getAttribute("aria-expanded")).toBe("true");
    });
  });

  function chooserParts() {
    const panel = document.querySelector<HTMLElement>(".crumb-chooser");
    const input = panel?.querySelector<HTMLInputElement>("input");
    if (!panel || !input) throw new Error("no chooser");
    return {
      panel,
      input,
      names: () => [...panel.querySelectorAll(".cc-item .cc-name")].map((n) => n.textContent),
      where: () => panel.querySelector(".cc-path")?.textContent,
      type: (text: string) => {
        input.value = text;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      },
    };
  }

  /** TREE with `plans` as `change` makes it. */
  function withPlans(change: (plans: TreeNode) => TreeNode): RootView {
    const tree = structuredClone(TREE);
    tree.children = tree.children.map((c) => (c.name === "plans" ? change(c) : c));
    return rootView(tree);
  }

  it("follows a library update while open: the new tree, the same folder and filter, the new crumb", async () => {
    const { fake, crumbEls } = await started(PLAN);
    crumbEls()[1]?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".cc-item")).not.toBeNull();
    });
    const cc = chooserParts();
    cc.type("o");
    expect(cc.names()).toEqual(["Roadmap.md"]);
    const before = crumbEls()[1];
    // A file added to the folder shows, one deleted goes.
    const added = withPlans((plans) => ({
      ...plans,
      children: [file(`${ROOT}\\plans\\Overview.md`), file(ROADMAP)],
    }));
    fake.emit("library-updated", { roots: [added] });
    expect(cc.where()).toBe("V / plans");
    expect(cc.input.value).toBe("o");
    expect(cc.names()).toEqual(["Overview.md", "Roadmap.md"]);
    const after = crumbEls()[1];
    expect(after).not.toBe(before);
    expect(after?.getAttribute("aria-expanded")).toBe("true");
    // Esc gives focus to the crumb as it is now.
    cc.input.focus();
    cc.input.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    expect(cc.panel.hidden).toBe(true);
    expect(document.activeElement).toBe(after);
  });

  it("goes up to the nearest folder still there when the one it lists goes", async () => {
    const { fake, crumbEls } = await started(PLAN);
    crumbEls()[1]?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".cc-item")).not.toBeNull();
    });
    const cc = chooserParts();
    [...cc.panel.querySelectorAll<HTMLElement>(".cc-item")][0]?.click();
    expect(cc.where()).toBe("V / plans / q1");
    cc.type("go");
    const gone = withPlans((plans) => ({ ...plans, children: [file(ROADMAP)] }));
    fake.emit("library-updated", { roots: [gone] });
    expect(cc.panel.hidden).toBe(false);
    expect(cc.where()).toBe("V / plans");
    expect(cc.input.value).toBe("");
    expect(cc.names()).toEqual(["Roadmap.md"]);
  });

  it("closes when the document leaves the library, or another document shows", async () => {
    const { fake, app, crumbEls } = await started(PLAN);
    crumbEls()[2]?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".cc-item")).not.toBeNull();
    });
    const cc = chooserParts();
    fake.emit("library-updated", { roots: [] });
    expect(cc.panel.hidden).toBe(true);
    fake.emit("library-updated", { roots: [rootView(TREE)] });
    crumbEls()[2]?.click();
    await vi.waitFor(() => {
      expect(cc.panel.hidden).toBe(false);
    });
    await app.open(`${ROOT}\\a.md`);
    expect(cc.panel.hidden).toBe(true);
  });

  it("leaves the crumbs of a document outside the library as text", async () => {
    const outside = "D:\\Elsewhere\\notes.md";
    const fx = fixtures({ [outside]: rendered("Notes", "<h1>Notes</h1>") });
    fx.tree = TREE;
    const fake = new FakeBackend(fx, { initial: outside });
    const app = new App(fake, appRoot());
    await app.start();
    await settle();
    const crumbEl = document.querySelectorAll("#lx-breadcrumbs .crumb");
    expect(crumbEl.length).toBeGreaterThan(0);
    expect(document.querySelectorAll("#lx-breadcrumbs button.crumb")).toHaveLength(0);
    expect(app.state.doc?.path).toBe(outside);
  });
});
