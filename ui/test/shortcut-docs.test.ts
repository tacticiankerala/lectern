import { describe, expect, it } from "vitest";
import readme from "../../README.md?raw";
import { renderWelcome } from "../src/welcome";

describe("the shortcut cheat sheets", () => {
  it("the welcome screen lists the library, breadcrumb chooser, sidebar text and comment keys", () => {
    const host = document.createElement("div");
    const none = (): void => undefined;
    renderWelcome(host, [], {
      openFile: none,
      addFolder: none,
      openRecent: none,
      removeRecent: none,
    });
    const keys = [...host.querySelectorAll(".keys dt")].map((dt) => dt.textContent);
    expect(keys.some((k) => k.split(" / ").includes("Ctrl+B"))).toBe(true);
    expect(keys).toContain("Ctrl+Shift+.");
    expect(keys).toContain("Ctrl+Alt+= / Ctrl+Alt+- / Ctrl+Alt+0");
    expect(keys).toContain("Ctrl+Shift+M");
    expect(keys).toContain("Ctrl+Alt+M");
  });

  it("so does the README, which also describes the chooser and Sidebar text", () => {
    expect(readme).toContain("| Ctrl+B | Show or hide the library |");
    expect(readme).toContain("| Ctrl+Shift+. |");
    expect(readme).toContain("| Ctrl+Alt+= / Ctrl+Alt+- / Ctrl+Alt+0 |");
    expect(readme).toContain("| Ctrl+Shift+M | Show or hide review comments |");
    expect(readme).toContain("| Ctrl+Alt+M |");
    expect(readme).toContain("**Sidebar text**");
    expect(readme).toMatch(/breadcrumb/i);
  });
});
