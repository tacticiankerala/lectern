import { describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures } from "./helpers";

async function setup() {
  const fake = new FakeBackend(fixtures(), { initial: A });
  const app = new App(fake, appRoot());
  await app.start();
  const button = document.querySelector<HTMLButtonElement>("#lx-library-btn");
  if (!button) throw new Error("no library button");
  return { fake, app, button };
}

describe("the library button", () => {
  it("sits at the far left of the header, before back and forward", async () => {
    const { button } = await setup();
    const header = document.getElementById("lx-header");
    expect(header?.firstElementChild).toBe(button);
    expect(button.nextElementSibling?.id).toBe("lx-history-nav");
    expect(button.getAttribute("aria-label")).toBe("Library");
    expect(button.querySelector("svg")).not.toBeNull();
  });

  it("hides and shows the library, saying which it will do, and saves it", async () => {
    const { fake, app, button } = await setup();
    const setSettings = vi.spyOn(fake, "setSettings");
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(button.title).toBe("Hide library (Ctrl+B)");
    button.click();
    expect(app.state.settings.libraryVisible).toBe(false);
    expect(app.layout.app.classList.contains("no-library")).toBe(true);
    expect(setSettings).toHaveBeenCalledWith({ libraryVisible: false });
    await vi.waitFor(() => {
      expect(button.getAttribute("aria-pressed")).toBe("false");
    });
    expect(button.title).toBe("Show library (Ctrl+B)");
    // Ctrl+B does the same.
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "b", ctrlKey: true, bubbles: true, cancelable: true }),
    );
    expect(app.state.settings.libraryVisible).toBe(true);
    await vi.waitFor(() => {
      expect(button.getAttribute("aria-pressed")).toBe("true");
    });
    expect(button.title).toBe("Hide library (Ctrl+B)");
  });

  it("starts pressed or not as the saved setting has it", async () => {
    const fake = new FakeBackend(fixtures(), { initial: A });
    await fake.setSettings({ libraryVisible: false });
    const app = new App(fake, appRoot());
    await app.start();
    const button = document.querySelector<HTMLButtonElement>("#lx-library-btn");
    await vi.waitFor(() => {
      expect(button?.getAttribute("aria-pressed")).toBe("false");
    });
    expect(button?.title).toBe("Show library (Ctrl+B)");
  });
});
