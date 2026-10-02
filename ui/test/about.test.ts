import { describe, expect, it, vi } from "vitest";
import type { Action } from "../src/keymap";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { appRoot, fixtures } from "./helpers";

async function openAbout() {
  const fake = new FakeBackend(fixtures());
  const follow = vi.spyOn(fake, "follow").mockResolvedValue({ action: "opened" });
  const app = new App(fake, appRoot());
  await app.start();
  document.querySelector<HTMLButtonElement>("#lx-more-btn")?.click();
  const item = await vi.waitFor(() => {
    const found = [...document.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
      (b) => b.querySelector(".menu-label")?.textContent === "About Lectern",
    );
    if (!found) throw new Error("no About item");
    return found;
  });
  item.click();
  const dialog = await vi.waitFor(() => {
    const found = document.querySelector<HTMLElement>(".about");
    const backdrop = found?.closest<HTMLElement>(".about-backdrop");
    if (!found || !backdrop || backdrop.hidden) {
      throw new Error("About isn't open");
    }
    return found;
  });
  return { fake, follow, dialog, app };
}

describe("About", () => {
  it("shows the version, the licences and the project link", async () => {
    const { dialog } = await openAbout();
    const text = dialog.textContent;
    expect(text).toContain("Version 0.0.0-fake");
    expect(text).toContain("MIT licence");
    expect(text).toContain("SIL Open Font License");
    for (const font of [
      "Inter",
      "Atkinson Hyperlegible Next",
      "Literata",
      "Source Serif 4",
      "JetBrains Mono",
    ]) {
      expect(text).toContain(font);
    }
    expect(dialog.getAttribute("role")).toBe("dialog");
  });

  it("opens the project page in the browser, not in the window", async () => {
    const { dialog, follow } = await openAbout();
    const link = dialog.querySelector<HTMLElement>(".about-link");
    expect(link?.textContent).toBe("github.com/tacticiankerala/lectern");
    link?.click();
    expect(follow).toHaveBeenCalledWith({
      kind: "external",
      target: "https://github.com/tacticiankerala/lectern",
      line: null,
      anchor: null,
    });
  });

  it.each<[Action, string]>([
    ["preferences", ".prefs-backdrop:not(.about-backdrop)"],
    ["quick-open", ".qo-backdrop"],
    ["search", ".sp-backdrop"],
    ["find", ".find-bar"],
  ])("closes when %s opens", async (action, opened) => {
    const { app, dialog } = await openAbout();
    expect(app.actions.run(action)).toBe(true);
    await vi.waitFor(() => {
      const el = document.querySelector<HTMLElement>(opened);
      if (!el || el.hidden) throw new Error(`${action} isn't open`);
    });
    expect(dialog.closest<HTMLElement>(".about-backdrop")?.hidden).toBe(true);
  });

  it("closes with Escape", async () => {
    const { dialog } = await openAbout();
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(dialog.closest<HTMLElement>(".about-backdrop")?.hidden).toBe(true);
  });
});
