import { describe, expect, it, vi } from "vitest";
import type { RootView } from "../src/generated/RootView";
import { Sidebar } from "../src/sidebar";

function root(path: string, truncated: boolean): RootView {
  const name = path.slice(path.lastIndexOf("\\") + 1);
  return {
    path,
    name,
    state: { state: "ready" },
    tree: { name, path, isDir: true, children: [], readme: null, status: null, comments: null },
    truncated,
  };
}

function host() {
  return { open: vi.fn(), retry: vi.fn(), contextMenu: vi.fn(), addFolder: vi.fn() };
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
});
