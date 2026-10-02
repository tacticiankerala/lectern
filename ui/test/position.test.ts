import { describe, expect, it } from "vitest";
import { capture, restore } from "../src/position";

/** The pane's height, in pixels. */
const VIEW = 500;

/**
 * A document laid out by hand, as jsdom has no layout: each block sits below the one before it,
 * at the height given, and moves with the pane's scrollTop.
 */
function laidOut(blocks: [html: string, height: number][]) {
  document.body.innerHTML = `<main id="pane"><article id="doc">${blocks.map(([html]) => html).join("")}</article></main>`;
  const scroller = document.getElementById("pane");
  const doc = document.getElementById("doc");
  if (!scroller || !doc) {
    throw new Error("no layout");
  }
  let y = 0;
  [...doc.children].forEach((el, i) => {
    const top = y;
    const height = blocks[i]?.[1] ?? 0;
    el.getBoundingClientRect = () => {
      const t = top - scroller.scrollTop;
      return { top: t, bottom: t + height, height } as DOMRect;
    };
    y += height;
  });
  scroller.scrollTop = 0;
  // The text starts at the top of the pane.
  doc.getBoundingClientRect = () => ({ top: -scroller.scrollTop }) as DOMRect;
  scroller.getBoundingClientRect = () => ({ top: 0, bottom: VIEW, height: VIEW }) as DOMRect;
  Object.defineProperty(scroller, "clientHeight", { value: VIEW, configurable: true });
  Object.defineProperty(scroller, "scrollHeight", { value: y, configurable: true });
  return { doc, scroller };
}

/** Headings at 0 and 340, paragraphs between and after: 980 px in all. */
function plan() {
  return laidOut([
    ['<h2 id="a" data-sourcepos="1:1-1:4">A</h2>', 40],
    ['<p data-sourcepos="3:1-3:9">one</p>', 300],
    ['<h2 id="b" data-sourcepos="5:1-5:4">B</h2>', 40],
    ['<p data-sourcepos="7:1-9:9">two</p>', 600],
  ]);
}

describe("position", () => {
  it("captures the nearest heading above the top and the offset below it, the top block's line and the fraction", () => {
    const { doc, scroller } = plan();
    scroller.scrollTop = 400;
    expect(capture(doc, scroller)).toEqual({
      headingId: "b",
      offset: 60,
      line: 7,
      fraction: 400 / (980 - VIEW),
    });
  });

  it("keeps only the fraction above the text, so the properties strip stays in view", () => {
    const { doc, scroller } = plan();
    // The text starts 100 px down the pane, below the properties strip.
    doc.getBoundingClientRect = () => ({ top: 100 - scroller.scrollTop }) as DOMRect;
    expect(capture(doc, scroller)).toEqual({ headingId: null, offset: 0, line: null, fraction: 0 });
    scroller.scrollTop = 48;
    expect(capture(doc, scroller)).toEqual({
      headingId: null,
      offset: 0,
      line: null,
      fraction: 48 / (980 - VIEW),
    });
  });

  it("restores by heading id first", () => {
    const { doc, scroller } = plan();
    restore(doc, scroller, { headingId: "b", offset: 25, line: 3, fraction: 0.9 });
    // B sits 25 px above the pane's top.
    expect(scroller.scrollTop).toBe(365);
  });

  it("falls back to sourcepos line", () => {
    const { doc, scroller } = plan();
    // No heading has that id: the block starting last at or before line 8 goes to the top.
    restore(doc, scroller, { headingId: "gone", offset: 25, line: 8, fraction: 0.9 });
    expect(scroller.scrollTop).toBe(380);
  });

  it("falls back to fraction", () => {
    const { doc, scroller } = plan();
    restore(doc, scroller, { headingId: null, offset: 0, line: null, fraction: 0.5 });
    expect(scroller.scrollTop).toBe(240);
    // A line before every block finds none either.
    scroller.scrollTop = 0;
    restore(doc, scroller, { headingId: "gone", offset: 0, line: 0, fraction: 0.25 });
    expect(scroller.scrollTop).toBe(120);
  });
});
