import { describe, expect, it } from "vitest";
import type { SavedPosition } from "../src/generated/SavedPosition";
import { History, type HistoryEntry } from "../src/history";

const at = (line: number): SavedPosition => ({ headingId: null, offset: 0, line, fraction: 0 });
const entry = (path: string, line?: number): HistoryEntry => ({
  path,
  position: line === undefined ? null : at(line),
});

describe("History", () => {
  it("starts empty", () => {
    const history = new History();
    expect(history.canBack()).toBe(false);
    expect(history.canForward()).toBe(false);
    expect(history.back(entry("a.md"))).toBeNull();
    expect(history.forward(entry("a.md"))).toBeNull();
  });

  it("push truncates forward", () => {
    const history = new History();
    history.push(entry("a.md"));
    history.push(entry("b.md"));
    // On c.md: back to b.md, leaving c.md ahead.
    expect(history.back(entry("c.md"))?.path).toBe("b.md");
    expect(history.canForward()).toBe(true);
    // Navigating somewhere new from b.md drops c.md.
    history.push(entry("b.md"));
    expect(history.canForward()).toBe(false);
    expect(history.forward(entry("d.md"))).toBeNull();
    expect(history.back(entry("d.md"))?.path).toBe("b.md");
    expect(history.back(entry("b.md"))?.path).toBe("a.md");
    expect(history.canBack()).toBe(false);
  });

  it("back then forward roundtrip restores positions", () => {
    const history = new History();
    history.push(entry("a.md", 10));
    history.push(entry("b.md", 20));
    const backToB = history.back(entry("c.md", 30));
    expect(backToB).toEqual(entry("b.md", 20));
    const backToA = history.back(entry("b.md", 25));
    expect(backToA).toEqual(entry("a.md", 10));
    // Forward returns each page where it was left, as of leaving it.
    expect(history.forward(entry("a.md", 11))).toEqual(entry("b.md", 25));
    expect(history.forward(entry("b.md", 26))).toEqual(entry("c.md", 30));
    expect(history.canForward()).toBe(false);
    expect(history.back(entry("c.md", 31))).toEqual(entry("b.md", 26));
  });

  it("collapses same-path pushes", () => {
    const history = new History();
    history.push(entry("a.md", 1));
    history.push(entry("A.MD", 2));
    history.push(entry("C:/notes/b.md", 3));
    history.push(entry("c:\\NOTES\\b.md", 4));
    expect(history.back(entry("c.md"))).toEqual(entry("c:\\NOTES\\b.md", 4));
    expect(history.back(entry("b.md"))).toEqual(entry("A.MD", 2));
    expect(history.canBack()).toBe(false);
  });

  it("keeps a bounded number of entries", () => {
    const history = new History(3);
    for (const name of ["a", "b", "c", "d", "e"]) {
      history.push(entry(`${name}.md`));
    }
    const seen: string[] = [];
    let current = entry("f.md");
    for (let back = history.back(current); back; back = history.back(current)) {
      seen.push(back.path);
      current = back;
    }
    expect(seen).toEqual(["e.md", "d.md", "c.md"]);
  });
});
