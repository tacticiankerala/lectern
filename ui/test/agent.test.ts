// An agent's requests to a window (src/agent.ts): what `status` hears from it about what it shows,
// for a note or text file (the heading path and line at the top of the view, and the selection), an
// HTML page (from its session), a PDF and nothing; the round trip through the app; and `show`,
// which asks first with the bar when the window shows another file, and the agent settings in
// Preferences.
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBackend, type FakeOptions } from "../dev/backend-fake";
import { NOT_YET, view, type DocHost } from "../src/agent";
import { App } from "../src/app";
import type { AgentRequest } from "../src/generated/AgentRequest";
import type { DocKind } from "../src/generated/DocKind";
import type { DocPayload } from "../src/generated/DocPayload";
import type { OutlineItem } from "../src/generated/OutlineItem";
import type { Target } from "../src/generated/Target";
import { A, B, C, appRoot, docPayload, fixtures, rendered, settle } from "./helpers";

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
  page: ReturnType<DocHost["page"]> = null,
): DocHost {
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
    expect(reply).toMatchObject({
      kind: "view",
      path: A,
      selection: null,
      ask: null,
      answers: [],
    });
    expect(await fake.agentRequest({ tool: "comments", path: A })).toEqual({
      kind: "failed",
      message: NOT_YET,
    });
  });
});

/** The app, started on `initial` (none: the welcome screen), with A, B and C to open. */
async function launched(initial: string | null = A, options: FakeOptions = {}) {
  const fake = new FakeBackend(
    fixtures({
      [B]: rendered(
        "Bee",
        "<h1 id='bee' data-sourcepos='1:1-1:5'>Bee</h1><p data-sourcepos='3:1-3:9'>Buzz.</p>",
      ),
    }),
    initial === null ? options : { ...options, initial },
  );
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

function show(path: string, target?: Target): AgentRequest {
  return { tool: "show", path, newWindow: false, wait: 0, ...(target ? { target } : {}) };
}

/** The ask bar, while it shows. */
function bar(): HTMLElement | null {
  const el = document.getElementById("lx-agent-ask");
  return el && !el.hidden ? el : null;
}

function barButton(name: string): HTMLButtonElement {
  const button = [...(bar()?.querySelectorAll("button") ?? [])].find((b) => b.textContent === name);
  if (!button) throw new Error(`no ${name} on the bar`);
  return button;
}

describe("an agent's show", () => {
  it("asks first when the window shows another file: Show opens it, and Back returns", async () => {
    const { fake, app } = await launched();
    expect(await fake.agentRequest(show(B, { heading: "Bee › Buzz" }))).toEqual({
      kind: "asked",
    });
    // The file on screen stays until the reader answers.
    expect(app.state.doc?.path).toBe(A);
    expect(bar()?.querySelector(".agent-ask-text")?.textContent).toBe(
      "Claude wants to show b.md › Bee › Buzz",
    );
    expect(bar()?.querySelector(".agent-ask-file")?.getAttribute("title")).toBe(B);
    barButton("Show").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
    expect(bar()).toBeNull();
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "shown" }]);
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(A);
  });

  it("names the agent as Rust does, and Not now leaves the file on screen", async () => {
    const { fake, app } = await launched();
    await fake.agentRequest(show(B), "Codex");
    expect(bar()?.textContent).toContain("Codex wants to show b.md");
    barButton("Not now").click();
    await settle();
    expect(bar()).toBeNull();
    expect(app.state.doc?.path).toBe(A);
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "dismissed" }]);
    expect(await fake.agentRequest({ tool: "status" })).toMatchObject({
      ask: null,
      answers: [{ path: B, answer: "dismissed" }],
    });
  });

  it("offers only the newest request: the older ones are replaced, as status tells", async () => {
    const { fake } = await launched();
    for (const path of [B, C, B]) {
      expect(await fake.agentRequest(show(path))).toEqual({ kind: "asked" });
    }
    expect(bar()?.querySelector(".agent-ask-text")?.textContent).toBe("Claude wants to show b.md");
    expect(fake.agentAnswers).toEqual([
      { id: 1, answer: "replaced" },
      { id: 2, answer: "replaced" },
    ]);
    expect(await fake.agentRequest({ tool: "status" })).toMatchObject({
      path: A,
      ask: B,
      answers: [
        { path: B, answer: "replaced" },
        { path: C, answer: "replaced" },
      ],
    });
  });

  it("switches at once when the setting says so, replacing a request still asking", async () => {
    const { fake, app } = await launched();
    await fake.agentRequest(show(C));
    fake.settingsElsewhere({ agentOpens: "switch" });
    expect(await fake.agentRequest(show(B))).toEqual({ kind: "shown" });
    expect(app.state.doc?.path).toBe(B);
    expect(bar()).toBeNull();
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "replaced" }]);
    // On the history like any other open.
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(A);
  });

  it("opens without asking in a window that shows nothing", async () => {
    const { fake, app } = await launched(null);
    expect(app.state.doc).toBeNull();
    expect(await fake.agentRequest(show(B))).toEqual({ kind: "shown" });
    expect(app.state.doc?.path).toBe(B);
    expect(bar()).toBeNull();
  });

  it("moves within the file on screen without asking", async () => {
    const { fake, app } = await launched(B);
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B.toUpperCase(), { line: 3 }))).toEqual({
      kind: "shown",
    });
    expect(scrolled).toHaveBeenCalledWith(3);
    expect(bar()).toBeNull();
    expect(app.state.doc?.path).toBe(B);
  });

  it("opens at a line target, and says why a file didn't open", async () => {
    const { fake, app } = await launched(null);
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { line: 3 }))).toEqual({ kind: "shown" });
    expect(scrolled).toHaveBeenCalledWith(3);
    fake.settingsElsewhere({ agentOpens: "switch" });
    const gone = "C:\\V\\gone.md";
    expect(await fake.agentRequest(show(gone))).toEqual({
      kind: "failed",
      message: `gone.md didn't open: Couldn't find ${gone}`,
    });
  });

  it("keeps the bar across the reader's own moves until it is answered", async () => {
    const { fake, app } = await launched();
    await fake.agentRequest(show(B));
    await app.open(C, { push: true });
    expect(bar()?.textContent).toContain("b.md");
    barButton("Show").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
  });
});

