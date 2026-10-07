// Keyboard shortcuts (spec §6): one global keydown handler that maps keys to actions. While the
// user types in a field only Esc and the find and search keys (Ctrl+F, F3, Shift+F3,
// Ctrl+Shift+F) go through: no field edits with them, and WebView2 would open its own find bar.

export type Action =
  | "open-file"
  | "add-folder"
  | "quick-open"
  | "search"
  | "find"
  | "find-next"
  | "find-prev"
  | "back"
  | "forward"
  | "font-up"
  | "font-down"
  | "font-reset"
  | "sidebar-font-up"
  | "sidebar-font-down"
  | "sidebar-font-reset"
  | "breadcrumbs"
  | "toggle-theme"
  | "toggle-library"
  | "toggle-outline"
  | "focus"
  | "open-editor"
  | "copy-path"
  | "reload"
  | "preferences"
  | "toggle-comments"
  | "add-comment"
  | "new-window"
  | "quit"
  | "escape";

/** Ctrl with a letter or comma, by `e.key` lower-cased: without Shift, then with it. */
const CTRL: Record<string, [Action | null, Action | null]> = {
  o: ["open-file", "toggle-outline"],
  n: ["new-window", "add-folder"],
  p: ["quick-open", null],
  f: ["find", "search"],
  t: [null, "toggle-theme"],
  b: ["toggle-library", null],
  e: ["open-editor", null],
  c: [null, "copy-path"],
  // F5's twin: WebView2 would otherwise reload the whole page.
  r: ["reload", null],
  ",": ["preferences", null],
  m: [null, "toggle-comments"],
  q: ["quit", null],
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
    case "F3":
      if (plain) return "find-next";
      return e.shiftKey && !e.ctrlKey && !e.altKey ? "find-prev" : null;
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
  if (!e.ctrlKey) {
    return null;
  }
  // Ctrl with "+" or "=" (with or without Shift, and the numpad's), "-", or "0": the text size,
  // and with Alt too, the sidebars' text size.
  switch (e.key) {
    case "=":
    case "+":
      return e.altKey ? "sidebar-font-up" : "font-up";
    case "-":
    case "_":
      return e.altKey ? "sidebar-font-down" : "font-down";
    case "0":
      if (e.shiftKey) return null;
      return e.altKey ? "sidebar-font-reset" : "font-reset";
  }
  if (e.altKey) {
    // Ctrl+Alt+M, by the key typed: where AltGr (Ctrl+Alt) makes M type a character (µ), that's
    // what the key is, and it stays typing.
    return !e.shiftKey && e.key.toLowerCase() === "m" ? "add-comment" : null;
  }
  // Ctrl+Shift+. (VS Code's "focus breadcrumbs"), by the key rather than what Shift makes of it.
  if (e.shiftKey && (e.code === "Period" || e.key === ">")) {
    return "breadcrumbs";
  }
  const pair = CTRL[e.key.toLowerCase()];
  return pair ? pair[e.shiftKey ? 1 : 0] : null;
}

/** What still goes through while the user types in a field. */
const WHILE_TYPING = new Set<Action>(["escape", "find", "find-next", "find-prev", "search"]);

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
 * the key's default. While the user types in a field only Esc, find and search go through.
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
    if (action === null || (!WHILE_TYPING.has(action) && isTyping(e.target))) {
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
