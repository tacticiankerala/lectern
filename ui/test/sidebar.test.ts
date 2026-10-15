import { describe, expect, it, vi } from "vitest";
import type { DocKind } from "../src/generated/DocKind";
import type { RootView } from "../src/generated/RootView";
import { Sidebar } from "../src/sidebar";

function root(path: string, truncated: boolean): RootView {
  const name = path.slice(path.lastIndexOf("\\") + 1);
  return {
    path,
    name,
    state: { state: "ready" },
    tree: {
      name,
      path,
      isDir: true,
      kind: null,
      children: [],
      readme: null,
      status: null,
      comments: null,
    },
    truncated,
  };
}

function host() {
  return {
    open: vi.fn(),
    retry: vi.fn(),
    contextMenu: vi.fn(),
    addFolder: vi.fn(),
    chooseWorkspace: vi.fn(),
  };
}

describe("Sidebar", () => {
  it("notes a root whose index stopped at the file cap", () => {
    const el = document.createElement("div");
    const sidebar = new Sidebar(el, host());
    sidebar.start();
    sidebar.setLibrary({ roots: [root("C:\\Code", true), root("C:\\Notes", false)] });
    const sections = [...el.querySelectorAll<HTMLElement>(".lib-root")];
    const noted = sections.filter((s) => s.querySelector(".lib-truncated") !== null);
    expect(noted.map((s) => s.dataset.root)).toEqual(["C:\\Code"]);
    expect(noted[0]?.querySelector(".lib-truncated")?.textContent).toContain("too many files");
  });

  it("counts a note's open review comments while the feature is on", () => {
    const el = document.createElement("div");
    const sidebar = new Sidebar(el, host());
    const notes = root("C:\\Notes", false);
    const file = (name: string, comments: number | null) => ({
      name,
      path: `C:\\Notes\\${name}`,
      isDir: false,
      kind: "markdown" as const,
      children: [],
      readme: null,
      status: null,
      comments,
    });
    notes.tree?.children.push(file("plan.md", 2), file("ideas.md", 0), file("log.md", null));
    sidebar.start();
    sidebar.setLibrary({ roots: [notes] });
    const counts = () =>
      [...el.querySelectorAll<HTMLElement>(".tree-row")].map((row) => [
        row.dataset.path,
        row.querySelector(".tree-count")?.textContent ?? null,
      ]);
    expect(counts()).toEqual([
      ["C:\\Notes", null],
      ["C:\\Notes\\plan.md", "2"],
      ["C:\\Notes\\ideas.md", null],
      ["C:\\Notes\\log.md", null],
    ]);
    expect(el.querySelector(".tree-count")?.getAttribute("title")).toBe("2 open comments");
    sidebar.setCommentCounts(false);
    expect(el.querySelectorAll(".tree-count")).toHaveLength(0);
  });

  it("marks text files, HTML pages and PDFs with their type, which opens them too", () => {
    const el = document.createElement("div");
    const app = host();
    const sidebar = new Sidebar(el, app);
    const notes = root("C:\\Notes", false);
    const file = (name: string, kind: DocKind) => ({
      name,
      path: `C:\\Notes\\${name}`,
      isDir: false,
      kind,
      children: [],
      readme: null,
      status: null,
      comments: null,
    });
    notes.tree?.children.push(
      file("brief.pdf", "pdf"),
      file("plan.md", "markdown"),
      file("readme.txt", "text"),
      file("report.html", "html"),
    );
    sidebar.start();
    sidebar.setLibrary({ roots: [notes] });
    const marks = () =>
      [...el.querySelectorAll<HTMLElement>(".tree-row")].map((row) => [
        row.dataset.path,
        row.querySelector(".tree-kind")?.textContent ?? null,
      ]);
    expect(marks()).toEqual([
      ["C:\\Notes", null],
      ["C:\\Notes\\brief.pdf", "PDF"],
      ["C:\\Notes\\plan.md", null],
      ["C:\\Notes\\readme.txt", "TXT"],
      ["C:\\Notes\\report.html", "HTML"],
    ]);
    // The name already says it, so screen readers skip the mark.
    expect(el.querySelector(".tree-kind")?.getAttribute("aria-hidden")).toBe("true");
    const report = [...el.querySelectorAll<HTMLElement>(".tree-row")].find(
      (row) => row.dataset.path === "C:\\Notes\\report.html",
    );
    report?.querySelector<HTMLElement>(".tree-kind")?.click();
    expect(app.open).toHaveBeenCalledWith("C:\\Notes\\report.html");
  });

  it("with no roots, offers to add a folder, until there is one", () => {
    const el = document.createElement("div");
    const app = host();
    const sidebar = new Sidebar(el, app);
    sidebar.start();
    const hint = el.querySelector(".lib-empty");
    expect(hint?.textContent).toContain("Add a folder to build your library");
    expect(hint?.textContent).toContain("Lectern indexes the Markdown files in folders you add.");
    const add = hint?.querySelector("button");
    expect(add?.textContent).toBe("Add folder…");
    add?.click();
    expect(app.addFolder).toHaveBeenCalledTimes(1);
    sidebar.setLibrary({ roots: [root("C:\\Notes", false)] });
    expect(el.querySelector(".lib-empty")).toBeNull();
    expect(el.querySelectorAll(".lib-root")).toHaveLength(1);
  });

  it("in a blank window, points at choosing a workspace or starting one", () => {
    const el = document.createElement("div");
    const app = host();
    const sidebar = new Sidebar(el, app);
    sidebar.start();
    sidebar.setBlank(true);
    expect(el.querySelector(".lib-empty-title")?.textContent).toBe(
      "Choose a workspace, or add a folder to start a new one.",
    );
    const buttons = [...el.querySelectorAll<HTMLButtonElement>(".lib-empty button")];
    expect(buttons.map((b) => b.textContent)).toEqual(["Choose a workspace", "Add folder…"]);
    buttons[0]?.click();
    expect(app.chooseWorkspace).toHaveBeenCalledTimes(1);
    buttons[1]?.click();
    expect(app.addFolder).toHaveBeenCalledTimes(1);
    // A window with a workspace has only Add folder.
    sidebar.setBlank(false);
    expect([...el.querySelectorAll(".lib-empty button")].map((b) => b.textContent)).toEqual([
      "Add folder…",
    ]);
  });
});
