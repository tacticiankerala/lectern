import { describe, expect, it } from "vitest";
import { selButtonPlace } from "../src/comment-adding";

/** The pane: from x 300 to 1000 and y 50 of the window, scrolled 400 down. */
const PANE = { left: 300, top: 50, clientWidth: 700, scrollLeft: 0, scrollTop: 400 };
const SIZE = { width: 90, height: 24 };

describe("selButtonPlace", () => {
  it("puts the button on the selection's last line, just after its end", () => {
    const end = { left: 400, right: 600, top: 200, bottom: 230 };
    expect(selButtonPlace(end, SIZE, PANE)).toEqual({
      // 6 px after the end, in the pane's content.
      left: 600 - 300 + 6,
      // Centred on the line, scrolled with the text.
      top: 215 - 12 - 50 + 400,
    });
  });

  it("flips before the end when it would overflow the pane's right edge", () => {
    const end = { left: 700, right: 950, top: 200, bottom: 230 };
    const at = selButtonPlace(end, SIZE, PANE);
    expect(at.left).toBe(950 - 300 - 6 - 90);
    expect(at.left + SIZE.width).toBeLessThanOrEqual(PANE.clientWidth);
    expect(at.top).toBe(215 - 12 - 50 + 400);
  });

  it("never goes past the pane's left edge", () => {
    const end = { left: 300, right: 340, top: 200, bottom: 230 };
    expect(selButtonPlace(end, { width: 700, height: 24 }, PANE).left).toBe(8);
  });
});
