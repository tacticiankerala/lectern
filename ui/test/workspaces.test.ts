import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend, type FakeOptions, type FakeWorkspace } from "../dev/backend-fake";
import { App } from "../src/app";
import { CommentsController } from "../src/comments";
import type { OpenRequest } from "../src/generated/OpenRequest";
import type { Settings } from "../src/generated/Settings";
import type { SettingsPatch } from "../src/generated/SettingsPatch";
import type { SettingsSnapshot } from "../src/generated/SettingsSnapshot";
import { folderName, rootsHint } from "../src/workspace-menu";
import { A, ROOT, appRoot, fixtures, settle } from "./helpers";

const PROJECTS = "C:\\Users\\me\\projects";

function workspace(id: string, name: string, roots: string[], open = false): FakeWorkspace {
  return { id, name, roots, open, theme: null };
}

const WORK = workspace("w1", "Work", [ROOT], true);
const PERSONAL = workspace("w2", "Personal", [PROJECTS]);

async function launch(options: FakeOptions = { workspaces: [WORK, PERSONAL], initial: A }) {
  const fake = new FakeBackend(fixtures(), options);
  const app = new App(fake, appRoot());
  const reload = vi.fn();
  app.reloadWindow = reload;
  await app.start();
  return { fake, app, reload };
}

function chip(): HTMLButtonElement {
  const el = document.querySelector<HTMLButtonElement>("#lx-workspace-btn");
  if (!el) throw new Error("no workspace chip");
  return el;
}

/** Opens the dropdown and waits for its rows. */
async function openMenu(): Promise<HTMLElement> {
  chip().click();
  return vi.waitFor(() => {
    const menu = document.querySelector<HTMLElement>(".ws-menu");
    if (!menu || menu.hidden || menu.querySelector(".ws-open") === null) {
      throw new Error("the dropdown isn't open");
    }
    return menu;
  });
}

function rowButton(scope: ParentNode, name: string): HTMLButtonElement {
  const found = [...scope.querySelectorAll<HTMLButtonElement>(".ws-open")].find(
    (b) => b.querySelector(".ws-name")?.textContent === name,
  );
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}

function key(target: EventTarget, init: KeyboardEventInit): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

function texts(scope: ParentNode, selector: string): string[] {
  return [...scope.querySelectorAll(selector)].map((el) => el.textContent);
}

