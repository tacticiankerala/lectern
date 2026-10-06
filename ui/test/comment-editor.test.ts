import { afterEach, describe, expect, it, vi } from "vitest";
import { CommentEditor } from "../src/comment-editor";

/** What the comment is on, in the viewport. */
const AT = { left: 60, top: 120, bottom: 150 };

function setup() {
  document.body.innerHTML = `<main id="pane"><article class="doc"><p>Text</p></article></main><button id="outside">Elsewhere</button>`;
  const pane = document.getElementById("pane");
  if (!pane) throw new Error("no pane");
  const editor = new CommentEditor(pane);
  const save = vi.fn<(text: string) => Promise<boolean>>(() => Promise.resolve(true));
  return { pane, editor, save };
}

function area(): HTMLTextAreaElement {
  const el = document.querySelector<HTMLTextAreaElement>(".comment-editor textarea");
  if (!el) throw new Error("no editor");
  return el;
}

function button(label: string): HTMLButtonElement {
  const el = [...document.querySelectorAll<HTMLButtonElement>(".comment-editor button")].find(
    (b) => b.textContent === label,
  );
  if (!el) throw new Error(`no ${label}`);
  return el;
}

function type(el: HTMLTextAreaElement, text: string): void {
  el.value = text;
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

function key(el: HTMLElement, init: KeyboardEventInit): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  el.dispatchEvent(e);
  return e;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("CommentEditor", () => {
  it("Ctrl+Enter saves, Esc cancels", async () => {
    const { pane, editor, save } = setup();
    editor.open({ at: AT, save });
    expect(editor.isOpen).toBe(true);
    // In the pane, never in the note.
    expect(pane.querySelector(":scope > .comment-editor")).not.toBeNull();
    expect(pane.querySelector(".doc .comment-editor")).toBeNull();
    expect(area().placeholder).toBe("Comment…");
    expect(document.querySelector(".comment-editor")?.getAttribute("aria-label")).toBe(
      "New comment",
    );
    expect(document.activeElement).toBe(area());
    type(area(), "  Why fifty?  ");
    key(area(), { key: "Enter", ctrlKey: true });
    await vi.waitFor(() => {
      expect(editor.isOpen).toBe(false);
    });
    expect(save).toHaveBeenCalledWith("Why fifty?");

    // Esc closes it, keeping nothing, and stops there: the app's Esc doesn't see it.
    editor.open({ at: AT, save });
    type(area(), "Never mind");
    const esc = key(area(), { key: "Escape" });
    expect(esc.defaultPrevented).toBe(true);
    expect(editor.isOpen).toBe(false);
    expect(save).toHaveBeenCalledTimes(1);
    editor.open({ at: AT, save });
    expect(area().value).toBe("");
    button("Cancel").click();
    expect(editor.isOpen).toBe(false);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("empty save is blocked", async () => {
    const { editor, save } = setup();
    editor.open({ at: AT, save });
    expect(button("Save").disabled).toBe(true);
    type(area(), "   \n  ");
    expect(button("Save").disabled).toBe(true);
    key(area(), { key: "Enter", ctrlKey: true });
    button("Save").click();
    await Promise.resolve();
    expect(save).not.toHaveBeenCalled();
    expect(editor.isOpen).toBe(true);
    type(area(), "Something");
    expect(button("Save").disabled).toBe(false);
  });

  it("failure keeps the text", async () => {
    const { editor, save } = setup();
    let answer: (saved: boolean) => void = () => undefined;
    save.mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          answer = resolve;
        }),
    );
    editor.open({ at: AT, save });
    type(area(), "Keep this text");
    button("Save").click();
    // Disabled while it saves, so what it shows is what was sent.
    expect(area().disabled).toBe(true);
    expect(button("Save").disabled).toBe(true);
    answer(false);
    await vi.waitFor(() => {
      expect(area().disabled).toBe(false);
    });
    expect(editor.isOpen).toBe(true);
    expect(area().value).toBe("Keep this text");
    expect(button("Save").disabled).toBe(false);
    expect(document.activeElement).toBe(area());
    // And it can be tried again.
    key(area(), { key: "Enter", ctrlKey: true });
    await vi.waitFor(() => {
      expect(editor.isOpen).toBe(false);
    });
    expect(save).toHaveBeenLastCalledWith("Keep this text");
  });

  it("is one at a time and keeps focus inside", () => {
    const { pane, editor, save } = setup();
    editor.open({ at: AT, save });
    type(area(), "First");
    editor.open({ at: AT, save });
    expect(pane.querySelectorAll(".comment-editor")).toHaveLength(1);
    expect(area().value).toBe("");
    type(area(), "Second");
    // Tab goes round the editor: the text, Save, Cancel.
    button("Cancel").focus();
    const tab = key(button("Cancel"), { key: "Tab" });
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(area());
    key(area(), { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(button("Cancel"));
    expect(save).not.toHaveBeenCalled();
  });

  it("a press outside closes it only while it holds no text", () => {
    const { editor, save } = setup();
    const outside = (): void => {
      document
        .getElementById("outside")
        ?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    };
    // Empty, or only spaces: closed.
    editor.open({ at: AT, save });
    outside();
    expect(editor.isOpen).toBe(false);
    editor.open({ at: AT, save });
    type(area(), "  \n ");
    outside();
    expect(editor.isOpen).toBe(false);

    // With text: it stays, the text kept, however often.
    editor.open({ at: AT, save });
    type(area(), "Half a thought");
    area().dispatchEvent(new Event("pointerdown", { bubbles: true }));
    outside();
    outside();
    expect(editor.isOpen).toBe(true);
    expect(area().value).toBe("Half a thought");
    // Only Esc or Cancel let it go.
    button("Cancel").click();
    expect(editor.isOpen).toBe(false);
    expect(save).not.toHaveBeenCalled();
  });

  it("stays in view when what it's on isn't", () => {
    const { pane, editor, save } = setup();
    // The pane shows 100 to 500 of the window; the text is far below it, then far above.
    const view = { x: 0, y: 100, left: 0, right: 800, width: 800, height: 400 };
    vi.spyOn(pane, "getBoundingClientRect").mockReturnValue({
      ...view,
      top: 100,
      bottom: 500,
      toJSON: () => ({}),
    });
    const el = document.querySelector<HTMLElement>(".comment-editor");
    if (!el) throw new Error("no editor");
    Object.defineProperty(el, "offsetHeight", { value: 120, configurable: true });
    editor.open({ at: { left: 60, top: 2400, bottom: 2430 }, save });
    const top = (): number => parseFloat(el.style.top) + 100;
    expect(top()).toBeGreaterThanOrEqual(108);
    expect(top() + 120).toBeLessThanOrEqual(492);
    editor.close();
    editor.open({ at: { left: 60, top: -900, bottom: -870 }, save });
    expect(top()).toBeGreaterThanOrEqual(108);
    expect(top() + 120).toBeLessThanOrEqual(492);
  });
});
