// Lectern's shortcuts while the focus is inside a frame: Rust decides which keys are Lectern's
// (crates/lectern-core/src/frame_keys.rs), from a table that must agree with keymap.ts, and the UI
// runs a `frame-key` through the same actions as a keydown.
import { describe, expect, it } from "vitest";
import { FRAME_KEYS } from "../src/generated/frame-keys";
import { actionFor, actionForKey } from "../src/keymap";

const MODIFIERS: Record<string, { ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {
  plain: { ctrlKey: false, shiftKey: false, altKey: false },
  Shift: { ctrlKey: false, shiftKey: true, altKey: false },
  Ctrl: { ctrlKey: true, shiftKey: false, altKey: false },
  "Ctrl+Shift": { ctrlKey: true, shiftKey: true, altKey: false },
  Alt: { ctrlKey: false, shiftKey: false, altKey: true },
  "Alt+Shift": { ctrlKey: false, shiftKey: true, altKey: true },
  "Ctrl+Alt": { ctrlKey: true, shiftKey: false, altKey: true },
  "Ctrl+Alt+Shift": { ctrlKey: true, shiftKey: true, altKey: true },
};

describe("keys inside a frame", () => {
  it("Rust takes as Lectern's exactly the keys keymap.ts has an action for", () => {
    expect(FRAME_KEYS.length).toBeGreaterThan(80);
    const disagree: string[] = [];
    for (const entry of FRAME_KEYS) {
      for (const [name, mods] of Object.entries(MODIFIERS)) {
        const key = mods.shiftKey ? entry.shifted : entry.key;
        const event = new KeyboardEvent("keydown", { key, code: entry.code, ...mods });
        const ours = actionFor(event) !== null;
        if (ours !== entry.lectern.includes(name)) {
          disagree.push(`${name}+${entry.code} (${key}): keymap ${String(ours)}`);
        }
      }
    }
    expect(disagree).toEqual([]);
  });

  it("run as the same key pressed in Lectern's own page would", () => {
    const key = (
      k: string,
      code: string,
      mods: Partial<Record<"ctrl" | "shift" | "alt", boolean>>,
    ) => actionForKey({ key: k, code, ctrl: false, shift: false, alt: false, ...mods });
    expect(key("f", "KeyF", { ctrl: true })).toBe("find");
    expect(key("F3", "F3", {})).toBe("find-next");
    expect(key("F3", "F3", { shift: true })).toBe("find-prev");
    expect(key("F11", "F11", {})).toBe("focus");
    expect(key("Escape", "Escape", {})).toBe("escape");
    expect(key(">", "Period", { ctrl: true, shift: true })).toBe("breadcrumbs");
    expect(key("h", "KeyH", { ctrl: true })).toBe("recent-files");
    expect(key("a", "KeyA", { ctrl: true })).toBeNull();
  });
});
