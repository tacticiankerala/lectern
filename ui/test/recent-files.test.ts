// The recent files menu (the header's clock, or Ctrl+H): this workspace's recent files but the one
// on screen, numbered, each opening on its number key, with × to forget one and quick open below.
import { describe, expect, it, vi } from "vitest";
import { FakeBackend, type FakeOptions } from "../dev/backend-fake";
import { App } from "../src/app";
import type { RecentEntry } from "../src/generated/RecentEntry";
import type { RenderedDoc } from "../src/generated/RenderedDoc";
import { recentChoices } from "../src/recent-files";
import { A, B, C, ROOT, appRoot, fixtures, rendered, settle } from "./helpers";

const D = "C:\\V\\notes\\deep\\d.md";
const ROOTS = [{ path: ROOT, name: "V" }];

function recent(path: string, title = path): RecentEntry {
  return { path, title, openedMs: 0 };
}

/** `n` notes, `C:\V\many\n1.md` and on, titled "Note 1" and on. */
function many(n: number): string[] {
  return Array.from({ length: n }, (_, i) => `C:\\V\\many\\n${String(i + 1)}.md`);
}

function title(path: string): string {
  return `Note ${/n(\d+)\.md$/.exec(path)?.[1] ?? ""}`;
}

/** The app over A, B, C, D and `extra`, with `options` (recent files, the file opened first). */
async function started(options: FakeOptions, extra: string[] = []) {
  const docs: Record<string, RenderedDoc> = { [D]: rendered("Dee", "<h1>Dee</h1>") };
  for (const path of extra) {
    docs[path] = rendered(title(path), `<h1>${title(path)}</h1>`);
  }
  const fake = new FakeBackend(fixtures(docs), options);
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

const SEEDED: RecentEntry[] = [
  recent(A, "Aye"),
  recent(B, "Bee"),
  recent(C, "Sea"),
  recent(D, "Dee"),
];

function clock(): HTMLButtonElement {
  const el = document.querySelector<HTMLButtonElement>("#lx-recent-btn");
  if (!el) throw new Error("no recent files button");
  return el;
}

function shownMenu(): HTMLElement | null {
  return document.querySelector<HTMLElement>("#lx-overlay-root .menu");
}

/** Ctrl+H, then the menu once it shows. */
async function openRecent(app: App): Promise<HTMLElement> {
  expect(app.actions.run("recent-files")).toBe(true);
  return vi.waitFor(() => {
    const menu = shownMenu();
    if (!menu) throw new Error("the menu isn't open");
    return menu;
  });
}

/** Each file the menu lists: its number, title and folder. */
function rows(menu: HTMLElement): string[][] {
  return [...menu.querySelectorAll(".menu-row")].map((row) => [
    row.querySelector(".menu-digit")?.textContent ?? "",
    row.querySelector(".menu-label")?.textContent ?? "",
    row.querySelector(".menu-detail")?.textContent ?? "",
  ]);
}

function labels(menu: HTMLElement): string[] {
  return [...menu.querySelectorAll(".menu-row .menu-label")].map((l) => l.textContent);
}

function press(target: EventTarget, init: KeyboardEventInit): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

function removeButton(menu: HTMLElement, label: string): HTMLButtonElement {
  const row = [...menu.querySelectorAll<HTMLElement>(".menu-row")].find(
    (r) => r.querySelector(".menu-label")?.textContent === label,
  );
  const remove = row?.querySelector<HTMLButtonElement>(".menu-remove");
  if (!remove) throw new Error(`no × for ${label}`);
  return remove;
}

describe("the entries", () => {
  it("are the recent files but the one on screen, numbered 1 to 9, then 0, then none", () => {
    const paths = many(12);
    const choices = recentChoices(
      paths.map((p) => recent(p)),
      // The file on screen compares as Windows paths do.
      paths[2]?.toUpperCase().replaceAll("\\", "/") ?? null,
      ROOTS,
      20,
    );
    expect(choices.map((c) => c.path)).toEqual(paths.filter((_, i) => i !== 2));
    expect(choices.map((c) => c.key)).toEqual([
      "1",
      "2",
      "3",
      "4",
      "5",
      "6",
      "7",
      "8",
      "9",
      "0",
      null,
    ]);
    expect(choices[0]?.title).toBe(paths[0]);
  });

  it("stop at the setting", () => {
    const entries = many(20).map((p) => recent(p));
    expect(recentChoices(entries, null, ROOTS, 5)).toHaveLength(5);
    expect(recentChoices(entries, null, ROOTS, 10)).toHaveLength(10);
    expect(recentChoices(entries, entries[0]?.path ?? null, ROOTS, 20)).toHaveLength(19);
    expect(recentChoices([], null, ROOTS, 10)).toEqual([]);
  });

  it("name each folder: below its root, the root at its top, else the whole folder, its middle cut when long", () => {
    const outside = "C:\\Users\\me\\Documents\\Projects\\Field notes\\Spring term\\Week twelve";
    const folders = recentChoices(
      [
        recent(D),
        recent("c:/v/Notes/b.md"),
        recent(A),
        recent("C:\\Users\\me\\Downloads\\x.md"),
        recent(`${outside}\\y.md`),
        recent("D:\\z.md"),
      ],
      null,
      ROOTS,
      20,
    ).map((c) => c.folder);
    expect(folders.slice(0, 4)).toEqual(["notes/deep", "Notes", "V", "C:\\Users\\me\\Downloads"]);
    expect(folders[4]).toHaveLength(48);
    expect(folders[4]).toBe("C:\\Users\\me\\Documents\\P…\\Spring term\\Week twelve");
    expect(folders[5]).toBe("D:\\");
    expect(
      recentChoices([recent("E:\\notes\\e.md")], null, [{ path: "E:\\", name: "E:\\" }], 20),
    ).toMatchObject([{ folder: "notes" }]);
  });
});

describe("the header's clock", () => {
  it("sits beside ⋯, named, with its shortcut", async () => {
    await started({ recent: SEEDED, initial: B });
    const button = clock();
    expect(button.getAttribute("aria-label")).toBe("Recent files");
    expect(button.title).toBe("Recent files (Ctrl+H)");
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    expect(button.querySelector("svg")).not.toBeNull();
    expect(button.nextElementSibling?.id).toBe("lx-more-btn");
    expect(button.disabled).toBe(false);
  });

  it("is off with no other recent file to list, and Ctrl+H does nothing", async () => {
    const { app } = await started({ initial: A });
    // A alone, and it's on screen.
    expect(app.state.recent.map((r) => r.path)).toEqual([A]);
    expect(clock().disabled).toBe(true);
    expect(app.actions.run("recent-files")).toBe(false);
    await settle();
    expect(shownMenu()).toBeNull();
    await app.open(B, { push: true });
    expect(clock().disabled).toBe(false);
  });

  it("stays, off, in a blank window", async () => {
    await started({ current: null });
    expect(clock().disabled).toBe(true);
    expect(clock().hidden).toBe(false);
  });
});

describe("the menu", () => {
  it("lists the recent files but the one on screen, numbered, with their folders, then All files", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    // Ctrl+H, as pressed in the window.
    press(document.body, { key: "h", ctrlKey: true });
    const menu = await vi.waitFor(() => {
      const found = shownMenu();
      if (!found) throw new Error("the menu isn't open");
      return found;
    });
    expect(rows(menu)).toEqual([
      ["1", "Aye", "V"],
      ["2", "Sea", "notes"],
      ["3", "Dee", "notes/deep"],
    ]);
    const items = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")];
    expect(items.at(-1)?.querySelector(".menu-label")?.textContent).toBe("All files…");
    expect(items.at(-1)?.querySelector("kbd")?.textContent).toBe("Ctrl+P");
    // Its tooltip: the path, and how to forget it from the keyboard.
    expect(items[0]?.title).toBe(`${A}\nDelete removes it`);
    expect(document.activeElement).toBe(items[0]);
    expect(clock().getAttribute("aria-expanded")).toBe("true");
    expect(app.state.doc?.path).toBe(B);
  });

  it("opens a file on its number, onto the history", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    const menu = await openRecent(app);
    press(document.activeElement ?? menu, { key: "2" });
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(C);
    });
    expect(shownMenu()).toBeNull();
    expect(clock().getAttribute("aria-expanded")).toBe("false");
    expect(app.actions.run("back")).toBe(true);
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
  });

  it("opens a file on a click", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    const menu = await openRecent(app);
    menu.querySelectorAll<HTMLButtonElement>(".menu-row .menu-item")[2]?.click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(D);
    });
    expect(shownMenu()).toBeNull();
  });

  it("moves with ↑ and ↓, and leaves digits with modifiers alone", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    const menu = await openRecent(app);
    const items = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")];
    press(document.activeElement ?? menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(items[1]);
    press(document.activeElement ?? menu, { key: "ArrowUp" });
    press(document.activeElement ?? menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(items.at(-1));
    // Shift gives "@", not "2"; and Alt+2 isn't the menu's.
    press(document.activeElement ?? menu, { key: "@", code: "Digit2", shiftKey: true });
    press(document.activeElement ?? menu, { key: "2", altKey: true });
    await settle();
    expect(shownMenu()).not.toBeNull();
    expect(app.state.doc?.path).toBe(B);
  });

  it("opens the tenth on 0, and numbers none past it", async () => {
    const paths = many(12);
    const { app } = await started({ recent: paths.map((p) => recent(p, title(p))) }, paths);
    app.updateSettings({ recentInMenu: 20 });
    const menu = await openRecent(app);
    expect(rows(menu).map(([digit]) => digit)).toEqual([
      "1",
      "2",
      "3",
      "4",
      "5",
      "6",
      "7",
      "8",
      "9",
      "0",
      "",
      "",
    ]);
    press(document.activeElement ?? menu, { key: "0" });
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(paths[9]);
    });
  });

  it("lists as many as the setting asks", async () => {
    const paths = many(20);
    const { app } = await started({ recent: paths.map((p) => recent(p, title(p))) }, paths);
    expect(rows(await openRecent(app))).toHaveLength(10);
    app.actions.run("escape");
    app.updateSettings({ recentInMenu: 5 });
    expect(labels(await openRecent(app))).toEqual([
      "Note 1",
      "Note 2",
      "Note 3",
      "Note 4",
      "Note 5",
    ]);
  });

  it("forgets a file on ×, for good, staying open on the rest", async () => {
    const { fake, app } = await started({ recent: SEEDED, initial: B });
    const removeRecent = vi.spyOn(fake, "removeRecent");
    const menu = await openRecent(app);
    const remove = removeButton(menu, "Sea");
    expect(remove.getAttribute("aria-label")).toBe("Remove Sea from recent");
    expect(remove.title).toBe("Remove from recent");
    remove.click();
    await vi.waitFor(() => {
      expect(rows(menu)).toEqual([
        ["1", "Aye", "V"],
        ["2", "Dee", "notes/deep"],
      ]);
    });
    expect(removeRecent).toHaveBeenCalledWith(C);
    expect(shownMenu()).toBe(menu);
    expect(menu.contains(document.activeElement)).toBe(true);
    // The recent files are Rust's, without it.
    expect(app.state.recent.map((r) => r.path).sort()).toEqual([A, B, D].sort());
    expect(app.state.doc?.path).toBe(B);
    // Quick open's recent files lose it too.
    expect(app.actions.run("quick-open")).toBe(true);
    await vi.waitFor(() => {
      expect(document.querySelectorAll(".quick-open .qo-item").length).toBeGreaterThan(0);
    });
    const names = [...document.querySelectorAll(".quick-open .qo-name")].map((n) => n.textContent);
    expect(names.sort()).toEqual(["a.md", "b.md", "d.md"]);
  });

  it("forgets one from the welcome screen's list as well, and says when none is left", async () => {
    const { app } = await started({ recent: [recent(A, "Aye"), recent(C, "Sea")] });
    const welcome = () =>
      [...document.querySelectorAll("#lx-doc .recent-title")].map((t) => t.textContent);
    expect(welcome()).toEqual(["Aye", "Sea"]);
    const menu = await openRecent(app);
    expect(labels(menu)).toEqual(["Aye", "Sea"]);
    removeButton(menu, "Aye").click();
    await vi.waitFor(() => {
      expect(welcome()).toEqual(["Sea"]);
    });
    expect(labels(menu)).toEqual(["Sea"]);
    removeButton(menu, "Sea").click();
    await vi.waitFor(() => {
      expect(welcome()).toEqual([]);
    });
    const items = [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")];
    expect(items.map((i) => i.querySelector(".menu-label")?.textContent)).toEqual([
      "No other recent files",
      "All files…",
    ]);
    // The note holds the focus, and is no action.
    expect(items[0]?.getAttribute("aria-disabled")).toBe("true");
    expect(document.activeElement).toBe(items[0]);
    items[0]?.click();
    press(items[0] ?? menu, { key: "Enter" });
    expect(shownMenu()).toBe(menu);
    expect(clock().disabled).toBe(true);
  });

  it("forgets the focused file on Delete, the focus going to the next, else the one before, else the note", async () => {
    const { fake, app } = await started({ recent: SEEDED, initial: B });
    const removeRecent = vi.spyOn(fake, "removeRecent");
    const menu = await openRecent(app);
    const focused = () => document.activeElement?.querySelector(".menu-label")?.textContent;
    press(document.activeElement ?? menu, { key: "ArrowDown" });
    expect(focused()).toBe("Sea");
    // With a modifier, Delete isn't the menu's.
    press(document.activeElement ?? menu, { key: "Delete", ctrlKey: true });
    await settle();
    expect(labels(menu)).toEqual(["Aye", "Sea", "Dee"]);
    press(document.activeElement ?? menu, { key: "Delete" });
    await vi.waitFor(() => {
      expect(rows(menu)).toEqual([
        ["1", "Aye", "V"],
        ["2", "Dee", "notes/deep"],
      ]);
    });
    expect(removeRecent).toHaveBeenLastCalledWith(C);
    expect(focused()).toBe("Dee");
    // The last row: the one before takes the focus.
    press(document.activeElement ?? menu, { key: "Delete" });
    await vi.waitFor(() => {
      expect(labels(menu)).toEqual(["Aye"]);
    });
    expect(focused()).toBe("Aye");
    press(document.activeElement ?? menu, { key: "Delete" });
    await vi.waitFor(() => {
      expect(labels(menu)).toEqual([]);
    });
    expect(focused()).toBe("No other recent files");
    expect(shownMenu()).toBe(menu);
    expect(removeRecent.mock.calls.map(([path]) => path)).toEqual([C, D, A]);
    // Delete on what is no file does nothing.
    press(document.activeElement ?? menu, { key: "Delete" });
    await settle();
    expect(removeRecent).toHaveBeenCalledTimes(3);
    expect(clock().disabled).toBe(true);
  });

  it("keeps All files at its foot while the rows scroll, no taller than the room below the clock", async () => {
    const paths = many(20);
    const { app } = await started({ recent: paths.map((p) => recent(p, title(p))) }, paths);
    app.updateSettings({ recentInMenu: 20 });
    const menu = await openRecent(app);
    const list = menu.querySelector<HTMLElement>(".menu-list");
    const foot = menu.querySelector<HTMLElement>(".menu-foot");
    expect(list?.querySelectorAll(".menu-row")).toHaveLength(20);
    expect(foot?.querySelector(".menu-sep")).not.toBeNull();
    expect([...(foot?.querySelectorAll(".menu-label") ?? [])].map((l) => l.textContent)).toEqual([
      "All files…",
    ]);
    // Below the clock, 4 px down, and 8 px short of the window's bottom.
    const bottom = clock().getBoundingClientRect().bottom;
    expect(menu.style.maxHeight).toBe(`${String(window.innerHeight - bottom - 4 - 8)}px`);
  });

  it("closes Preferences, quick open or any other overlay before it opens", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    expect(app.actions.run("preferences")).toBe(true);
    const prefs = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".prefs-backdrop");
      if (!found || found.hidden) throw new Error("Preferences isn't open");
      return found;
    });
    // Ctrl+H, pressed in Preferences.
    press(document.activeElement ?? document.body, { key: "h", ctrlKey: true });
    await vi.waitFor(() => {
      expect(shownMenu()).not.toBeNull();
    });
    expect(prefs.hidden).toBe(true);
    expect(app.actions.run("escape")).toBe(true);
    // Quick open, then the clock.
    expect(app.actions.run("quick-open")).toBe(true);
    const quick = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".qo-backdrop");
      if (!found || found.hidden) throw new Error("quick open isn't open");
      return found;
    });
    clock().click();
    await vi.waitFor(() => {
      expect(shownMenu()).not.toBeNull();
    });
    expect(quick.hidden).toBe(true);
  });

  it("hands over to quick open from All files…", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    const menu = await openRecent(app);
    [...menu.querySelectorAll<HTMLButtonElement>(".menu-item")].at(-1)?.click();
    await vi.waitFor(() => {
      expect(document.querySelector<HTMLElement>(".qo-backdrop")?.hidden).toBe(false);
    });
    expect(shownMenu()).toBeNull();
  });

  it("closes on Esc, on the clock again, and on Ctrl+H again", async () => {
    const { app } = await started({ recent: SEEDED, initial: B });
    await openRecent(app);
    expect(app.actions.run("escape")).toBe(true);
    expect(shownMenu()).toBeNull();
    expect(clock().getAttribute("aria-expanded")).toBe("false");
    clock().click();
    await vi.waitFor(() => {
      expect(shownMenu()).not.toBeNull();
    });
    clock().click();
    await settle();
    expect(shownMenu()).toBeNull();
    await openRecent(app);
    expect(app.actions.run("recent-files")).toBe(true);
    await settle();
    expect(shownMenu()).toBeNull();
  });

  it("opens on Ctrl+H pressed inside a page or a PDF, which Rust tells", async () => {
    const { fake } = await started({ recent: SEEDED, initial: B });
    fake.frameKey({ key: "h", code: "KeyH", ctrl: true });
    await vi.waitFor(() => {
      expect(shownMenu()).not.toBeNull();
    });
  });
});

