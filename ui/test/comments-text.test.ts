// The UI's side of the visible-text contract with core (review/text.rs), on HTML copied from the
// renderer's snapshots (crates/lectern-core/tests/snapshots).
import { afterEach, describe, expect, it } from "vitest";
import {
  blockForLine,
  blockLines,
  buildTextIndex,
  docText,
  elementsForLines,
  leafBlockOf,
  locate,
  quoteFromRange,
} from "../src/comments-text";

/** From friends/readme-style.md: an alert, a footnote and its back-reference, a code block. */
const ALERT = `<div class="markdown-alert markdown-alert-note" data-sourcepos="11:1-12:50">
<p class="markdown-alert-title">Note</p>
<p data-sourcepos="12:3-12:50">Lectern only reads notes. It never changes them.</p>
</div>`;
const FOOTNOTE_REF = `<p data-sourcepos="17:1-17:33">Every claim here is invented.<sup data-sourcepos="17:30-17:33" class="footnote-ref"><a href="#fn-1" id="fnref-1" data-footnote-ref="">1</a></sup></p>`;
const CODE = `<div class="code-block" data-lang="mermaid" data-sourcepos="25:1-29:3"><div class="code-head"><span class="code-lang">mermaid</span><button type="button" class="code-copy" aria-label="Copy code">Copy</button></div><div class="code-note">Diagram rendering isn't supported yet</div><pre><code class="language-mermaid">graph TD
  Library --&gt; Index
  Index --&gt; Render
</code></pre></div>`;
const FOOTNOTES = `<section data-sourcepos="35:1-35:30" class="footnotes" data-footnotes="">
<ol>
<li data-sourcepos="35:1-35:30" id="fn-1">
<p data-sourcepos="35:7-35:30">Including this footnote. <a href="#fnref-1" class="footnote-backref" data-footnote-backref="" data-footnote-backref-idx="1" aria-label="Back to reference 1">↩</a></p>
</li>
</ol>
</section>`;
/** From work/alpha/README.md: task items, and a table row with raw `<br>`. */
const TASKS = `<ul data-sourcepos="12:1-15:28">
<li data-sourcepos="12:1-12:20"><input type="checkbox" checked="" disabled=""> Draft the plan</li>
<li data-sourcepos="15:1-16:0"><input type="checkbox" disabled=""> Archive the old branch</li>
</ul>`;
const TABLE = `<div class="table-wrap"><table data-sourcepos="19:1-22:77">
<thead>
<tr data-sourcepos="19:1-19:16">
<th data-sourcepos="19:2-19:7">File</th>
<th data-sourcepos="19:9-19:15">Notes</th>
</tr>
</thead>
<tbody>
<tr data-sourcepos="21:1-21:69">
<td data-sourcepos="21:2-21:33"><code data-sourcepos="21:3-21:32">plans/2026-01-01-big-plan.md</code></td>
<td data-sourcepos="21:35-21:68">The full plan<br>with every step</td>
</tr>
<tr data-sourcepos="22:1-22:77">
<td data-sourcepos="22:2-22:30"><code data-sourcepos="22:3-22:29">notes/2026-01-02-notes.md</code></td>
<td data-sourcepos="22:32-22:76">Pipes in a cell: <code data-sourcepos="22:50-22:56">a | b</code>, and either | or</td>
</tr>
</tbody>
</table></div>`;
/** From prompts/writer.md: an allowed raw HTML block (no sourcepos) and raw inline tags. */
const RAW_HTML = `<details><summary>More</summary>hidden</details>
<p data-sourcepos="22:1-22:51">Press <kbd>Ctrl</kbd> and <kbd>Enter</kbd> to send.</p>`;
/**
 * A note with two footnotes, as core renders it: the footnotes' section carries the lines of the
 * first definition only.
 */