describe("the workspace chip and its dropdown", () => {
  it("names the window's workspace, and the title names it when there are several", async () => {
    await launch();
    expect(chip().hidden).toBe(false);
    expect(chip().textContent).toContain("Work");
    expect(chip().title).toBe("Work");
    expect(document.title).toBe("Aye — Work");
  });

  it("keeps the title as it was with one workspace, until another is made elsewhere", async () => {
    const { fake } = await launch({ workspaces: [WORK], initial: A });
    expect(document.title).toBe("Aye — Lectern");
    fake.workspacesElsewhere("w2", { name: "Personal", open: true });
    await vi.waitFor(() => {
      expect(document.title).toBe("Aye — Work");
    });
  });

  it("names the workspace alone without a note", async () => {
    await launch({ workspaces: [WORK, PERSONAL] });
    expect(document.title).toBe("Work — Lectern");
  });

  it("lists every workspace with its first folder, and opens another here with Enter", async () => {
    const { fake, reload } = await launch();
    const menu = await openMenu();
    expect(texts(menu, ".ws-name")).toEqual(["Work", "Personal"]);
    expect(texts(menu, ".ws-hint")).toEqual(["V", "projects"]);
    // The window's own is checked, and has no second button; the other's is always there.
    expect(rowButton(menu, "Work").getAttribute("aria-current")).toBe("true");
    expect(rowButton(menu, "Work").querySelector(".ws-mark svg")).not.toBeNull();
    expect(rowButton(menu, "Personal").querySelector(".ws-mark svg")).toBeNull();
    const seconds = [...menu.querySelectorAll<HTMLButtonElement>(".ws-new-window")];
    expect(seconds.map((b) => [b.title, b.getAttribute("aria-label")])).toEqual([
      ["Open in new window", "Open in new window"],
    ]);
    expect(seconds[0]?.closest(".ws-row")?.contains(rowButton(menu, "Personal"))).toBe(true);
    expect(document.activeElement).toBe(rowButton(menu, "Work"));
    expect(texts(menu, ".ws-action")).toEqual(["New workspace", 'Rename "Work"']);
    key(document.activeElement ?? menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(rowButton(menu, "Personal"));
    // Enter on a focused button clicks it.
    rowButton(menu, "Personal").click();
    await vi.waitFor(() => {
      expect(reload).toHaveBeenCalledTimes(1);
    });
    expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w2", where: "here" }]);
    expect(menu.hidden).toBe(true);
    // Until the page reloads, the chip names the workspace on screen.
    await settle();
    expect(chip().textContent).toContain("Work");
  });

  it("opens one in a new window with Ctrl+Enter, then says it's open there", async () => {
    const { fake, reload } = await launch();
    let menu = await openMenu();
    key(rowButton(menu, "Personal"), { key: "Enter", ctrlKey: true });
    await vi.waitFor(() => {
      expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w2", where: "newWindow" }]);
    });
    expect(reload).not.toHaveBeenCalled();
    expect(chip().textContent).toContain("Work");

    // Personal is open in that window now: its row says so in place of the button, and brings the
    // window forward either way.
    menu = await openMenu();
    await vi.waitFor(() => {
      expect(texts(menu, ".ws-switch")).toEqual(["Switch to window"]);
    });
    expect(texts(menu, ".ws-hint")).toEqual(["V", "projects"]);
    expect(menu.querySelector(".ws-new-window")).toBeNull();
    rowButton(menu, "Personal").click();
    await settle();
    expect(fake.windowCalls.at(-1)).toEqual({
      call: "openWorkspace",
      id: "w2",
      where: "newWindow",
    });
    expect(reload).not.toHaveBeenCalled();
  });

  it("a new workspace takes the suggested name, selected, then opens here", async () => {
    const third = workspace("w3", "Workspace 2", []);
    const { fake, reload } = await launch({ workspaces: [WORK, PERSONAL, third], initial: A });
    const menu = await openMenu();
    [...menu.querySelectorAll<HTMLButtonElement>(".ws-action")][0]?.click();
    const input = await vi.waitFor(() => {
      const found = menu.querySelector<HTMLInputElement>(".ws-name-input");
      if (!found) throw new Error("no name field");
      return found;
    });
    expect(input.value).toBe("Workspace 3");
    expect(document.activeElement).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, "Workspace 3".length]);
    expect(texts(menu, ".ws-name-actions .btn")).toEqual(["Open here", "New window"]);
    input.value = "Garden";
    key(input, { key: "Enter" });
    await vi.waitFor(() => {
      expect(reload).toHaveBeenCalledTimes(1);
    });
    expect(fake.windowCalls).toEqual([
      { call: "createWorkspace", name: "Garden", where: "here", root: null },
    ]);
    expect(fake.workspaces().map((ws) => ws.name)).toEqual([
      "Work",
      "Personal",
      "Workspace 2",
      "Garden",
    ]);
  });

  it("Esc in the name field goes back to the actions, and Esc again closes", async () => {
    await launch();
    const menu = await openMenu();
    [...menu.querySelectorAll<HTMLButtonElement>(".ws-action")][0]?.click();
    const input = await vi.waitFor(() => {
      const found = menu.querySelector<HTMLInputElement>(".ws-name-input");
      if (!found) throw new Error("no name field");
      return found;
    });
    key(input, { key: "Escape" });
    expect(menu.querySelector(".ws-name-input")).toBeNull();
    expect(menu.hidden).toBe(false);
    expect(document.activeElement?.textContent).toBe("New workspace");
    key(document.activeElement ?? menu, { key: "Escape" });
    expect(menu.hidden).toBe(true);
    expect(document.activeElement).toBe(chip());
  });

  it("renames the window's workspace from the dropdown", async () => {
    const { fake } = await launch();
    const menu = await openMenu();
    [...menu.querySelectorAll<HTMLButtonElement>(".ws-action")][1]?.click();
    const input = await vi.waitFor(() => {
      const found = menu.querySelector<HTMLInputElement>(".ws-name-input");
      if (!found) throw new Error("no name field");
      return found;
    });
    expect(input.value).toBe("Work");
    input.value = "Atelier";
    key(input, { key: "Enter" });
    await vi.waitFor(() => {
      expect(chip().textContent).toContain("Atelier");
    });
    expect(document.title).toBe("Aye — Atelier");
    expect(fake.workspaces()[0]?.name).toBe("Atelier");
    expect(menu.hidden).toBe(true);
  });

  it("starts a workspace with no folders on the welcome screen, Add folder focused", async () => {
    const garden = workspace("w3", "Garden", [], true);
    await launch({ workspaces: [WORK, garden], current: "w3" });
    expect(chip().textContent).toContain("Garden");
    expect(document.activeElement?.classList.contains("welcome-add-folder")).toBe(true);
  });
});