describe("Preferences", () => {
  it("sets how many the menu lists, 5 to 20, under Reading", async () => {
    const { fake, app } = await started({ recent: SEEDED, initial: B });
    expect(app.actions.run("preferences")).toBe(true);
    const section = await vi.waitFor(() => {
      const found = [...document.querySelectorAll<HTMLElement>(".prefs-section")].find(
        (s) => s.querySelector("h4")?.textContent === "Reading",
      );
      if (!found) throw new Error("no Reading section");
      return found;
    });
    const input = section.querySelector<HTMLInputElement>('input[name="lx-recent-in-menu"]');
    if (!input) throw new Error("no recent files setting");
    expect(input.type).toBe("number");
    expect(input.closest("label")?.textContent).toBe("Recent files in the menu");
    expect([input.min, input.max, input.value]).toEqual(["5", "20", "10"]);
    const change = (value: string) => {
      input.value = value;
      input.dispatchEvent(new Event("change"));
    };
    change("7");
    await settle();
    expect(fake.sharedSettings().recentInMenu).toBe(7);
    // Out of range, it keeps to the range; empty, it goes back.
    change("50");
    await settle();
    expect(input.value).toBe("20");
    expect(fake.sharedSettings().recentInMenu).toBe(20);
    change("");
    expect(input.value).toBe("20");
    // Changed elsewhere, it shows here.
    fake.settingsElsewhere({ recentInMenu: 12 });
    expect(input.value).toBe("12");
  });
});
