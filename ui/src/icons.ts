// The header's icon buttons.
import { h } from "./dom";

/** Header icons: 16 px strokes in the text colour. */
export const ICONS = {
  back: '<path d="M10 3.5 5.5 8l4.5 4.5"/>',
  forward: '<path d="M6 3.5 10.5 8 6 12.5"/>',
  library: '<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M6 3v10"/>',
  outline: '<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M10 3v10"/>',
  more: '<circle cx="3.5" cy="8" r="0.6"/><circle cx="8" cy="8" r="0.6"/><circle cx="12.5" cy="8" r="0.6"/>',
  comments:
    '<path d="M3.5 3h9A1.5 1.5 0 0 1 14 4.5v5a1.5 1.5 0 0 1-1.5 1.5H8l-3 2.5V11H3.5A1.5 1.5 0 0 1 2 9.5v-5A1.5 1.5 0 0 1 3.5 3z"/>',
};

/** A header button holding an icon (static markup from ICONS). */
export function iconButton(
  id: string,
  label: string,
  icon: string,
  keys?: string,
): HTMLButtonElement {
  const button = h("button", {
    type: "button",
    id,
    class: "icon-btn",
    title: keys === undefined ? label : `${label} (${keys})`,
    "aria-label": label,
  });
  button.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">${icon}</svg>`;
  return button;
}