describe("a blank window", () => {
  const blank: FakeOptions = {
    workspaces: [{ ...WORK, open: false }, PERSONAL],
    current: null,
  };

  it("has no chip, and its welcome screen lists the workspaces to choose from", async () => {
    await launch(blank);
    expect(chip().hidden).toBe(true);
    expect(document.title).toBe("Lectern");
    const doc = document.getElementById("lx-doc");
    expect(doc?.querySelector("h2")?.textContent).toBe("Choose a workspace");
    const list = await vi.waitFor(() => {
      const found = doc?.querySelector<HTMLElement>(".ws-chooser");
      if (!found || found.querySelector(".ws-open") === null) throw new Error("no list");
      return found;
    });
    expect(texts(list, ".ws-name")).toEqual(["Work", "Personal"]);
    expect(texts(list, ".ws-action")).toEqual(["New workspace"]);
    expect(list.querySelectorAll(".ws-new-window")).toHaveLength(2);
    // The library has none to add to: it points at choosing a workspace, or starting one.
    const hint = document.querySelector("#lx-library .lib-empty");
    expect(hint?.querySelector(".lib-empty-title")?.textContent).toBe(
      "Choose a workspace, or add a folder to start a new one.",
    );
    expect(hint?.querySelector("button")?.textContent).toBe("Add folder…");
  });

  it("asks for one open in another window in a new window, from its list too", async () => {
    const { fake, reload } = await launch({ ...blank, workspaces: [WORK, PERSONAL] });
    const list = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".ws-chooser");
      if (!found || found.querySelector(".ws-switch") === null) throw new Error("no list");
      return found;
    });
    rowButton(list, "Work").click();
    await settle();
    expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w1", where: "newWindow" }]);
    expect(reload).not.toHaveBeenCalled();
  });

  it("opens a workspace here from its list, or makes a new one", async () => {
    const { fake, reload } = await launch(blank);
    const list = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".ws-chooser");
      if (!found || found.querySelector(".ws-open") === null) throw new Error("no list");
      return found;
    });
    rowButton(list, "Personal").click();
    await vi.waitFor(() => {
      expect(reload).toHaveBeenCalledTimes(1);
    });
    expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w2", where: "here" }]);
  });

  it("asks a folder launched into it for a name, then makes a workspace holding it", async () => {
    const { fake, reload } = await launch(blank);
    fake.emit("open-request", {
      path: PROJECTS,
      t0Ms: null,
      folder: true,
    } satisfies OpenRequest);
    const input = await vi.waitFor(() => {
      const found = document.querySelector<HTMLInputElement>(".ws-ask .ws-name-input");
      if (!found) throw new Error("no prompt");
      return found;
    });
    expect(document.querySelector(".ws-ask-folder")?.textContent).toBe(PROJECTS);
    expect(input.value).toBe("Workspace 2");
    input.value = "Projects";
    key(input, { key: "Enter" });
    await vi.waitFor(() => {
      expect(reload).toHaveBeenCalledTimes(1);
    });
    expect(fake.windowCalls).toEqual([
      { call: "createWorkspace", name: "Projects", where: "here", root: PROJECTS },
    ]);
    expect(document.querySelector(".ws-ask")).toBeNull();
  });

  it("drops a folder launch when the name is cancelled", async () => {
    const { fake } = await launch(blank);
    fake.emit("open-request", { path: PROJECTS, t0Ms: null, folder: true } satisfies OpenRequest);
    const input = await vi.waitFor(() => {
      const found = document.querySelector<HTMLInputElement>(".ws-ask .ws-name-input");
      if (!found) throw new Error("no prompt");
      return found;
    });
    key(input, { key: "Escape" });
    await settle();
    expect(document.querySelector(".ws-ask")).toBeNull();
    expect(fake.windowCalls).toEqual([]);
  });

  it("a dropped folder asks for a workspace; dropped files open as loose files", async () => {
    const { fake, app } = await launch(blank);
    const open = vi.spyOn(fake, "openUserPath");
    fake.drop([A]);
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(A);
    });
    // Not named as Markdown, yet a file: Rust says so, and it opens without a prompt.
    const text = `${ROOT}\\tide.txt`;
    fake.setDoc(text, "<p>High water at noon</p>");
    fake.drop([text]);
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(text);
    });
    await settle();
    expect(document.querySelector(".ws-ask")).toBeNull();
    // A folder: Rust says so, and a workspace to hold it is asked for.
    fake.drop([PROJECTS]);
    await vi.waitFor(() => {
      expect(document.querySelector(".ws-ask .ws-name-input")).not.toBeNull();
    });
    expect(open.mock.calls.map(([path]) => path)).toEqual([A, text, PROJECTS]);
    // Nothing joined anything.
    expect(app.state.library.roots).toEqual([]);
    expect(fake.workspaces().map((ws) => ws.roots)).toEqual([[ROOT], [PROJECTS]]);
  });

  it("Add folder picks the folder, then asks for the workspace's name", async () => {
    const { fake, app, reload } = await launch(blank);
    vi.spyOn(fake, "pickFolder").mockResolvedValue(PROJECTS);
    expect(app.actions.run("add-folder")).toBe(true);
    const input = await vi.waitFor(() => {
      const found = document.querySelector<HTMLInputElement>(".ws-ask .ws-name-input");
      if (!found) throw new Error("no prompt");
      return found;
    });
    document.querySelectorAll<HTMLButtonElement>(".ws-ask .btn")[1]?.click();
    await settle();
    expect(fake.windowCalls).toEqual([
      { call: "createWorkspace", name: input.value, where: "newWindow", root: PROJECTS },
    ]);
    expect(reload).not.toHaveBeenCalled();
  });
});