describe("Preferences › Agents", () => {
  it("has the two agent settings as choices, saved for every window", async () => {
    const { fake, app } = await launched();
    expect(app.actions.run("preferences")).toBe(true);
    const section = await vi.waitFor(() => {
      const found = [...document.querySelectorAll<HTMLElement>(".prefs-section")].find(
        (s) => s.querySelector("h4")?.textContent === "Agents",
      );
      if (!found) throw new Error("no Agents section");
      return found;
    });
    const opens = [...section.querySelectorAll<HTMLInputElement>('input[name="lx-agent-opens"]')];
    const focus = [...section.querySelectorAll<HTMLInputElement>('input[name="lx-agent-focus"]')];
    // Each group named, as the design words them.
    expect(opens[0]?.closest("[role=radiogroup]")?.getAttribute("aria-label")).toBe(
      "When an agent opens a file",
    );
    expect(focus[0]?.closest("[role=radiogroup]")?.getAttribute("aria-label")).toBe(
      "When an agent shows something",
    );
    expect(opens.map((c) => c.closest("label")?.textContent)).toEqual(["Ask", "Switch right away"]);
    expect(focus.map((c) => c.closest("label")?.textContent)).toEqual([
      "Bring Lectern to front",
      "Show without taking focus",
      "Leave windows alone",
    ]);
    expect(section.textContent).toContain("only flashes the taskbar");
    expect(opens.map((c) => c.checked)).toEqual([true, false]);
    expect(focus.map((c) => c.checked)).toEqual([true, false, false]);
    opens[1]?.click();
    focus[2]?.click();
    await settle();
    expect(fake.sharedSettings()).toMatchObject({ agentOpens: "switch", agentFocus: "none" });
    // Changed elsewhere, they show here.
    fake.settingsElsewhere({ agentOpens: "ask", agentFocus: "raise" });
    expect(opens.map((c) => c.checked)).toEqual([true, false]);
    expect(focus.map((c) => c.checked)).toEqual([false, true, false]);
  });
});