const TWO_FOOTNOTES = `<h1 id="sources" data-sourcepos="1:1-1:9">Sources</h1>
<p data-sourcepos="3:1-3:56">The sync runs nightly.<sup data-sourcepos="3:23-3:26" class="footnote-ref"><a href="#fn-1" id="fnref-1" data-footnote-ref="">1</a></sup> It batches fifty changes.<sup data-sourcepos="3:53-3:56" class="footnote-ref"><a href="#fn-2" id="fnref-2" data-footnote-ref="">2</a></sup></p>
<section data-sourcepos="5:1-5:31" class="footnotes" data-footnotes="">
<ol>
<li data-sourcepos="5:1-5:31" id="fn-1">
<p data-sourcepos="5:7-5:31">Since the spring release. <a href="#fnref-1" class="footnote-backref" data-footnote-backref="" data-footnote-backref-idx="1" aria-label="Back to reference 1">↩</a></p>
</li>
<li data-sourcepos="7:1-7:60" id="fn-2">
<p data-sourcepos="7:7-7:60">The batch size came from the load test on the old hub. <a href="#fnref-2" class="footnote-backref" data-footnote-backref="" data-footnote-backref-idx="2" aria-label="Back to reference 2">↩</a></p>
</li>
</ol>
</section>`;

/** From stress/long-lines.md: nested tight lists. */
const NESTED = `<ul data-sourcepos="69:1-75:17">
<li data-sourcepos="69:1-74:21">Level one
<ul data-sourcepos="70:3-74:21">
<li data-sourcepos="70:3-73:45">Level two, indented by two
<ul data-sourcepos="71:6-73:45">
<li data-sourcepos="71:6-73:45">Level three, indented by three more
<ul data-sourcepos="72:10-73:45">
<li data-sourcepos="72:10-73:45">Level four, indented by four more</li>
</ul>
</li>
</ul>
</li>
</ul>
</li>
</ul>`;

function doc(html: string): HTMLElement {
  document.body.innerHTML = `<article class="doc">${html}</article>`;
  const el = document.querySelector<HTMLElement>(".doc");
  if (!el) throw new Error("no doc");
  return el;
}