describe("leaving the window's workspace", () => {
  it("saves the settings waiting for their debounce before turning to another", async () => {
    const { fake, app, reload } = await launch();
    const saves = vi.spyOn(fake, "setSettings");
    app.updateSettings({ fontSize: 21 }, { debounce: true });
    expect(saves).not.toHaveBeenCalled();
    await app.workspaces.open("w2", "here");
    expect(saves).toHaveBeenCalledWith({ fontSize: 21 });
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("asks before unsaved comment text is lost, and stays when told to", async () => {
    const { fake, app, reload } = await launch();
    const typed = vi.spyOn(CommentsController.prototype, "hasUnsavedText").mockReturnValue(true);
    const switching = app.workspaces.open("w2", "here");
    const keep = await vi.waitFor(() => {
      const found = document.querySelector<HTMLButtonElement>(".ws-confirm .btn.primary");
      if (!found) throw new Error("no confirm");
      return found;
    });
    expect(document.querySelector(".ws-confirm h2")?.textContent).toBe("Your comment isn't saved");
    expect(keep.textContent).toBe("Keep writing");
    keep.click();
    await switching;
    expect(fake.windowCalls).toEqual([]);
    expect(reload).not.toHaveBeenCalled();

    const quitting = app.quit();
    const leave = await vi.waitFor(() => {
      const found = document.querySelectorAll<HTMLButtonElement>(".ws-confirm .btn")[1];
      if (!found) throw new Error("no confirm");
      return found;
    });
    expect(leave.textContent).toBe("Quit anyway");
    leave.click();
    await quitting;
    expect(fake.windowCalls).toEqual([{ call: "quit", force: false }]);
    expect(typed).toHaveBeenCalled();
    typed.mockRestore();
  });

  it("asks before quitting while another window holds an unsaved comment", async () => {
    const { fake, app } = await launch();
    fake.unsavedElsewhere = ["Personal"];
    const confirm = (): Promise<HTMLElement> =>
      vi.waitFor(() => {
        const found = document.querySelector<HTMLElement>(".ws-confirm");
        if (!found) throw new Error("no confirm");
        return found;
      });
    const quitting = app.quit();
    let asked = await confirm();
    expect(asked.querySelector("h2")?.textContent).toBe("Unsaved comment in Personal.");
    expect(asked.querySelector("p")?.textContent).toBe("Quit anyway?");
    expect(texts(asked, ".btn")).toEqual(["Quit", "Cancel"]);
    expect(document.activeElement?.textContent).toBe("Cancel");
    asked.querySelectorAll<HTMLButtonElement>(".btn")[1]?.click();
    await quitting;
    expect(fake.windowCalls).toEqual([{ call: "quit", force: false }]);

    // Two windows; this one's own draft is asked about first.
    fake.unsavedElsewhere = ["Personal", "a blank window"];
    const typed = vi.spyOn(CommentsController.prototype, "hasUnsavedText").mockReturnValue(true);
    const again = app.quit();
    asked = await confirm();
    expect(asked.querySelector("h2")?.textContent).toBe("Your comment isn't saved");
    asked.querySelectorAll<HTMLButtonElement>(".btn")[1]?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".ws-confirm h2")?.textContent).toBe(
        "Unsaved comment in Personal and a blank window.",
      );
    });
    asked = await confirm();
    asked.querySelectorAll<HTMLButtonElement>(".btn")[0]?.click();
    await again;
    expect(fake.windowCalls).toEqual([
      { call: "quit", force: false },
      { call: "quit", force: false },
      { call: "quit", force: true },
    ]);
    typed.mockRestore();
  });

  it("brings a workspace open in another window forward without asking about drafts", async () => {
    const { fake, app, reload } = await launch({
      workspaces: [WORK, { ...PERSONAL, open: true }],
      current: "w1",
      initial: A,
    });
    const typed = vi.spyOn(CommentsController.prototype, "hasUnsavedText").mockReturnValue(true);
    await vi.waitFor(() => {
      document.dispatchEvent(new Event("input"));
      expect(fake.unsavedReports).toEqual([true]);
    });
    await app.workspaces.open("w2", "here");
    expect(document.querySelector(".ws-confirm")).toBeNull();
    // Asked for in a new window, which brings the one showing it forward.
    expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w2", where: "newWindow" }]);
    expect(reload).not.toHaveBeenCalled();
    typed.mockRestore();
  });

  it("a workspace whose window closed unseen opens in a new window, never here", async () => {
    const { fake, reload } = await launch({
      workspaces: [WORK, { ...PERSONAL, open: true }],
      current: "w1",
      initial: A,
    });
    // Its window closes, and this window hasn't heard yet: its list still says open elsewhere.
    vi.spyOn(fake, "listWorkspaces").mockResolvedValue(await fake.listWorkspaces());
    const emit = vi.spyOn(fake, "emit").mockImplementation(() => undefined);
    fake.workspacesElsewhere("w2", { open: false });
    emit.mockRestore();
    const menu = await openMenu();
    expect(texts(menu, ".ws-switch")).toEqual(["Switch to window"]);
    rowButton(menu, "Personal").click();
    await vi.waitFor(() => {
      expect(fake.windowCalls).toEqual([{ call: "openWorkspace", id: "w2", where: "newWindow" }]);
    });
    await settle();
    expect(reload).not.toHaveBeenCalled();
    expect(chip().textContent).toContain("Work");
    expect(fake.workspaces().map((ws) => ws.open)).toEqual([true, true]);
  });

  it("leaves while a comment load is stalled, as no save is waiting", async () => {
    const fake = new FakeBackend(fixtures(), { workspaces: [WORK, PERSONAL], initial: A });
    // The sidecar read never answers, as on a share that has gone away.
    const load = vi.spyOn(fake, "loadReview").mockReturnValue(new Promise<never>(() => undefined));
    const app = new App(fake, appRoot());
    const reload = vi.fn();
    app.reloadWindow = reload;
    await app.start();
    await vi.waitFor(() => {
      expect(load).toHaveBeenCalled();
    });
    await app.workspaces.open("w2", "here");
    expect(reload).toHaveBeenCalledTimes(1);
    await app.quit();
    expect(fake.windowCalls).toEqual([
      { call: "openWorkspace", id: "w2", where: "here" },
      { call: "quit", force: false },
    ]);
  });

  it("tells Rust when unsaved comment text starts and stops", async () => {
    const { fake } = await launch();
    const typed = vi.spyOn(CommentsController.prototype, "hasUnsavedText").mockReturnValue(true);
    await vi.waitFor(() => {
      document.dispatchEvent(new Event("input"));
      expect(fake.unsavedReports).toEqual([true]);
    });
    // Only changes are sent.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    await settle();
    expect(fake.unsavedReports).toEqual([true]);
    typed.mockReturnValue(false);
    document.dispatchEvent(new MouseEvent("click"));
    await vi.waitFor(() => {
      expect(fake.unsavedReports).toEqual([true, false]);
    });
    typed.mockRestore();
  });

  it("Ctrl+N opens a new window, Ctrl+Q quits, and Ctrl+Shift+N still adds a folder", async () => {
    const { fake } = await launch();
    const folder = vi.spyOn(fake, "pickFolder");
    key(document, { key: "n", ctrlKey: true });
    key(document, { key: "N", ctrlKey: true, shiftKey: true });
    await settle();
    expect(fake.windowCalls).toEqual([{ call: "newWindow" }]);
    expect(folder).toHaveBeenCalledTimes(1);
    key(document, { key: "q", ctrlKey: true });
    await vi.waitFor(() => {
      expect(fake.windowCalls).toEqual([{ call: "newWindow" }, { call: "quit", force: false }]);
    });
  });
});

