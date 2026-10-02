// Keyboard shortcuts (spec §6): one global keydown handler that maps keys to actions. While the
// user types in a field only Esc goes through.

export type Action =
  | "open-file"
  | "add-folder"
  | "quick-open"
  | "search"
  | "find"
  | "back"
  | "forward"
  | "font-up"
  | "font-down"
  | "font-reset"
  | "toggle-theme"
  | "toggle-library"
  | "toggle-outline"
  | "focus"
  | "open-editor"
  | "copy-path"
  | "reload"
  | "preferences"
  | "escape";

/** Ctrl with a letter or comma, by `e.key` lower-cased: without Shift, then with it. */
const CTRL: Record<string, [Action | null, Action | null]> = {
  o: ["open-file", "toggle-outline"],
  n: [null, "add-folder"],
  p: ["quick-open", null],
  f: ["find", "search"],
  t: [null, "toggle-theme"],
  b: ["toggle-library", null],
  e: ["open-editor", null],
  c: [null, "copy-path"],
  // F5's twin: WebView2 would otherwise reload the whole page.
  r: ["reload", null],
  ",": ["preferences", null],
};

/** The action a key press stands for, or null. */
export function actionFor(e: KeyboardEvent): Action | null {
  if (e.metaKey) {
    return null;
  }
  const plain = !e.ctrlKey && !e.altKey && !e.shiftKey;
  switch (e.key) {
    case "Escape":
      return e.ctrlKey || e.altKey ? null : "escape";
    case "F11":
      return plain ? "focus" : null;
    case "F5":
      return plain ? "reload" : null;
    case "BrowserBack":
      return plain ? "back" : null;
    case "BrowserForward":
      return plain ? "forward" : null;
    case "ArrowLeft":
    case "ArrowRight":
      if (e.altKey && !e.ctrlKey && !e.shiftKey) {
        return e.key === "ArrowLeft" ? "back" : "forward";
      }
      return null;
  }
  if (!e.ctrlKey || e.altKey) {
    return null;
  }
  // Ctrl with "+" or "=" (with or without Shift, and the numpad's), "-", or "0".
  switch (e.key) {
    case "=":
    case "+":
      return "font-up";
    case "-":
    case "_":
      return "font-down";
    case "0":
      return e.shiftKey ? null : "font-reset";
  }
  const pair = CTRL[e.key.toLowerCase()];
  return pair ? pair[e.shiftKey ? 1 : 0] : null;
}

/** Text entry, where keys belong to the field. */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target instanceof HTMLInputElement) {
    return !["checkbox", "radio", "range", "button", "submit", "reset", "color"].includes(
      target.type,
    );
  }
  return target instanceof HTMLTextAreaElement || target.isContentEditable;
}

/**
 * Listens for shortcuts on `target`. `run` returns whether it handled the action, which then stops
 * the key's default. While the user types in a field only Esc goes through.
 */
export function installKeymap(
  target: Document | HTMLElement,
  run: (action: Action, e: KeyboardEvent) => boolean,
): () => void {
  const onKey = (e: Event): void => {
    if (!(e instanceof KeyboardEvent) || e.defaultPrevented) {
      return;
    }
    const action = actionFor(e);
    if (action === null || (action !== "escape" && isTyping(e.target))) {
      return;
    }
    if (run(action, e)) {
      e.preventDefault();
    }
  };
  target.addEventListener("keydown", onKey);
  return () => {
    target.removeEventListener("keydown", onKey);
  };
}