function textIn(root: Element, needle: string): Text {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n instanceof Text && n.data.includes(needle)) return n;
  }
  throw new Error(`no text holding ${needle}`);
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("comments text", () => {
  it("docText skips chrome and collapses whitespace", () => {
    const el = doc(
      [ALERT, FOOTNOTE_REF, TASKS, TABLE, CODE, RAW_HTML, FOOTNOTES].join("\n") +
        // A failed image shows its alt text in a placeholder, which core never sees.
        `<p data-sourcepos="40:1-40:20"><a href="#"><span class="img-placeholder">build</span></a> after</p>`,
    );
    expect(docText(el)).toBe(
      [
        "Lectern only reads notes. It never changes them.",
        "Every claim here is invented.",
        "Draft the plan",
        "Archive the old branch",
        "File Notes",
        "plans/2026-01-01-big-plan.md The full planwith every step",
        "notes/2026-01-02-notes.md Pipes in a cell: a | b, and either | or",
        "graph TD Library --> Index Index --> Render",
        "Press Ctrl and Enter to send.",
        "Including this footnote.",
        "after",
      ].join(" "),
    );
  });

  it("maps every character of the text to its node", () => {
    const el = doc(`<p data-sourcepos="1:1-1:30">One  <em>two</em>\n three</p>`);
    const index = buildTextIndex([el]);
    expect(index.text).toBe("One two three");
    const at = (i: number) => index.nodes[index.nodeOf[i] ?? -1]?.data[index.offOf[i] ?? -1];
    for (let i = 0; i < index.text.length; i++) {
      if (index.text[i] !== " ") expect(at(i), `character ${String(i)}`).toBe(index.text[i]);
    }
  });

  it("locate returns a range across inline elements", () => {
    const el = doc(
      `<p data-sourcepos="1:1-1:60">It sends <strong data-sourcepos="1:10-1:30">at most <em data-sourcepos="1:20-1:28">fifty</em></strong>\nchanges per batch.</p>`,
    );
    const range = locate(buildTextIndex([el]), "at most fifty changes");
    expect(range?.toString()).toBe("at most fifty\nchanges");
    expect(range?.startContainer).toBe(textIn(el, "at most"));
    expect(range?.endContainer).toBe(textIn(el, "changes per"));
    expect(locate(buildTextIndex([el]), "at most fifty   changes")?.toString()).toBe(
      "at most fifty\nchanges",
    );
    expect(locate(buildTextIndex([el]), "not there")).toBeNull();
  });

  it("quote from a selection skips code-block chrome", () => {
    const el = doc(`<p data-sourcepos="23:1-23:20">The diagram follows:</p>\n${CODE}`);
    const range = document.createRange();
    range.setStart(textIn(el, "diagram follows"), 4);
    const code = textIn(el, "graph TD");
    range.setEnd(code, code.data.indexOf("Library") + "Library".length);
    const quote = quoteFromRange(range);
    expect(quote).toBe("diagram follows: graph TD Library");
    expect(quote).not.toContain("mermaid");
    expect(quote).not.toContain("Copy");
  });

  it("blockLines handles end column 0", () => {
    const el = doc(`${TASKS}<div class="code-block" data-sourcepos="11:2-51:0"></div>`);
    const items = el.querySelectorAll("li");
    expect(blockLines(items[0] as Element)).toEqual([12, 12]);
    expect(blockLines(items[1] as Element)).toEqual([15, 15]);
    expect(blockLines(el.querySelector(".code-block") as Element)).toEqual([11, 50]);
    expect(blockLines(el)).toBeNull();
  });

  it("blockForLine picks the deepest block", () => {
    const el = doc(`${NESTED}\n${TABLE}`);
    expect(blockForLine(el, 72)?.getAttribute("data-sourcepos")).toBe("72:10-73:45");
    expect(blockForLine(el, 70)?.getAttribute("data-sourcepos")).toBe("70:3-73:45");
    expect(blockForLine(el, 69)?.getAttribute("data-sourcepos")).toBe("69:1-74:21");
    // A table's row, not its cell.
    expect(blockForLine(el, 21)?.tagName).toBe("TR");
    expect(blockForLine(el, 99)).toBeNull();
  });

  it("leafBlockOf finds the block a text node counts in", () => {
    const el = doc(`${CODE}\n${TABLE}\n${RAW_HTML}`);
    expect(leafBlockOf(textIn(el, "graph TD"), el)?.className).toBe("code-block");
    expect(leafBlockOf(textIn(el, "with every step"), el)?.tagName).toBe("TD");
    expect(leafBlockOf(textIn(el, "hidden"), el)).toBeNull();
    expect(leafBlockOf(textIn(el, "Ctrl"), el)?.tagName).toBe("P");
  });

  it("elementsForLines keeps a list item's own text when its nested list is partly covered", () => {
    // stress/long-lines.md L69–L70: the item's own line and the first line of its nested list.
    const el = doc(NESTED);
    const range = locate(
      buildTextIndex(elementsForLines(el, 69, 70)),
      "Level one Level two, indented by two",
    );
    expect(range?.toString().replace(/\s+/g, " ")).toBe("Level one Level two, indented by two");
    // An item with text only in its nested list's lines still narrows to that list's item.
    expect(elementsForLines(el, 72, 72).map((e) => e.getAttribute("data-sourcepos"))).toEqual([
      "72:10-73:45",
    ]);
  });

  it("finds a later footnote, whatever lines the footnotes' section carries", () => {
    const el = doc(TWO_FOOTNOTES);
    // Core's text for this note, block by block.
    expect(docText(el)).toBe(
      "Sources The sync runs nightly. It batches fifty changes. Since the spring release. The batch size came from the load test on the old hub.",
    );
    const range = locate(buildTextIndex(elementsForLines(el, 7, 7)), "load test on the old hub");
    expect(range?.toString()).toBe("load test on the old hub");
    expect(blockForLine(el, 7)?.getAttribute("data-sourcepos")).toBe("7:7-7:60");
    expect(blockForLine(el, 5)?.getAttribute("data-sourcepos")).toBe("5:7-5:31");
    expect(elementsForLines(el, 5, 7).map((e) => e.getAttribute("data-sourcepos"))).toEqual([
      "5:1-5:31",
      "7:1-7:60",
    ]);
  });

  it("elementsForLines unwraps table-wrap", () => {
    const el = doc(`${ALERT}\n${TABLE}\n${CODE}`);
    const rows = elementsForLines(el, 21, 22);
    expect(rows.map((r) => r.tagName)).toEqual(["TR", "TR"]);
    const whole = elementsForLines(el, 19, 22);
    expect(whole.map((r) => r.tagName)).toEqual(["TABLE"]);
    expect(elementsForLines(el, 12, 12).map((r) => r.getAttribute("data-sourcepos"))).toEqual([
      "12:3-12:50",
    ]);
    expect(elementsForLines(el, 22, 26).map((r) => r.tagName)).toEqual(["TR", "DIV"]);
    expect(elementsForLines(el, 90, 99)).toEqual([]);
  });
});