/**
 * Stands in for Rust behind `setSettings`, from the fake's startup revision (0): a save is held
 * until `land` lets it go, then applies, bumps the revision, is told back and answered, as Rust
 * does. Another window's change applies and is told at once (`elsewhere`).
 */
class HeldRust {
  settings: Settings;
  rev = 0;
  private readonly held: (() => void)[] = [];

  constructor(
    private readonly fake: FakeBackend,
    app: App,
  ) {
    this.settings = { ...app.state.settings };
    vi.spyOn(fake, "setSettings").mockImplementation(
      (patch) =>
        new Promise<SettingsSnapshot>((resolve) => {
          this.held.push(() => {
            resolve(this.changed(patch));
          });
        }),
    );
  }

  /** The save sent `index`th (from 0) lands. */
  land(index: number): void {
    this.held[index]?.();
  }

  elsewhere(patch: SettingsPatch): void {
    this.changed(patch);
  }

  private changed(patch: SettingsPatch): SettingsSnapshot {
    const set = Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== null));
    this.settings = { ...this.settings, ...set };
    const snapshot = { settings: this.settings, rev: ++this.rev };
    this.fake.emit("settings-changed", snapshot);
    return snapshot;
  }
}

describe("settings from Rust", () => {
  it("a settings-changed applies at once, as a change made here does", async () => {
    const { fake, app } = await launch();
    fake.settingsElsewhere({ fontSize: 22, lightTheme: "sepia", themeMode: "light" });
    expect(app.state.settings.fontSize).toBe(22);
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--font-size")).toBe("22px");
    expect(root.dataset.theme).toBe("sepia");
  });

  it("the echo of an older save never undoes a newer change", async () => {
    const { fake, app } = await launch();
    const rust = new HeldRust(fake, app);
    app.updateSettings({ fontSize: 20 });
    app.updateSettings({ fontSize: 24 });
    app.updateSettings({ lineHeight: 1.8 }, { debounce: true });
    // The first lands, and is told back, while the second and the debounced one wait.
    rust.land(0);
    expect(app.state.settings.fontSize).toBe(24);
    expect(app.state.settings.lineHeight).toBe(1.8);
    await settle();
    expect(app.state.settings.fontSize).toBe(24);
    rust.land(1);
    await settle(200);
    rust.land(2);
    await settle();
    expect(app.state.settings.fontSize).toBe(24);
    expect(app.state.settings.lineHeight).toBe(1.8);
    // Another window's change applies.
    rust.elsewhere({ fontSize: 26 });
    expect(app.state.settings.fontSize).toBe(26);
  });

  it("a change back to an earlier value survives the echo of the one between", async () => {
    // 18 → 20 → 18 while both saves wait, and another window's unrelated change, still carrying
    // 18, arrives between them: neither save is taken as landed for it.
    const { fake, app } = await launch();
    const rust = new HeldRust(fake, app);
    expect(app.state.settings.fontSize).toBe(18);
    app.updateSettings({ fontSize: 20 });
    app.updateSettings({ fontSize: 18 });
    rust.elsewhere({ lineHeight: 1.8 });
    expect(app.state.settings).toMatchObject({ fontSize: 18, lineHeight: 1.8 });
    // The first save lands and echoes 20, which the second, still on its way, covers.
    rust.land(0);
    expect(app.state.settings.fontSize).toBe(18);
    await settle();
    expect(app.state.settings.fontSize).toBe(18);
    rust.land(1);
    await settle();
    expect(app.state.settings).toMatchObject({ fontSize: 18, lineHeight: 1.8 });
  });

  it("a recovery fetch answered after a newer event never overwrites it", async () => {
    // A settings-changed before startup answers has the settings read again afterwards; by the
    // time that read is answered, another window has changed them again.
    const fake = new FakeBackend(fixtures(), { workspaces: [WORK, PERSONAL], initial: A });
    const app = new App(fake, appRoot());
    let answer: (snapshot: SettingsSnapshot) => void = () => undefined;
    const fetch = vi.spyOn(fake, "getSettings").mockReturnValue(
      new Promise<SettingsSnapshot>((resolve) => {
        answer = resolve;
      }),
    );
    const starting = app.start();
    const base = fake.sharedSettings();
    fake.emit("settings-changed", { settings: { ...base, fontSize: 20 }, rev: 1 });
    await starting;
    expect(fetch).toHaveBeenCalledTimes(1);
    fake.emit("settings-changed", { settings: { ...base, fontSize: 22 }, rev: 2 });
    expect(app.state.settings.fontSize).toBe(22);
    // What Rust had at the first event: older than what's shown.
    answer({ settings: { ...base, fontSize: 20 }, rev: 1 });
    await settle();
    expect(app.state.settings.fontSize).toBe(22);
  });

  it("a save answered after a newer event leaves the newer settings showing", async () => {
    const { fake, app } = await launch();
    let answer: (snapshot: SettingsSnapshot) => void = () => undefined;
    vi.spyOn(fake, "setSettings").mockImplementation(
      () =>
        new Promise<SettingsSnapshot>((resolve) => {
          answer = resolve;
        }),
    );
    const base = { ...app.state.settings };
    app.updateSettings({ fontSize: 24 });
    // Rust applied it and told it back; then another window's 21 landed, and was told too.
    fake.emit("settings-changed", { settings: { ...base, fontSize: 24 }, rev: 1 });
    fake.emit("settings-changed", { settings: { ...base, fontSize: 21 }, rev: 2 });
    // Until the save is answered, its change covers what came since.
    expect(app.state.settings.fontSize).toBe(24);
    answer({ settings: { ...base, fontSize: 24 }, rev: 1 });
    await settle();
    expect(app.state.settings.fontSize).toBe(21);
    // Answered before it is told back: the answer applies, and the telling is nothing new.
    app.updateSettings({ fontSize: 26 });
    answer({ settings: { ...base, fontSize: 26 }, rev: 3 });
    await settle();
    expect(app.state.settings.fontSize).toBe(26);
    fake.emit("settings-changed", { settings: { ...base, fontSize: 26 }, rev: 3 });
    fake.emit("settings-changed", { settings: { ...base, fontSize: 23 }, rev: 4 });
    expect(app.state.settings.fontSize).toBe(23);
  });

  it("a theme of the workspace's own changes only this window's", async () => {
    const { fake, app } = await launch();
    await app.workspaces.setOwnTheme(true);
    expect(app.workspaces.current?.ownTheme).toBe(true);
    app.updateSettings({ themeMode: "light", lightTheme: "sepia" });
    await settle();
    expect(document.documentElement.dataset.theme).toBe("sepia");
    expect(fake.workspaces()[0]?.theme).toEqual({
      themeMode: "light",
      lightTheme: "sepia",
      darkTheme: "graphite",
    });
    expect(fake.sharedSettings().lightTheme).toBe("paper");
    expect(fake.sharedSettings().themeMode).toBe("system");
    // Back to the shared theme.
    await app.workspaces.setOwnTheme(false);
    expect(app.state.settings.lightTheme).toBe("paper");
    expect(fake.workspaces()[0]?.theme).toBeNull();
  });
});

