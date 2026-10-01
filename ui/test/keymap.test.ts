import { afterEach, describe, expect, it, vi } from "vitest";
import { actionFor, installKeymap } from "../src/keymap";

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

describe("actionFor", () => {
  it("maps the reading shortcuts", () => {
    expect(actionFor(key({ key: "=", ctrlKey: true }))).toBe("font-up");
    expect(actionFor(key({ key: "+", ctrlKey: true, shiftKey: true }))).toBe("font-up");
    expect(actionFor(key({ key: "-", ctrlKey: true }))).toBe("font-down");
    expect(actionFor(key({ key: "0", ctrlKey: true }))).toBe("font-reset");
    expect(actionFor(key({ key: "T", ctrlKey: true, shiftKey: true }))).toBe("toggle-theme");
    expect(actionFor(key({ key: "F11" }))).toBe("focus");
    expect(actionFor(key({ key: "Escape" }))).toBe("escape");
  });

  it("ignores other keys and modifiers", () => {
    expect(actionFor(key({ key: "=" }))).toBeNull();
    expect(actionFor(key({ key: "=", ctrlKey: true, altKey: true }))).toBeNull();
    expect(actionFor(key({ key: "t", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "F11", ctrlKey: true }))).toBeNull();
  });
});

describe("installKeymap", () => {
  let uninstall = (): void => undefined;
  afterEach(() => {
    uninstall();
    document.body.innerHTML = "";
  });

  it("runs the action and stops the default when it was handled", () => {
    const run = vi.fn(() => true);
    uninstall = installKeymap(document, run);
    const e = key({ key: "=", ctrlKey: true });
    document.body.dispatchEvent(e);
    expect(run).toHaveBeenCalledWith("font-up", e);
    expect(e.defaultPrevented).toBe(true);
  });

  it("leaves the default alone when the action wasn't handled", () => {
    uninstall = installKeymap(document, () => false);
    const e = key({ key: "Escape" });
    document.body.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });

  it("stands aside while typing, except for Esc", () => {
    const run = vi.fn(() => true);
    uninstall = installKeymap(document, run);
    document.body.innerHTML = "<input type='text'><input type='range'>";
    const [text, range] = document.querySelectorAll("input");
    text?.dispatchEvent(key({ key: "=", ctrlKey: true }));
    expect(run).not.toHaveBeenCalled();
    text?.dispatchEvent(key({ key: "Escape" }));
    expect(run).toHaveBeenCalledWith("escape", expect.any(KeyboardEvent));
    range?.dispatchEvent(key({ key: "=", ctrlKey: true }));
    expect(run).toHaveBeenLastCalledWith("font-up", expect.any(KeyboardEvent));
  });
});
