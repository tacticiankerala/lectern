import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import { A, appRoot, fixtures, settle } from "./helpers";

async function setup() {
  const fake = new FakeBackend(fixtures(), { initial: A });
  const app = new App(fake, appRoot());
  await app.start();
  const setSettings = vi.spyOn(fake, "setSettings");
  const listSystemFonts = vi.spyOn(fake, "listSystemFonts");
  const button = document.querySelector<HTMLButtonElement>("#lx-reading-btn");
  if (!button) throw new Error("no Aa button");
  button.click();
  const panel = document.querySelector<HTMLElement>(".reading-panel");
  if (!panel) throw new Error("no reading panel");
  return { fake, app, button, panel, setSettings, listSystemFonts };
}

function control(panel: HTMLElement, selector: string): HTMLInputElement {
  const el = panel.querySelector<HTMLInputElement>(selector);
  if (!el) throw new Error(`no ${selector}`);
  return el;
}

function select(panel: HTMLElement, selector: string): HTMLSelectElement {
  const el = panel.querySelector<HTMLSelectElement>(selector);
  if (!el) throw new Error(`no ${selector}`);
  return el;
}

function rootVar(name: string): string {
  return document.documentElement.style.getPropertyValue(name);
}

afterEach(() => {
  vi.useRealTimers();
});

describe("reading panel", () => {
  it("opens from the Aa button as a dialog", async () => {
    const { button, panel } = await setup();
    expect(panel.hidden).toBe(false);
    expect(panel.getAttribute("role")).toBe("dialog");
    expect(button.getAttribute("aria-expanded")).toBe("true");
  });

  it("sets the theme mode", async () => {
    const { panel, setSettings, app } = await setup();
    control(panel, 'input[name="lx-theme-mode"][value="dark"]').click();
    expect(setSettings).toHaveBeenCalledWith({ themeMode: "dark" });
    expect(app.state.settings.themeMode).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("graphite");
  });

  it("picking a swatch of the other mode switches to it", async () => {
    const { panel, setSettings } = await setup();
    expect(document.documentElement.dataset.theme).toBe("paper");
    control(panel, 'input[name="lx-dark-theme"][value="nord"]').click();
    expect(setSettings).toHaveBeenCalledWith({ darkTheme: "nord", themeMode: "dark" });
    expect(document.documentElement.dataset.theme).toBe("nord");
    control(panel, 'input[name="lx-light-theme"][value="sepia"]').click();
    expect(setSettings).toHaveBeenLastCalledWith({ lightTheme: "sepia", themeMode: "light" });
    expect(document.documentElement.dataset.theme).toBe("sepia");
  });

  it("picking a swatch of the current mode keeps the mode", async () => {
    const { panel, setSettings } = await setup();
    control(panel, 'input[name="lx-light-theme"][value="latte"]').click();
    expect(setSettings).toHaveBeenCalledTimes(1);
    expect(setSettings).toHaveBeenCalledWith({ lightTheme: "latte" });
    expect(document.documentElement.dataset.theme).toBe("latte");
  });

  it("clicking the dark theme already chosen, while light shows, switches to it", async () => {
    const { panel, setSettings } = await setup();
    const graphite = control(panel, 'input[name="lx-dark-theme"][value="graphite"]');
    expect(graphite.checked).toBe(true);
    graphite.click();
    expect(setSettings).toHaveBeenCalledWith({ themeMode: "dark" });
    expect(document.documentElement.dataset.theme).toBe("graphite");
    graphite.click();
    expect(setSettings).toHaveBeenCalledTimes(1);
  });

  it("applies a slider at once and saves it once the drag settles", async () => {
    const { panel, setSettings } = await setup();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const size = control(panel, 'input[name="lx-font-size"]');
    for (const value of ["20", "21", "22"]) {
      size.value = value;
      size.dispatchEvent(new Event("input", { bubbles: true }));
    }
    expect(rootVar("--font-size")).toBe("22px");
    expect(setSettings).not.toHaveBeenCalled();
    vi.advanceTimersByTime(150);
    expect(setSettings).toHaveBeenCalledTimes(1);
    expect(setSettings).toHaveBeenCalledWith({ fontSize: 22 });
  });

  it("offers widths from 60 to 160 characters, starting at 100", async () => {
    const { panel } = await setup();
    const width = control(panel, 'input[name="lx-measure"]');
    expect([width.min, width.max, width.value]).toEqual(["60", "160", "100"]);
    expect(rootVar("--measure")).toBe("100ch");
  });

  it("full width, on from the start, turns off to 100 characters", async () => {
    const fake = new FakeBackend(fixtures(), { initial: A });
    await fake.setSettings({ measure: "full" });
    const app = new App(fake, appRoot());
    await app.start();
    document.querySelector<HTMLButtonElement>("#lx-reading-btn")?.click();
    const panel = document.querySelector<HTMLElement>(".reading-panel");
    if (!panel) throw new Error("no reading panel");
    control(panel, 'input[name="lx-full-width"]').click();
    expect(app.state.settings.measure).toBe(100);
    expect(control(panel, 'input[name="lx-measure"]').value).toBe("100");
  });

  it("sets line height and width, and full width disables the width slider", async () => {
    const { panel, setSettings, app } = await setup();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const lineHeight = control(panel, 'input[name="lx-line-height"]');
    lineHeight.value = "1.8";
    lineHeight.dispatchEvent(new Event("input", { bubbles: true }));
    const width = control(panel, 'input[name="lx-measure"]');
    width.value = "90";
    width.dispatchEvent(new Event("input", { bubbles: true }));
    expect(rootVar("--line-height")).toBe("1.8");
    expect(rootVar("--measure")).toBe("90ch");
    vi.advanceTimersByTime(150);
    expect(setSettings).toHaveBeenCalledWith({ lineHeight: 1.8, measure: 90 });

    control(panel, 'input[name="lx-full-width"]').click();
    expect(app.state.settings.measure).toBe("full");
    expect(rootVar("--measure")).toBe("none");
    expect(width.disabled).toBe(true);
    control(panel, 'input[name="lx-full-width"]').click();
    expect(app.state.settings.measure).toBe(90);
    expect(width.disabled).toBe(false);
  });

  it("toggles code wrap", async () => {
    const { panel, setSettings } = await setup();
    control(panel, 'input[name="lx-code-wrap"]').click();
    expect(setSettings).toHaveBeenCalledWith({ codeWrap: true });
    expect(document.documentElement.classList.contains("code-wrap")).toBe(true);
  });

  it("picks a curated body font", async () => {
    const { panel, setSettings } = await setup();
    const body = select(panel, 'select[name="lx-body-font"]');
    expect([...body.options].map((o) => o.value)).toEqual([
      "Segoe UI Variable Text",
      "Inter",
      "Atkinson Hyperlegible Next",
      "Literata",
      "Source Serif 4",
      "Georgia",
      "Sitka Text",
      "Cambria",
      "",
    ]);
    body.value = "Literata";
    body.dispatchEvent(new Event("change", { bubbles: true }));
    expect(setSettings).toHaveBeenCalledWith({ bodyFont: "Literata" });
    expect(rootVar("--body-font")).toMatch(/^"Literata", /);
  });

  it("asks for the installed fonts only once a font select opens, and only once", async () => {
    const { panel, listSystemFonts } = await setup();
    expect(listSystemFonts).not.toHaveBeenCalled();
    const code = select(panel, 'select[name="lx-code-font"]');
    code.focus();
    await settle();
    select(panel, 'select[name="lx-body-font"]').focus();
    code.focus();
    await settle();
    expect(listSystemFonts).toHaveBeenCalledTimes(1);
    // Cascadia Code is offered only when it is installed; the fake has it.
    expect([...code.options].map((o) => o.value)).toEqual([
      "JetBrains Mono",
      "Cascadia Code",
      "Consolas",
      "",
    ]);
  });

  it("takes any installed font through Other", async () => {
    const { panel, setSettings } = await setup();
    const body = select(panel, 'select[name="lx-body-font"]');
    body.focus();
    body.value = "";
    body.dispatchEvent(new Event("change", { bubbles: true }));
    const other = control(panel, 'input[name="lx-body-font-other"]');
    expect(other.hidden).toBe(false);
    await settle();
    const listId = other.getAttribute("list") ?? "";
    const offered = [...(document.getElementById(listId)?.querySelectorAll("option") ?? [])];
    expect(offered.map((o) => o.value)).toContain("Calibri");
    other.value = "Calibri";
    other.dispatchEvent(new Event("change", { bubbles: true }));
    expect(setSettings).toHaveBeenCalledWith({ bodyFont: "Calibri" });
    expect(body.value).toBe("Calibri");
    expect(other.hidden).toBe(true);
  });

  it("closes on Esc and gives focus back to the button", async () => {
    const { panel, button } = await setup();
    control(panel, 'input[name="lx-theme-mode"][value="light"]').focus();
    panel.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(panel.hidden).toBe(true);
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(button);
  });

  it("closes on a click outside", async () => {
    const { panel } = await setup();
    document
      .getElementById("lx-doc")
      ?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(panel.hidden).toBe(true);
  });

  it("follows settings changed elsewhere", async () => {
    const { panel, app } = await setup();
    app.updateSettings({ fontSize: 26, themeMode: "dark" });
    expect(control(panel, 'input[name="lx-font-size"]').value).toBe("26");
    expect(control(panel, 'input[name="lx-theme-mode"][value="dark"]').checked).toBe(true);
  });
});

