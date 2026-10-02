import { describe, expect, it } from "vitest";
import { findRanges } from "../src/find";

function doc(html: string): HTMLElement {
  document.body.innerHTML = `<article id="doc">${html}</article>`;
  const el = document.getElementById("doc");
  if (!el) {
    throw new Error("no #doc");
  }
  return el;
}

const texts = (ranges: Range[]) => ranges.map((r) => r.toString());

describe("findRanges", () => {
  it("finds across inline elements", () => {
    const root = doc("<p>hello <b>wor</b>ld</p>");
    const ranges = findRanges(root, "world");
    expect(texts(ranges)).toEqual(["world"]);
    expect(ranges[0]?.startContainer.textContent).toBe("wor");
    expect(ranges[0]?.endContainer.textContent).toBe("ld");
  });

  it("smart case: lower case ignores case, an upper-case letter makes it exact", () => {
    const root = doc("<p>Step one, step two, STEP three</p><p>Ärger, ärger</p>");
    expect(texts(findRanges(root, "step"))).toEqual(["Step", "step", "STEP"]);
    expect(texts(findRanges(root, "Step"))).toEqual(["Step"]);
    expect(texts(findRanges(root, "ärger"))).toEqual(["Ärger", "ärger"]);
    expect(texts(findRanges(root, "Ärger"))).toEqual(["Ärger"]);
  });

  it("skips code-head labels", () => {
    const root = doc(
      '<div class="code-block" data-lang="ruby"><div class="code-head"><span class="code-lang">ruby</span>' +
        '<button type="button" class="code-copy">Copy</button></div>' +
        "<pre><code>ruby -e 'copy'</code></pre></div>",
    );
    const ruby = findRanges(root, "ruby");
    expect(texts(ruby)).toEqual(["ruby"]);
    expect(ruby[0]?.startContainer.parentElement?.closest("code")).not.toBeNull();
    expect(findRanges(root, "copy")).toHaveLength(1);
  });

  it("never matches across blocks", () => {
    const root = doc("<h2>Section 1</h2>\n<p>Section 1 covers</p><p>foo</p><p>bar</p>");
    expect(findRanges(root, "1 Section")).toHaveLength(0);
    expect(findRanges(root, "foobar")).toHaveLength(0);
    expect(findRanges(root, "Section 1")).toHaveLength(2);
  });

  it("matches a line break inside a paragraph as the space it renders as", () => {
    const root = doc("<p>leaves the\nsuite green</p>");
    expect(texts(findRanges(root, "the suite"))).toEqual(["the\nsuite"]);
  });

  it("finds nothing for a blank query", () => {
    const root = doc("<p>a b c</p>");
    expect(findRanges(root, "")).toHaveLength(0);
    expect(findRanges(root, "   ")).toHaveLength(0);
  });

  it("returns matches in document order, without overlaps, up to a limit", () => {
    const root = doc("<p>aaaa</p><p>aa</p>");
    expect(findRanges(root, "aa")).toHaveLength(3);
    expect(findRanges(root, "aa", 2)).toHaveLength(2);
  });
});
