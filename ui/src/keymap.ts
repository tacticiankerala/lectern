// Keyboard shortcuts (spec §6): one global keydown handler that maps keys to actions. Task 10 has
// the reading ones; Task 11 adds navigation to `Action` and `actionFor`.

export type Action = "font-up" | "font-down" | "font-reset" | "toggle-theme" | "focus" | "escape";

/** The action a key press stands for, or null. */
export function actionFor(e: KeyboardEvent): Action | null {
  if (e.key === "Escape" && !e.ctrlKey && !e.altKey) {
    return "escape";
  }
  if (e.key === "F11" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
    return "focus";
  }
  if (!e.ctrlKey || e.altKey || e.metaKey) {
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
    case "T":
    case "t":
      return e.shiftKey ? "toggle-theme" : null;
    default:
      return null;
  }
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
