// An agent's requests to a window (src/agent.ts): what `status` hears from it about what it shows,
// for a note or text file (the heading path and line at the top of the view, and the selection), an
// HTML page (from its session), a PDF and nothing; and the round trip through the app.
import { afterEach, describe, expect, it } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { NOT_YET, answer, view, type AgentHost } from "../src/agent";
import { App } from "../src/app";
import type { DocKind } from "../src/generated/DocKind";
import type { DocPayload } from "../src/generated/DocPayload";
import type { OutlineItem } from "../src/generated/OutlineItem";
import { A, appRoot, docPayload, fixtures, rendered } from "./helpers";

const PLAN = "S:\\Notes\\My Vault\\plan.md";
/** The pane's height, in pixels. */
const VIEW = 500;

/**
 * A document laid out by hand, as jsdom has no layout: each block below the one before it, at the
 * height given, moving with the pane's scrollTop.
 */
function laidOut(blocks: [html: string, height: number][]) {
  document.body.innerHTML = `<main id="pane"><article id="doc">${blocks.map(([html]) => html).join("")}</article></main><p id="elsewhere">Not the note.</p>`;
  const scroller = document.getElementById("pane");
  const doc = document.getElementById("doc");
  if (!scroller || !doc) throw new Error("no layout");
  let y = 0;
  [...doc.children].forEach((el, i) => {
    const top = y;
    const height = blocks[i]?.[1] ?? 0;
    const box = () => {
      const t = top - scroller.scrollTop;
      return { top: t, bottom: t + height, height } as DOMRect;
    };
    el.getBoundingClientRect = box;
    // A heading's box is its block's.
    for (const inner of el.querySelectorAll("*")) inner.getBoundingClientRect = box;
    y += height;
  });
  scroller.getBoundingClientRect = () => ({ top: 0, bottom: VIEW, height: VIEW }) as DOMRect;
  return { doc, scroller };
}

function host(
  kind: DocKind,
  layout: { doc: HTMLElement; scroller: HTMLElement },
  outline: OutlineItem[] = [],
  page: ReturnType<AgentHost["page"]> = null,
): AgentHost {
  const doc: DocPayload = { ...docPayload(null), path: PLAN, kind, outline };
  return {
    doc: () => doc,
    docElement: () => layout.doc,
    scroller: () => layout.scroller,
    page: () => page,
  };
}

/** Plan › Rollout › Steps, with Risks after. */
function plan() {
  return laidOut([
    ['<h1 id="plan" data-sourcepos="1:1-1:6">Plan</h1>', 40],
    ['<p data-sourcepos="3:1-3:9">Intro.</p>', 200],
    [
      '<h2 id="rollout" data-sourcepos="5:1-5:10">Rollout<a class="anchor" href="#rollout">#</a></h2>',
      40,
    ],
    ['<p id="steps-text" data-sourcepos="7:1-8:20">Roll back the migration first.</p>', 300],
    ['<h3 id="steps" data-sourcepos="10:1-10:9">Steps</h3>', 40],
    ['<p data-sourcepos="12:1-12:9">One.</p>', 300],
    ['<h2 id="risks" data-sourcepos="14:1-14:8">Risks</h2>', 40],
    ['<p data-sourcepos="16:1-16:9">None.</p>', 400],
  ]);
}

const OUTLINE: OutlineItem[] = [
  { level: 1, text: "Plan", id: "plan" },
  { level: 2, text: "Rollout", id: "rollout" },
  { level: 3, text: "Steps", id: "steps" },
  { level: 2, text: "Risks", id: "risks" },
];