describe("Preferences", () => {
  async function preferences(options?: FakeOptions) {
    const launched = await launch(options);
    expect(launched.app.actions.run("preferences")).toBe(true);
    const dialog = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".prefs");
      if (!found || found.closest<HTMLElement>(".prefs-backdrop")?.hidden !== false) {
        throw new Error("Preferences isn't open");
      }
      return found;
    });
    return { ...launched, dialog };
  }

  it("has This workspace first, then All windows", async () => {
    const { dialog } = await preferences();
    expect(texts(dialog, ".prefs-group-title")).toEqual(["This workspace", "All windows"]);
    expect(texts(dialog, ".prefs-section h4")).toEqual([
      "Name",
      "Libraries",
      "Theme",
      "Status badges",
      "Reading",
      "Path mappings",
      "Editor",
      "Updates",
      "Workspaces",
    ]);
    expect(dialog.querySelector<HTMLInputElement>(".prefs-ws-name")?.value).toBe("Work");
  });

  it("switches on a theme of the workspace's own, with its pickers", async () => {
    const { fake, dialog } = await preferences();
    const own = dialog.querySelector<HTMLInputElement>('input[name="lx-own-theme"]');
    const pickers = dialog.querySelector<HTMLElement>(".prefs-theme");
    expect(own?.checked).toBe(false);
    expect(pickers?.hidden).toBe(true);
    expect(dialog.textContent).toContain("Same as other windows.");
    own?.click();
    await vi.waitFor(() => {
      expect(pickers?.hidden).toBe(false);
    });
    const light = dialog.querySelector<HTMLSelectElement>('select[name="lx-ws-light-theme"]');
    if (!light) throw new Error("no light theme picker");
    light.value = "sepia";
    light.dispatchEvent(new Event("change"));
    dialog
      .querySelector<HTMLInputElement>('input[name="lx-ws-theme-mode"][value="light"]')
      ?.click();
    await settle();
    expect(document.documentElement.dataset.theme).toBe("sepia");
    expect(fake.sharedSettings().lightTheme).toBe("paper");
  });

  it("renames the workspace, and lists the workspaces with Delete only for closed ones", async () => {
    const { fake, dialog } = await preferences();
    const name = dialog.querySelector<HTMLInputElement>(".prefs-ws-name");
    if (!name) throw new Error("no name field");
    name.value = "Atelier";
    name.dispatchEvent(new Event("change"));
    await vi.waitFor(() => {
      expect(chip().textContent).toContain("Atelier");
    });
    const rows = [...dialog.querySelectorAll<HTMLElement>(".prefs-workspaces li")];
    expect(rows.map((li) => li.querySelector(".prefs-root-name")?.textContent)).toEqual([
      "Atelier",
      "Personal",
    ]);
    const remove = (li: HTMLElement | undefined) =>
      [...(li?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
        (b) => b.textContent === "Delete",
      );
    expect(remove(rows[0])?.disabled).toBe(true);
    expect(remove(rows[0])?.title).toBe("Close its window first.");
    expect(remove(rows[1])?.disabled).toBe(false);
    // It asks first; Cancel keeps it.
    const confirm = async (): Promise<HTMLElement> =>
      vi.waitFor(() => {
        const found = document.querySelector<HTMLElement>(".ws-confirm");
        if (!found) throw new Error("no confirm");
        return found;
      });
    remove(rows[1])?.click();
    let asked = await confirm();
    expect(asked.querySelector("h2")?.textContent).toBe('Delete the workspace "Personal"?');
    expect(asked.querySelector("p")?.textContent).toBe(
      "Lectern forgets it; its folders stay on disk.",
    );
    expect(texts(asked, ".btn")).toEqual(["Delete", "Cancel"]);
    expect(document.activeElement?.textContent).toBe("Cancel");
    asked.querySelectorAll<HTMLButtonElement>(".btn")[1]?.click();
    await settle();
    expect(document.querySelector(".ws-confirm")).toBeNull();
    expect(fake.workspaces().map((ws) => ws.name)).toEqual(["Atelier", "Personal"]);
    remove(rows[1])?.click();
    asked = await confirm();
    asked.querySelectorAll<HTMLButtonElement>(".btn")[0]?.click();
    await vi.waitFor(() => {
      expect(dialog.querySelectorAll(".prefs-workspaces li")).toHaveLength(1);
    });
    expect(fake.workspaces().map((ws) => ws.name)).toEqual(["Atelier"]);
    // One workspace left: the title goes back to Lectern's.
    expect(document.title).toBe("Aye — Lectern");
  });

  it("path mappings changed elsewhere replace untouched rows, and closing saves nothing", async () => {
    const { fake, app, dialog } = await preferences();
    const saves = vi.spyOn(fake, "setSettings");
    const values = (): string[] =>
      [...dialog.querySelectorAll<HTMLInputElement>(".prefs-mapping input")].map((i) => i.value);
    fake.settingsElsewhere({ pathMappings: [{ from: "/home/me/shared", to: "S:\\Shared" }] });
    expect(values()).toEqual(["/home/me/shared", "S:\\Shared"]);
    expect(app.actions.run("escape")).toBe(true);
    expect(saves).not.toHaveBeenCalled();
    expect(app.state.settings.pathMappings).toEqual([
      { from: "/home/me/shared", to: "S:\\Shared" },
    ]);
  });

  it("path mappings edited here stay, and closing saves them", async () => {
    const { fake, app, dialog } = await preferences();
    const add = [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => b.textContent === "Add mapping",
    );
    add?.click();
    const [from, to] = [...dialog.querySelectorAll<HTMLInputElement>(".prefs-mapping input")];
    if (!from || !to) throw new Error("no mapping row");
    from.value = "/home/me/notes";
    from.dispatchEvent(new Event("input", { bubbles: true }));
    to.value = "S:\\Notes";
    to.dispatchEvent(new Event("input", { bubbles: true }));
    // A change made elsewhere meanwhile doesn't replace what's being typed.
    fake.settingsElsewhere({ pathMappings: [{ from: "/home/me/shared", to: "S:\\Shared" }] });
    expect(from.isConnected).toBe(true);
    expect(app.actions.run("escape")).toBe(true);
    expect(fake.sharedSettings().pathMappings).toEqual([
      { from: "/home/me/notes", to: "S:\\Notes" },
    ]);
  });

  it("has no This workspace part in a blank window", async () => {
    const { dialog } = await preferences({ workspaces: [{ ...WORK, open: false }], current: null });
    expect(dialog.querySelector<HTMLElement>(".prefs-group")?.hidden).toBe(true);
  });
});

describe("row hints", () => {
  it("name the first folder, and how many more there are", () => {
    expect(rootsHint([])).toBe("");
    expect(rootsHint([PROJECTS])).toBe("projects");
    expect(rootsHint([PROJECTS, "S:\\Notes\\My Vault", "\\\\nas\\share"])).toBe("projects +2");
    expect(folderName("S:\\Notes\\My Vault\\")).toBe("My Vault");
    expect(folderName("/home/me/notes")).toBe("notes");
    expect(folderName("C:\\")).toBe("C:");
  });
});

describe("the automatic update check", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"], shouldAdvanceTime: true });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs only in the process's first window", async () => {
    const { fake } = await launch({ workspaces: [WORK], primary: false });
    await vi.advanceTimersByTimeAsync(6000);
    await settle(20);
    expect(fake.updateCalls).toEqual([]);
  });
});
