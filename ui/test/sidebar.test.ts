import { describe, expect, it, vi } from "vitest";
import type { RootView } from "../src/generated/RootView";
import { Sidebar } from "../src/sidebar";

function root(path: string, truncated: boolean): RootView {
  const name = path.slice(path.lastIndexOf("\\") + 1);
  return {
    path,
    name,
    state: { state: "ready" },
    tree: { name, path, isDir: true, children: [], readme: null, status: null },
    truncated,
  };
}

describe("Sidebar", () => {
  it("notes a root whose index stopped at the file cap", () => {
    const host = document.createElement("div");
    const sidebar = new Sidebar(host, { open: vi.fn(), retry: vi.fn(), contextMenu: vi.fn() });
    sidebar.start();
    sidebar.setLibrary({ roots: [root("C:\\Code", true), root("C:\\Notes", false)] });
    const sections = [...host.querySelectorAll<HTMLElement>(".lib-root")];
    const noted = sections.filter((s) => s.querySelector(".lib-truncated") !== null);
    expect(noted.map((s) => s.dataset.root)).toEqual(["C:\\Code"]);
    expect(noted[0]?.querySelector(".lib-truncated")?.textContent).toContain("too many files");
  });
});