function select(node: Node, from: number, to: number, endNode: Node = node): void {
  const range = document.createRange();
  range.setStart(node, from);
  range.setEnd(endNode, to);
  const selection = document.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function textOf(id: string): Text {
  const text = document.getElementById(id)?.firstChild;
  if (!(text instanceof Text)) throw new Error(`no text in #${id}`);
  return text;
}

afterEach(() => {
  document.getSelection()?.removeAllRanges();
  document.body.replaceChildren();
});

describe("what a window shows", () => {
  it("gives a note's heading path and line at the top of the view, by the outline's text", () => {
    const layout = plan();
    layout.scroller.scrollTop = 300;
    expect(view(host("markdown", layout, OUTLINE))).toEqual({
      path: PLAN,
      heading: ["Plan", "Rollout"],
      line: 7,
      selection: null,
    });
    // Further down, the path leaves Steps behind for Risks.
    layout.scroller.scrollTop = 940;
    expect(view(host("markdown", layout, OUTLINE)).heading).toEqual(["Plan", "Risks"]);
    // Shown text stands in for a heading the outline lacks.
    layout.scroller.scrollTop = 300;
    expect(view(host("markdown", layout)).heading).toEqual(["Plan", "Rollout#"]);
  });

  it("names the heading the outline marks: the first in the top of the view, else the last above", () => {
    const layout = plan();
    // Risks landed 12 px below the top, as navigating to it leaves it: Risks, not the section before.
    layout.scroller.scrollTop = 920 - 12;
    expect(view(host("markdown", layout, OUTLINE)).heading).toEqual(["Plan", "Risks"]);
    // Rollout with Steps right under it, both in the top of the view: Rollout, the first.
    const close = laidOut([
      ['<h1 id="plan" data-sourcepos="1:1-1:6">Plan</h1>', 40],
      ['<p data-sourcepos="3:1-3:9">Intro.</p>', 200],
      ['<h2 id="rollout" data-sourcepos="5:1-5:10">Rollout</h2>', 40],
      ['<h3 id="steps" data-sourcepos="7:1-7:9">Steps</h3>', 40],
      ['<p data-sourcepos="9:1-9:9">One.</p>', 800],
    ]);
    close.scroller.scrollTop = 240 - 12;
    expect(view(host("markdown", close, OUTLINE)).heading).toEqual(["Plan", "Rollout"]);
    // Still Rollout while any of it shows; once it is scrolled past, Steps.
    close.scroller.scrollTop = 270;
    expect(view(host("markdown", close, OUTLINE)).heading).toEqual(["Plan", "Rollout"]);
    close.scroller.scrollTop = 290;
    expect(view(host("markdown", close, OUTLINE)).heading).toEqual(["Plan", "Rollout", "Steps"]);
    // A heading below the top 30% of the view isn't being read yet: the last scrolled past is.
    close.scroller.scrollTop = 240 - 160;
    expect(view(host("markdown", close, OUTLINE)).heading).toEqual(["Plan"]);
  });

  it("gives the first heading above every one, when it is in view", () => {
    const layout = laidOut([
      ['<p data-sourcepos="1:1-1:9">Before.</p>', 100],
      ['<h1 id="plan" data-sourcepos="3:1-3:6">Plan</h1>', 40],
      ['<p data-sourcepos="5:1-5:9">After.</p>', 1000],
    ]);
    expect(view(host("markdown", layout, OUTLINE))).toMatchObject({ heading: ["Plan"], line: 1 });
    const far = laidOut([
      ['<p data-sourcepos="1:1-1:9">Before.</p>', 900],
      ['<h1 id="plan" data-sourcepos="3:1-3:6">Plan</h1>', 40],
    ]);
    expect(view(host("markdown", far, OUTLINE)).heading).toEqual([]);
  });

  it("gives the line of a wrapped block at the top, such as a table, from inside its wrapper", () => {
    const layout = laidOut([
      ['<p data-sourcepos="1:1-1:9">Intro.</p>', 100],
      [
        '<div class="table-wrap"><table data-sourcepos="3:1-5:12"><tbody><tr data-sourcepos="5:1-5:12"><td>a</td></tr></tbody></table></div>',
        300,
      ],
      ['<p data-sourcepos="7:1-7:9">After.</p>', 400],
    ]);
    layout.scroller.scrollTop = 150;
    expect(view(host("markdown", layout)).line).toBe(3);
  });

  it("leaves a heading out of the path when it isn't shown, as in a closed details", () => {
    const layout = laidOut([
      ['<h1 id="plan" data-sourcepos="1:1-1:6">Plan</h1>', 40],
      ['<details open><summary>Open</summary><h2 id="open">Open part</h2></details>', 40],
      [
        '<details><summary>More</summary><h3 id="folded" data-sourcepos="5:1-5:10">Folded</h3></details>',
        40,
      ],
      ['<h4 id="hidden" data-sourcepos="9:1-9:10">Hidden</h4>', 40],
      ['<p data-sourcepos="11:1-11:9">Text.</p>', 800],
    ]);
    const hidden = document.getElementById("hidden");
    if (!hidden) throw new Error("no heading");
    // Not rendered (display: none, say), as the browser reports it.
    hidden.checkVisibility = () => false;
    layout.scroller.scrollTop = 200;
    expect(view(host("markdown", layout)).heading).toEqual(["Plan", "Open part"]);
  });

  it("gives the reader's selection in the note, with its lines, and none elsewhere", () => {
    const layout = plan();
    select(textOf("steps-text"), 0, 8);
    expect(view(host("markdown", layout, OUTLINE)).selection).toEqual({
      quote: "Roll bac",
      startLine: 7,
      endLine: 8,
    });
    select(textOf("elsewhere"), 0, 3);
    expect(view(host("markdown", layout, OUTLINE)).selection).toBeNull();
    document.getSelection()?.removeAllRanges();
    expect(view(host("markdown", layout, OUTLINE)).selection).toBeNull();
  });

  it("reads a text file as a note, by its paragraphs' lines", () => {
    const layout = laidOut([
      ['<p class="lx-plain" data-sourcepos="1:1-2:5">first</p>', 300],
      ['<p class="lx-plain" id="second" data-sourcepos="4:1-6:3">second</p>', 300],
    ]);
    layout.scroller.scrollTop = 350;
    select(textOf("second"), 0, 6);
    expect(view(host("text", layout))).toEqual({
      path: PLAN,
      heading: [],
      line: 4,
      selection: { quote: "second", startLine: 4, endLine: 6 },
    });
  });

  it("gives an HTML page's heading path and selection from its session, without lines", () => {
    const layout = plan();
    const page = {
      headingPath: () => ["Weekly report", "Results"],
      selection: {
        quote: "Rendered by",
        start: 22,
        end: 33,
        rect: { x: 0, y: 0, width: 1, height: 1 },
      },
    };
    expect(view(host("html", layout, [], page))).toEqual({
      path: PLAN,
      heading: ["Weekly report", "Results"],
      line: null,
      selection: { quote: "Rendered by", startLine: null, endLine: null },
    });
    // Before its session starts, only the file.
    expect(view(host("html", layout))).toEqual({
      path: PLAN,
      heading: [],
      line: null,
      selection: null,
    });
  });

  it("gives a PDF's file alone, and nothing for no file", () => {
    const layout = plan();
    select(textOf("steps-text"), 0, 4);
    expect(view(host("pdf", layout))).toEqual({
      path: PLAN,
      heading: [],
      line: null,
      selection: null,
    });
    expect(view({ ...host("markdown", layout), doc: () => null })).toEqual({
      path: null,
      heading: [],
      line: null,
      selection: null,
    });
  });

  it("answers status with the view, and the other tools as not available yet", () => {
    const layout = plan();
    expect(answer(host("pdf", layout), { tool: "status" })).toEqual({
      kind: "view",
      path: PLAN,
      heading: [],
      line: null,
      selection: null,
    });
    expect(answer(host("pdf", layout), { tool: "read" })).toEqual({
      kind: "failed",
      message: NOT_YET,
    });
  });
});

describe("an agent's request through the app", () => {
  it("is answered with what the window shows, by the request's id", async () => {
    const fake = new FakeBackend(
      fixtures({ [A]: rendered("Aye", "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye</h1>") }),
      { initial: A },
    );
    const app = new App(fake, appRoot());
    await app.start();
    const reply = await fake.agentRequest({ tool: "status" });
    expect(reply).toMatchObject({ kind: "view", path: A, selection: null });
    expect(await fake.agentRequest({ tool: "comments", path: A })).toEqual({
      kind: "failed",
      message: NOT_YET,
    });
  });
});