describe("App shortcuts and focus mode", () => {
  function press(init: KeyboardEventInit): void {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  }

  it("Ctrl+= / Ctrl+- / Ctrl+0 change the font size", async () => {
    const { app } = await setup();
    press({ key: "=", ctrlKey: true });
    press({ key: "=", ctrlKey: true });
    expect(app.state.settings.fontSize).toBe(20);
    expect(rootVar("--font-size")).toBe("20px");
    press({ key: "-", ctrlKey: true });
    expect(app.state.settings.fontSize).toBe(19);
    press({ key: "0", ctrlKey: true });
    expect(app.state.settings.fontSize).toBe(18);
  });

  it("Ctrl+Shift+T flips to the other theme of the pair", async () => {
    const { app } = await setup();
    press({ key: "T", ctrlKey: true, shiftKey: true });
    expect(app.state.settings.themeMode).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("graphite");
  });

  it("F11 enters focus mode full screen and Esc leaves it", async () => {
    const { fake, panel } = await setup();
    panel.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    press({ key: "F11" });
    expect(document.body.classList.contains("focus")).toBe(true);
    expect(fake.fullscreen).toEqual([true]);
    press({ key: "Escape" });
    expect(document.body.classList.contains("focus")).toBe(false);
    expect(fake.fullscreen).toEqual([true, false]);
  });
});
