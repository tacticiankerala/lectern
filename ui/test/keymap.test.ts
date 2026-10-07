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

  it("maps shortcuts", () => {
    expect(actionFor(new KeyboardEvent("keydown", { key: "p", ctrlKey: true }))).toBe("quick-open");
    expect(actionFor(key({ key: "F", ctrlKey: true, shiftKey: true }))).toBe("search");
    expect(actionFor(key({ key: "f", ctrlKey: true }))).toBe("find");
    // F3 is WebView2's own find shortcut otherwise.
    expect(actionFor(key({ key: "F3" }))).toBe("find-next");
    expect(actionFor(key({ key: "F3", shiftKey: true }))).toBe("find-prev");
    expect(actionFor(key({ key: "F3", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "ArrowLeft", altKey: true }))).toBe("back");
    expect(actionFor(key({ key: "ArrowRight", altKey: true }))).toBe("forward");
    expect(actionFor(key({ key: "F11" }))).toBe("focus");
    expect(actionFor(key({ key: "=", ctrlKey: true }))).toBe("font-up");
  });

  it("maps the rest of the spec's table", () => {
    const table: [KeyboardEventInit, string][] = [
      [{ key: "o", ctrlKey: true }, "open-file"],
      [{ key: "N", ctrlKey: true, shiftKey: true }, "add-folder"],
      // A new window, which lists the workspaces; and quitting with every window kept.
      [{ key: "n", ctrlKey: true }, "new-window"],
      [{ key: "q", ctrlKey: true }, "quit"],
      [{ key: "b", ctrlKey: true }, "toggle-library"],
      [{ key: "O", ctrlKey: true, shiftKey: true }, "toggle-outline"],
      [{ key: "e", ctrlKey: true }, "open-editor"],
      [{ key: "C", ctrlKey: true, shiftKey: true }, "copy-path"],
      [{ key: "F5" }, "reload"],
      // WebView2 would otherwise reload the whole page.
      [{ key: "r", ctrlKey: true }, "reload"],
      [{ key: ",", ctrlKey: true }, "preferences"],
      // Caps Lock gives capitals without Shift.
      [{ key: "P", ctrlKey: true }, "quick-open"],
      // Keyboards with Back and Forward keys.
      [{ key: "BrowserBack" }, "back"],
      [{ key: "BrowserForward" }, "forward"],
      // The sidebars' text size.
      [{ key: "=", ctrlKey: true, altKey: true }, "sidebar-font-up"],
      [{ key: "+", ctrlKey: true, altKey: true, shiftKey: true }, "sidebar-font-up"],
      [{ key: "-", ctrlKey: true, altKey: true }, "sidebar-font-down"],
      [{ key: "0", ctrlKey: true, altKey: true }, "sidebar-font-reset"],
      // VS Code's "focus breadcrumbs": the period key, whatever Shift makes of it.
      [{ key: ">", code: "Period", ctrlKey: true, shiftKey: true }, "breadcrumbs"],
    ];
    for (const [init, action] of table) {
      expect(actionFor(key(init)), JSON.stringify(init)).toBe(action);
    }
  });

  it("maps the comment shortcuts", () => {
    expect(actionFor(key({ key: "M", ctrlKey: true, shiftKey: true }))).toBe("toggle-comments");
    expect(actionFor(key({ key: "m", ctrlKey: true, altKey: true }))).toBe("add-comment");
    // Caps Lock gives a capital without Shift.
    expect(actionFor(key({ key: "M", ctrlKey: true, altKey: true }))).toBe("add-comment");
    // Ctrl+Alt with other letters stays free.
    expect(actionFor(key({ key: "p", ctrlKey: true, altKey: true }))).toBeNull();
    // AltGr (Ctrl+Alt on Windows) types µ on some layouts: that's typing, not the shortcut.
    expect(actionFor(key({ key: "µ", code: "KeyM", ctrlKey: true, altKey: true }))).toBeNull();
    expect(actionFor(key({ key: "m", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "M", ctrlKey: true, altKey: true, shiftKey: true }))).toBeNull();
  });

  it("ignores other keys and modifiers", () => {
    expect(actionFor(key({ key: "=" }))).toBeNull();
    expect(actionFor(key({ key: "p", ctrlKey: true, altKey: true }))).toBeNull();
    expect(actionFor(key({ key: "0", ctrlKey: true, altKey: true, shiftKey: true }))).toBeNull();
    expect(actionFor(key({ key: ".", code: "Period", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: ">", code: "Period", shiftKey: true }))).toBeNull();
    expect(actionFor(key({ key: "t", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "F11", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "p" }))).toBeNull();
    expect(actionFor(key({ key: "P", ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(actionFor(key({ key: "ArrowLeft" }))).toBeNull();
    expect(actionFor(key({ key: "ArrowLeft", altKey: true, ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "ArrowLeft", altKey: true, shiftKey: true }))).toBeNull();
    expect(actionFor(key({ key: "F5", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "c", ctrlKey: true }))).toBeNull();
    expect(actionFor(key({ key: "b", ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(actionFor(key({ key: "p", metaKey: true }))).toBeNull();
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

  it("stands aside while typing, except for Esc, find and search", () => {
    const run = vi.fn(() => true);
    uninstall = installKeymap(document, run);
    document.body.innerHTML = "<input type='text'><input type='range'>";
    const [text, range] = document.querySelectorAll("input");
    text?.dispatchEvent(key({ key: "=", ctrlKey: true }));
    text?.dispatchEvent(key({ key: "p", ctrlKey: true }));
    expect(run).not.toHaveBeenCalled();
    text?.dispatchEvent(key({ key: "Escape" }));
    expect(run).toHaveBeenCalledWith("escape", expect.any(KeyboardEvent));
    text?.dispatchEvent(key({ key: "f", ctrlKey: true }));
    expect(run).toHaveBeenLastCalledWith("find", expect.any(KeyboardEvent));
    text?.dispatchEvent(key({ key: "F", ctrlKey: true, shiftKey: true }));
    expect(run).toHaveBeenLastCalledWith("search", expect.any(KeyboardEvent));
    text?.dispatchEvent(key({ key: "F3", shiftKey: true }));
    expect(run).toHaveBeenLastCalledWith("find-prev", expect.any(KeyboardEvent));
    range?.dispatchEvent(key({ key: "=", ctrlKey: true }));
    expect(run).toHaveBeenLastCalledWith("font-up", expect.any(KeyboardEvent));
  });
});
