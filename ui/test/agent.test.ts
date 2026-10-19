// An agent's requests to a window (src/agent.ts): what `status` hears from it about what it shows,
// for a note or text file (the heading path and line at the top of the view, and the selection), an
// HTML page (from its session), a PDF and nothing; the round trip through the app; and `show`,
// which asks first with the bar when the window shows another file, settled once the reader's
// answer has landed, or as the window leaves; and the agent settings in Preferences, with the
// commands that connect Claude Code. Then targets:
// where core found them in a note, an HTML page's found by the window, a PDF's page, with Back
// after a move within the file on screen, and a window that started for an agent; the agent's
// highlights; and an HTML page's text for `read`.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend, type FakeOptions, type FakeWorkspace } from "../dev/backend-fake";
import {
  Agent,
  NOT_HERE,
  pageSpan,
  view,
  type AgentHost,
  type DocHost,
  type Landing,
  type PageText,
} from "../src/agent";
import { App } from "../src/app";
import { buildTextIndex } from "../src/comments-text";
import type { Passage } from "../src/doc-view";
import type { AgentOpens } from "../src/generated/AgentOpens";
import type { AgentRequest } from "../src/generated/AgentRequest";
import type { CommentStatus } from "../src/generated/CommentStatus";
import type { DocKind } from "../src/generated/DocKind";
import type { DocPayload } from "../src/generated/DocPayload";
import type { OutlineItem } from "../src/generated/OutlineItem";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import type { SavedPosition } from "../src/generated/SavedPosition";
import type { Target } from "../src/generated/Target";
import type { TextPlace } from "../src/generated/TextPlace";
import {
  A,
  B,
  C,
  ROOT,
  appRoot,
  deferred,
  docPayload,
  type Deferred,
  fixtures,
  GatedBackend,
  rendered,
  settle,
} from "./helpers";

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

// The text index, watched: a jump to a heading never builds one of the whole document.
vi.mock("../src/comments-text", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/comments-text")>();
  return { ...actual, buildTextIndex: vi.fn(actual.buildTextIndex) };
});

afterEach(() => {
  document.getSelection()?.removeAllRanges();
  document.body.replaceChildren();
  vi.unstubAllGlobals();
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
    // Rust answers `comments` itself.
    expect(await fake.agentRequest({ tool: "comments", path: A })).toEqual({
      kind: "failed",
      message: NOT_HERE,
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

  it("settles Show once the file has opened, not before", async () => {
    const { fake, app } = await launched();
    await fake.agentRequest(show(B));
    const opening = deferred();
    const open = fake.openDocument.bind(fake);
    vi.spyOn(fake, "openDocument").mockImplementationOnce(async (path, generation) => {
      await opening.promise;
      return open(path, generation);
    });
    barButton("Show").click();
    await settle();
    // The bar goes at once; the answer waits for the open.
    expect(bar()).toBeNull();
    expect(fake.agentAnswers).toEqual([]);
    opening.resolve();
    await vi.waitFor(() => {
      expect(fake.agentAnswers).toEqual([{ id: 1, answer: "shown" }]);
    });
    expect(app.state.doc?.path).toBe(B);
  });

  it("settles Show of a file that doesn't open as failed, with why", async () => {
    const { fake } = await launched();
    const gone = "C:\\V\\gone.md";
    await fake.agentRequest(show(gone));
    barButton("Show").click();
    const message = `gone.md didn't open: Couldn't find ${gone}`;
    await vi.waitFor(() => {
      expect(fake.agentAnswers).toEqual([{ id: 1, answer: "failed", message }]);
    });
    expect(await fake.agentRequest({ tool: "status" })).toMatchObject({
      ask: null,
      answers: [{ path: gone, answer: "failed", message }],
    });
  });

  it("settles as shown when the reader opens the file asked about themselves", async () => {
    const { fake, app } = await launched();
    await fake.agentRequest(show(B));
    await app.open(B, { push: true });
    expect(bar()).toBeNull();
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "shown" }]);
    expect(await fake.agentRequest({ tool: "status" })).toMatchObject({
      ask: null,
      answers: [{ path: B, answer: "shown" }],
    });
  });
});

describe("an agent's open still on its way", () => {
  /** The app with A on screen, whose opens of `slow` wait for its gate; agents' opens as `opens`. */
  async function gated(slow: string, opens: AgentOpens = "switch") {
    const fake = new GatedBackend(fixtures(), { initial: A });
    fake.startupGate.resolve();
    const app = new App(fake, appRoot());
    await app.start();
    fake.settingsElsewhere({ agentOpens: opens });
    fake.slowPath = slow;
    return { fake, app };
  }

  it("is dropped by a newer show of the file on screen, which stays", async () => {
    const { fake, app } = await gated(B);
    const opens = vi.spyOn(fake, "openDocument");
    const keeps = vi.spyOn(fake, "keepDocument");
    const first = fake.agentRequest(show(B));
    await settle();
    expect(await fake.agentRequest(show(A))).toEqual({ kind: "shown" });
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
    await settle();
    expect(app.state.doc?.path).toBe(A);
    // Rust had taken B's open as its document already (the fake does at once): A is opened again
    // in place, so Rust's is the one on screen.
    // The keep is numbered after B's open, so Rust refuses that open however late it runs, and
    // the reopen after the keep.
    expect(opens.mock.calls).toEqual([
      [B, 1],
      [A, 3],
    ]);
    expect(keeps.mock.calls).toEqual([[A, 2]]);
  });

  it("is dropped by a newer show that asks first, once Show on the bar sent it", async () => {
    const { fake, app } = await gated(B, "ask");
    expect(await fake.agentRequest(show(B))).toEqual({ kind: "asked" });
    barButton("Show").click();
    await settle();
    expect(await fake.agentRequest(show(C))).toEqual({ kind: "asked" });
    fake.openGate.resolve();
    await vi.waitFor(() => {
      expect(fake.agentAnswers).toEqual([{ id: 1, answer: "replaced" }]);
    });
    await settle();
    expect(app.state.doc?.path).toBe(A);
    expect(bar()?.textContent).toContain("Claude wants to show c.md");
  });

  it("isn't the reader's own: a newer show leaves the file they open to land", async () => {
    const { fake, app } = await gated(C);
    const reading = app.open(C, { push: true });
    await settle();
    expect(await fake.agentRequest(show(A))).toEqual({ kind: "shown" });
    fake.openGate.resolve();
    await reading;
    expect(app.state.doc?.path).toBe(C);
  });

  it("taken by Rust already, has the file on screen opened again before the newer target lands", async () => {
    const { fake, app } = await gated(B);
    const first = fake.agentRequest(show(B));
    await settle();
    // A changed on disk meanwhile, so opened again it renders afresh.
    fake.setDoc(A, "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye again</h1>");
    const landedOn: (string | undefined)[] = [];
    vi.spyOn(app.view, "scrollToLine").mockImplementation(() => {
      landedOn.push(document.querySelector("#lx-doc h1")?.textContent);
      return true;
    });
    // The fake takes B's open as its document at once, as Rust can before the drop.
    expect(await fake.agentRequest(show(A, { line: 1 }), "Claude", [placed(1, 1)])).toEqual({
      kind: "shown",
    });
    // The target went to the document that stays, not the one opened again over it.
    expect(landedOn).toEqual(["Aye again"]);
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });

  it("taken by Rust already, holds back a later show too until the file on screen is back", async () => {
    const { fake, app } = await gated(B);
    const first = fake.agentRequest(show(B));
    await settle();
    // A changed on disk meanwhile; opening it again waits, as a slow share would.
    fake.setDoc(A, "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye again</h1>");
    const reopening = deferred();
    const open = fake.openDocument.bind(fake);
    vi.spyOn(fake, "openDocument").mockImplementation(async (path, generation) => {
      if (path === A) await reopening.promise;
      return open(path, generation);
    });
    const reading: SavedPosition = { headingId: "aye", offset: 40, line: 1, fraction: 0.5 };
    vi.spyOn(app.view, "captureAnchor").mockReturnValue(reading);
    const moves: string[] = [];
    vi.spyOn(app.view, "restore").mockImplementation(() => {
      moves.push("restored");
    });
    vi.spyOn(app.view, "scrollToLine").mockImplementation((line) => {
      moves.push(`line ${String(line)}`);
      return true;
    });
    // The show that drops B's open starts opening A again; a later one drops nothing itself.
    const second = fake.agentRequest(show(A));
    await settle();
    const third = fake.agentRequest(show(A, { line: 1 }), "Claude", [placed(1, 1)]);
    await settle();
    expect(moves).toEqual([]);
    reopening.resolve();
    expect(await third).toEqual({ kind: "shown" });
    expect(await second).toEqual({ kind: "replaced" });
    // The newest target lands last, over the place the reopened A was restored to.
    expect(moves).toEqual(["restored", "line 1"]);
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });

  it("leaves a change deferred behind the reader's own open for that open to settle, so Rust stays on their file", async () => {
    const { fake, app } = await gated(B);
    const first = fake.agentRequest(show(B));
    await settle();
    // Rust answers the keep slowly, and the reader's open of C is slow too.
    const keeping = deferred();
    const keep = fake.keepDocument.bind(fake);
    vi.spyOn(fake, "keepDocument").mockImplementation(async (path, generation) => {
      await keeping.promise;
      return keep(path, generation);
    });
    const openingC = deferred();
    const open = fake.openDocument.bind(fake);
    const opens = vi.spyOn(fake, "openDocument").mockImplementation(async (path, generation) => {
      if (path === C) await openingC.promise;
      return open(path, generation);
    });
    // The show drops B's open; its recovery waits for Rust. The reader opens C meanwhile, and A
    // changes on disk behind that open.
    const second = fake.agentRequest(show(A));
    await settle();
    const reading = app.open(C, { push: true });
    await settle();
    fake.emit("doc-changed", { path: A });
    keeping.resolve();
    expect(await second).toEqual({
      kind: "failed",
      message: "a.md isn't on screen any more: another file took its place.",
    });
    openingC.resolve();
    await reading;
    await settle();
    expect(app.state.doc?.path).toBe(C);
    // The change waited for C to land, which dropped it: no refresh of A was numbered after C's
    // open, so Rust's document is C too.
    expect(opens.mock.calls.map(([path]) => path)).toEqual([C]);
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });

  it("taken by nobody yet, refreshes the file on screen again when the drop lost a refresh on its way", async () => {
    const { fake } = await gated(B);
    // A changes on disk, and its refresh is slow, as a share can be.
    fake.setDoc(A, "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye again</h1>");
    const refreshing = deferred();
    const open = fake.openDocument.bind(fake);
    const opens = vi.spyOn(fake, "openDocument").mockImplementation(async (path, generation) => {
      if (path === A) await refreshing.promise;
      return open(path, generation);
    });
    fake.emit("doc-changed", { path: A });
    await settle();
    // An agent's open of B starts while that refresh is on its way, and a newer show of A drops
    // it, which loses the refresh; Rust hadn't taken B's open, so A is kept as it is.
    const first = fake.agentRequest(show(B));
    await settle();
    vi.spyOn(fake, "keepDocument").mockResolvedValue(true);
    const second = fake.agentRequest(show(A));
    await settle();
    refreshing.resolve();
    expect(await second).toEqual({ kind: "shown" });
    await settle();
    // Refreshed again after the keep: A shows its new text.
    expect(opens.mock.calls.map(([path]) => path)).toEqual([A, B, A]);
    expect(document.querySelector("#lx-doc h1")?.textContent).toBe("Aye again");
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });

  it("taken by nobody yet, lands its target over a refresh deferred while that open was on its way", async () => {
    const { fake, app } = await gated(B);
    const first = fake.agentRequest(show(B));
    await settle();
    // A changed on disk while B's open was on its way: its refresh waits for that open to land.
    fake.setDoc(A, "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye again</h1>");
    fake.emit("doc-changed", { path: A });
    // Rust hadn't taken B's open, so A is kept; the deferred refresh is slow, as a share can be.
    vi.spyOn(fake, "keepDocument").mockResolvedValue(true);
    const refreshing = deferred();
    const open = fake.openDocument.bind(fake);
    vi.spyOn(fake, "openDocument").mockImplementation(async (path, generation) => {
      if (path === A) await refreshing.promise;
      return open(path, generation);
    });
    const reading: SavedPosition = { headingId: "aye", offset: 40, line: 1, fraction: 0.5 };
    vi.spyOn(app.view, "captureAnchor").mockReturnValue(reading);
    const moves: string[] = [];
    vi.spyOn(app.view, "restore").mockImplementation(() => {
      moves.push("restored");
    });
    vi.spyOn(app.view, "scrollToLine").mockImplementation((line) => {
      moves.push(`line ${String(line)}`);
      return true;
    });
    // The show drops B's open, which lets the deferred refresh go; its target waits for that.
    const second = fake.agentRequest(show(A, { line: 1 }), "Claude", [placed(1, 1)]);
    await settle();
    expect(moves).toEqual([]);
    refreshing.resolve();
    expect(await second).toEqual({ kind: "shown" });
    // The refresh rendered the changed A back where the reader was; the target then landed over
    // it, and holds.
    expect(moves).toEqual(["restored", "line 1"]);
    expect(document.querySelector("#lx-doc h1")?.textContent).toBe("Aye again");
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });

  it("taken by Rust already, goes nowhere once the reader moves on while the file on screen opens again", async () => {
    const { fake, app } = await gated(B);
    const first = fake.agentRequest(show(B));
    await settle();
    // A changed on disk meanwhile; opening it again waits, as a slow share would.
    fake.setDoc(A, "<h1 id='aye' data-sourcepos='1:1-1:5'>Aye again</h1>");
    const reopening = deferred();
    const open = fake.openDocument.bind(fake);
    const opens = vi.spyOn(fake, "openDocument").mockImplementation(async (path, generation) => {
      if (path === A) await reopening.promise;
      return open(path, generation);
    });
    // The show drops B's open and starts opening A again; the reader opens C themselves meanwhile.
    const second = fake.agentRequest(show(A, { line: 1 }), "Claude", [placed(1, 1)]);
    await settle();
    await app.open(C, { push: true });
    expect(app.state.doc?.path).toBe(C);
    reopening.resolve();
    expect(await second).toEqual({
      kind: "failed",
      message: "a.md isn't on screen any more: another file took its place.",
    });
    await settle();
    // The reader stays on C: neither the reopening nor the show put A back.
    expect(app.state.doc?.path).toBe(C);
    expect(opens.mock.calls.filter(([path]) => path === A)).toHaveLength(1);
    fake.openGate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
  });
});

describe("a newer show while the file on screen opens again", () => {
  it("answers once its page's target is reached in the page that stays", async () => {
    const page = reportPage();
    const { host, move } = agentHost("html", REPORT, page);
    const reopened = deferred();
    const order: string[] = [];
    host.dropOpen = async () => {
      await reopened.promise;
      order.push("opened again");
    };
    move.mockImplementation(() => {
      order.push("moved");
      return true;
    });
    const agent = new Agent(host);
    let answered = false;
    const answer = agent
      .answer(call(show(REPORT, { quote: "Results", prefix: "Archive" })))
      .then((reply) => {
        answered = true;
        return reply;
      });
    await settle();
    expect(move).not.toHaveBeenCalled();
    expect(answered).toBe(false);
    reopened.resolve();
    expect(await answer).toEqual({ kind: "shown" });
    expect(order).toEqual(["opened again", "moved"]);
  });

  it("is replaced by a newer one that came meanwhile", async () => {
    const { host, move } = agentHost("html", REPORT, reportPage());
    const reopened = deferred();
    host.dropOpen = () => reopened.promise;
    const agent = new Agent(host);
    const first = agent.answer(call(show(REPORT, { heading: "Archive" })));
    await settle();
    host.dropOpen = () => Promise.resolve();
    expect(await agent.answer({ ...call(show(REPORT, { heading: "Results" })), id: 2 })).toEqual({
      kind: "shown",
    });
    reopened.resolve();
    expect(await first).toEqual({ kind: "replaced" });
    expect(move).toHaveBeenCalledTimes(1);
  });

  it("goes nowhere once the reader moves on meanwhile, as a show on its way does", async () => {
    // A show of another file, which switches at once, waits for the file on screen to be back.
    const { host, move } = agentHost("markdown", PLAN);
    host.opens = () => "switch";
    const open = vi.fn<AgentHost["open"]>(() => Promise.resolve(null));
    host.open = open;
    const recovered = deferred();
    host.recovered = () => recovered.promise;
    let navigation = 1;
    host.navigation = () => navigation;
    const agent = new Agent(host);
    const answer = agent.answer(call(show(REPORT, { heading: "Results" })));
    await settle();
    expect(open).not.toHaveBeenCalled();
    // The reader opens another file themselves meanwhile.
    navigation = 2;
    recovered.resolve();
    expect(await answer).toEqual({
      kind: "failed",
      message: "report.html isn't on screen any more: another file took its place.",
    });
    expect(open).not.toHaveBeenCalled();
    expect(move).not.toHaveBeenCalled();
  });
});

describe("a request that comes before startup's first document", () => {
  it("waits for the launched file to show, and is judged against it: asked about", async () => {
    const fake = new GatedBackend(fixtures(), { initial: A });
    const app = new App(fake, appRoot());
    const starting = app.start();
    const reply = fake.agentRequest(show(B));
    // Time for the request to act before startup has answered, were it not to wait.
    await settle();
    fake.startupGate.resolve();
    await starting;
    expect(await reply).toEqual({ kind: "asked" });
    expect(app.state.doc?.path).toBe(A);
    expect(bar()?.textContent).toContain("Claude wants to show b.md");
  });
});

const WORK: FakeWorkspace = { id: "w1", name: "Work", roots: [ROOT], open: true, theme: null };
const PERSONAL: FakeWorkspace = {
  id: "w2",
  name: "Personal",
  roots: ["C:\\Users\\me\\projects"],
  open: false,
  theme: null,
};

describe("a request asked about as the window leaves", () => {
  it("is dismissed before the page reloads into another workspace", async () => {
    const { fake, app } = await launched(A, { workspaces: [WORK, PERSONAL] });
    const reload = vi.fn();
    app.reloadWindow = reload;
    await fake.agentRequest(show(B));
    const replied = vi.spyOn(fake, "replyToAgent");
    await app.workspaces.open("w2", "here");
    expect(reload).toHaveBeenCalledTimes(1);
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "dismissed" }]);
    expect(replied.mock.invocationCallOrder[0]).toBeLessThan(
      reload.mock.invocationCallOrder[0] ?? 0,
    );
  });

  it("is dismissed before the window closes, once its unsaved comment is let go", async () => {
    const { fake } = await launched();
    await fake.agentRequest(show(B));
    fake.emit("close-requested", null);
    const asked = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".ws-confirm");
      if (!found) throw new Error("no confirm");
      return found;
    });
    // Kept open, the request still asks.
    asked.querySelectorAll<HTMLButtonElement>(".btn")[1]?.click();
    await settle();
    expect(bar()).not.toBeNull();
    expect(fake.agentAnswers).toEqual([]);
    fake.emit("close-requested", null);
    const again = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".ws-confirm");
      if (!found) throw new Error("no confirm");
      return found;
    });
    const replied = vi.spyOn(fake, "replyToAgent");
    const closed = vi.spyOn(fake, "closeWindow");
    again.querySelectorAll<HTMLButtonElement>(".btn")[0]?.click();
    await vi.waitFor(() => {
      expect(closed).toHaveBeenCalledTimes(1);
    });
    expect(fake.agentAnswers).toEqual([{ id: 1, answer: "dismissed" }]);
    expect(replied.mock.invocationCallOrder[0]).toBeLessThan(
      closed.mock.invocationCallOrder[0] ?? 0,
    );
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

  describe("Connect Claude Code", () => {
    let writeText: ReturnType<typeof vi.fn<(text: string) => Promise<void>>>;

    beforeEach(() => {
      writeText = vi.fn(() => Promise.resolve());
      Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    });

    afterEach(() => {
      Reflect.deleteProperty(navigator, "clipboard");
    });

    /** The Agents section of Preferences, opened. */
    async function agentsSection(app: App): Promise<HTMLElement> {
      expect(app.actions.run("preferences")).toBe(true);
      return vi.waitFor(() => {
        const found = [...document.querySelectorAll<HTMLElement>(".prefs-section")].find(
          (s) => s.querySelector("h4")?.textContent === "Agents",
        );
        if (!found) throw new Error("no Agents section");
        return found;
      });
    }

    /** The row of the command for `shell`. */
    function row(section: HTMLElement, shell: "wsl" | "windows"): HTMLElement {
      const found = section.querySelector<HTMLElement>(`.prefs-connect-row[data-shell="${shell}"]`);
      if (!found) throw new Error(`no ${shell} row`);
      return found;
    }

    /** The command shown for `shell`, once it shows. */
    function command(section: HTMLElement, shell: "wsl" | "windows"): string {
      const shown = row(section, shell);
      if (shown.hidden) throw new Error(`the ${shell} command is hidden`);
      return shown.querySelector("code")?.textContent ?? "";
    }

    function copy(section: HTMLElement, shell: "wsl" | "windows"): void {
      row(section, shell).querySelector<HTMLButtonElement>("button")?.click();
    }

    function lastToast(): string {
      return document.querySelector("#lx-toasts .toast:last-child")?.textContent ?? "";
    }

    const WSL_COMMAND = `claude mcp add --scope user lectern -- '/mnt/c/Users/me/AppData/Local/Lectern/lectern.exe' mcp --distro "$WSL_DISTRO_NAME"`;
    const WINDOWS_COMMAND = `claude mcp add --scope user lectern -- "C:\\Users\\me\\AppData\\Local\\Lectern\\lectern.exe" mcp`;

    it("gives the WSL and Windows commands for this exe, each with a Copy button", async () => {
      const { app } = await launched();
      const section = await agentsSection(app);
      const connect = section.querySelector<HTMLElement>(".prefs-connect");
      expect(connect?.getAttribute("aria-label")).toBe("Connect Claude Code");
      expect(await vi.waitFor(() => command(section, "wsl"))).toBe(WSL_COMMAND);
      expect(command(section, "windows")).toBe(WINDOWS_COMMAND);
      expect(row(section, "wsl").querySelector("button")?.getAttribute("aria-label")).toBe(
        "Copy the WSL command",
      );
      expect(row(section, "windows").querySelector("button")?.getAttribute("aria-label")).toBe(
        "Copy the Windows command",
      );
      expect(section.querySelector<HTMLElement>(".prefs-connect-no-wsl")?.hidden).toBe(true);
      expect(section.querySelector<HTMLElement>(".prefs-connect-portable")?.hidden).toBe(true);

      copy(section, "wsl");
      await vi.waitFor(() => {
        expect(lastToast()).toBe("Copied the command");
      });
      expect(writeText).toHaveBeenLastCalledWith(WSL_COMMAND);
      copy(section, "windows");
      await vi.waitFor(() => {
        expect(writeText).toHaveBeenLastCalledWith(WINDOWS_COMMAND);
      });
      // A clipboard that refuses says so.
      writeText.mockRejectedValueOnce(new Error("denied"));
      copy(section, "wsl");
      await vi.waitFor(() => {
        expect(lastToast()).toBe("Couldn't copy to the clipboard");
      });
    });

    it("leaves out the WSL command, saying why, when WSL can't reach this exe", async () => {
      const { fake, app } = await launched();
      fake.exe = { windows: "\\\\nas\\share\\apps\\lectern.exe", wsl: null };
      const section = await agentsSection(app);
      expect(await vi.waitFor(() => command(section, "windows"))).toBe(
        `claude mcp add --scope user lectern -- "\\\\nas\\share\\apps\\lectern.exe" mcp`,
      );
      expect(row(section, "wsl").hidden).toBe(true);
      const why = section.querySelector<HTMLElement>(".prefs-connect-no-wsl");
      expect(why?.hidden).toBe(false);
      expect(why?.textContent).toBe(
        "There's no WSL command: this copy of Lectern isn't on a drive WSL mounts.",
      );
    });

    it("quotes a WSL path with an apostrophe for the shell", async () => {
      const { fake, app } = await launched();
      fake.exe = {
        windows: "C:\\Users\\me\\O'Brien's apps\\lectern.exe",
        wsl: "/mnt/c/Users/me/O'Brien's apps/lectern.exe",
      };
      const section = await agentsSection(app);
      expect(await vi.waitFor(() => command(section, "wsl"))).toBe(
        `claude mcp add --scope user lectern -- '/mnt/c/Users/me/O'\\''Brien'\\''s apps/lectern.exe' mcp --distro "$WSL_DISTRO_NAME"`,
      );
      expect(command(section, "windows")).toBe(
        `claude mcp add --scope user lectern -- "C:\\Users\\me\\O'Brien's apps\\lectern.exe" mcp`,
      );
    });

    const POWERSHELL_HINT =
      "Run this one in PowerShell: the path holds a character Command Prompt would change.";

    // Inside double quotes PowerShell expands `$name` and backticks, and Command Prompt `%NAME%`,
    // so a path holding one is quoted for PowerShell alone, where single quotes keep it as it is.
    it.each([
      {
        holding: "nothing either shell changes",
        exe: "C:\\Tools\\Lectern 2\\lectern.exe",
        shell: "Windows",
        command: `claude mcp add --scope user lectern -- "C:\\Tools\\Lectern 2\\lectern.exe" mcp`,
      },
      {
        holding: "a $",
        exe: "C:\\Tools\\$draft\\lectern.exe",
        shell: "PowerShell",
        command: `claude mcp add --scope user lectern -- 'C:\\Tools\\$draft\\lectern.exe' mcp`,
      },
      {
        holding: "a backtick",
        exe: "C:\\Tools\\`n\\lectern.exe",
        shell: "PowerShell",
        command: "claude mcp add --scope user lectern -- 'C:\\Tools\\`n\\lectern.exe' mcp",
      },
      {
        holding: "a %",
        exe: "C:\\Tools\\%TEMP%\\lectern.exe",
        shell: "PowerShell",
        command: `claude mcp add --scope user lectern -- 'C:\\Tools\\%TEMP%\\lectern.exe' mcp`,
      },
      {
        holding: "a ' and a $",
        exe: "C:\\Tools\\O'Brien's $apps\\lectern.exe",
        shell: "PowerShell",
        command: `claude mcp add --scope user lectern -- 'C:\\Tools\\O''Brien''s $apps\\lectern.exe' mcp`,
      },
    ])(
      "names the Windows command's shell for a path holding $holding",
      async ({ exe, shell, command: expected }) => {
        const { fake, app } = await launched();
        fake.exe = { windows: exe, wsl: null };
        const section = await agentsSection(app);
        expect(await vi.waitFor(() => command(section, "windows"))).toBe(expected);
        const windows = row(section, "windows");
        expect(windows.querySelector(".prefs-connect-shell")?.textContent).toBe(shell);
        expect(windows.querySelector("button")?.getAttribute("aria-label")).toBe(
          `Copy the ${shell} command`,
        );
        const hint = section.querySelector<HTMLElement>(".prefs-connect-powershell");
        expect(hint?.hidden).toBe(shell !== "PowerShell");
        expect(hint?.textContent).toBe(POWERSHELL_HINT);
        // The hint sits under the row it is about.
        expect(windows.nextElementSibling).toBe(hint);
      },
    );

    it("tells a portable copy to stay where it is", async () => {
      const { app } = await launched(A, { portable: true });
      const section = await agentsSection(app);
      await vi.waitFor(() => command(section, "windows"));
      const note = section.querySelector<HTMLElement>(".prefs-connect-portable");
      expect(note?.hidden).toBe(false);
      expect(note?.textContent).toBe(
        "This is a portable copy: keep it where it is, or register again after moving it.",
      );
    });
  });
});

const PDF = "C:\\V\\brief.pdf";
const REPORT = "C:\\V\\report.html";

/**
 * Where core found a target in a note: on lines `start` to `end`, at `text` in its text, `inBlock`
 * into its own block's text when given.
 */
function placed(start: number, end: number, text?: [number, number], inBlock?: number): TextPlace {
  const place: TextPlace = text
    ? { startLine: start, endLine: end, textStart: text[0], textEnd: text[1] }
    : { startLine: start, endLine: end };
  if (inBlock !== undefined) place.blockOffset = inBlock;
  return place;
}

/** The frame's URL, of the page or PDF on screen. */
function frameSrc(): string {
  return document.querySelector<HTMLIFrameElement>("iframe.lx-frame")?.getAttribute("src") ?? "";
}

/** The `?v` of the frame's URL. */
function frameVersion(): string | undefined {
  return /[?&]v=(\d+)/.exec(frameSrc())?.[1];
}

/** The app with A on screen, and a PDF to open; agents' opens switch at once. */
async function withPdf(options: FakeOptions = { initial: A }) {
  const fake = new FakeBackend(
    { ...fixtures({ [PDF]: rendered("brief.pdf", "") }), kinds: { [PDF]: "pdf" } },
    options,
  );
  fake.settingsElsewhere({ agentOpens: "switch" });
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

/**
 * Where the app takes an agent's target whose own text core found: `scrollToPassage`, which
 * measures the layout jsdom hasn't got, so it answers as landed.
 */
function passageLandings(app: App) {
  return vi.spyOn(app.view, "scrollToPassage").mockReturnValue(true);
}

/** The text of the document on screen that the passage `landing` was given finds. */
function passageText(call: [number, Passage] | undefined): string | undefined {
  const doc = document.getElementById("lx-doc");
  return doc ? call?.[1](doc)?.toString() : undefined;
}

describe("an agent's targets", () => {
  it("move within the file on screen to where core found them, and Back returns", async () => {
    const { fake, app } = await launched(B);
    const reading: SavedPosition = { headingId: "bee", offset: 4, line: 1, fraction: 0 };
    vi.spyOn(app.view, "captureAnchor").mockReturnValue(reading);
    const scrolled = passageLandings(app);
    expect(
      await fake.agentRequest(show(B, { quote: "Buzz" }), "Claude", [placed(3, 3, [4, 8], 0)]),
    ).toEqual({ kind: "shown" });
    // To its line, with its own text there, which the layout decides on.
    expect(scrolled).toHaveBeenCalledWith(3, expect.any(Function));
    expect(passageText(scrolled.mock.calls[0])).toBe("Buzz");
    // Back goes to where the reader was before the move.
    const restored = vi.spyOn(app.view, "restore");
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(B);
    expect(restored).toHaveBeenCalledWith(reading);
  });

  it("open another file at where core found them, at once or once the reader says Show", async () => {
    const { fake, app } = await launched(null);
    const scrolled = passageLandings(app);
    const byBlock = vi.spyOn(app.view, "scrollToLine");
    // A heading lands by its block.
    expect(
      await fake.agentRequest(show(B, { heading: "Buzz" }), "Claude", [placed(3, 3, [4, 8], 0)]),
    ).toEqual({ kind: "shown" });
    expect(byBlock).toHaveBeenLastCalledWith(3);
    expect(scrolled).not.toHaveBeenCalled();
    await app.open(A, { push: true });
    expect(
      await fake.agentRequest(show(B, { quote: "Bee" }), "Claude", [placed(1, 1, [0, 3], 0)]),
    ).toEqual({ kind: "asked" });
    barButton("Show").click();
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(B);
    });
    expect(scrolled).toHaveBeenCalledWith(1, expect.any(Function));
    expect(passageText(scrolled.mock.calls[0])).toBe("Bee");
  });

  it("jump to a heading by its block, never indexing the whole document, however long", async () => {
    const { fake, app } = await launched(B);
    const entries = 3000;
    const paragraphs = Array.from({ length: entries }, (_, i) => {
      const line = String(3 + 2 * i);
      return `<p data-sourcepos="${line}:1-${line}:40">Survey entry ${String(i)}: the lanternfish count.</p>`;
    }).join("");
    const last = 3 + 2 * entries;
    fake.setDoc(
      B,
      `<h1 id="bee" data-sourcepos="1:1-1:5">Bee</h1>${paragraphs}` +
        `<h2 id="results" data-sourcepos="${String(last)}:1-${String(last)}:10">Results</h2>`,
    );
    fake.emit("doc-changed", { path: B });
    await vi.waitFor(() => {
      expect(document.getElementById("results")).not.toBeNull();
    });
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    vi.mocked(buildTextIndex).mockClear();
    // As core sends a heading's place: with its text, and where that starts in its block.
    const place = placed(last, last, [130_000, 130_007], 0);
    expect(await fake.agentRequest(show(B, { heading: "Results" }), "Claude", [place])).toEqual({
      kind: "shown",
    });
    expect(scrolled).toHaveBeenCalledWith(last);
    expect(buildTextIndex).not.toHaveBeenCalled();
  });

  it("find a quote's passage in its own block's text alone", async () => {
    const { fake, app } = await launched(B);
    const scrolled = passageLandings(app);
    // "Bee Buzz.": "zz" is 6 in, and 2 into its paragraph.
    await fake.agentRequest(show(B, { quote: "zz" }), "Claude", [placed(3, 3, [6, 8], 2)]);
    vi.mocked(buildTextIndex).mockClear();
    expect(passageText(scrolled.mock.calls[0])).toBe("zz");
    const indexed = vi
      .mocked(buildTextIndex)
      .mock.calls.map(([roots]) => roots.map((root) => (root as Element).tagName));
    expect(indexed).toEqual([["P"]]);
  });

  it("open a PDF at its page, and go to another page of it on screen, each with a new ?v", async () => {
    const { fake } = await withPdf();
    expect(await fake.agentRequest(show(PDF, { page: 3 }))).toEqual({ kind: "shown" });
    expect(frameSrc()).toMatch(/[?&]v=\d+#page=3$/);
    const first = frameVersion();
    expect(await fake.agentRequest(show(PDF, { page: 7 }))).toEqual({ kind: "shown" });
    expect(frameSrc()).toMatch(/[?&]v=\d+#page=7$/);
    expect(frameVersion()).not.toBe(first);
  });
});

describe("a window started for an agent", () => {
  it("goes to the agent's target once its file opens, over where the reader left it", async () => {
    const fake = new FakeBackend(
      fixtures({
        [B]: rendered(
          "Bee",
          "<h1 id='bee' data-sourcepos='1:1-1:5'>Bee</h1><p data-sourcepos='3:1-3:9'>Buzz.</p>",
        ),
      }),
      { initial: B, agentTarget: { target: { quote: "Buzz" }, place: placed(3, 3, [4, 8], 0) } },
    );
    // The reader left it at its top.
    await fake.savePosition(B, { headingId: "bee", offset: 0, line: 1, fraction: 0 });
    const app = new App(fake, appRoot());
    const scrolled = passageLandings(app);
    const restored = vi.spyOn(app.view, "restore");
    await app.start();
    expect(app.state.doc?.path).toBe(B);
    expect(scrolled).toHaveBeenCalledWith(3, expect.any(Function));
    expect(passageText(scrolled.mock.calls[0])).toBe("Buzz");
    expect(restored).not.toHaveBeenCalled();
  });

  it("opens a PDF at the agent's page", async () => {
    await withPdf({ initial: PDF, agentTarget: { target: { page: 3 } } });
    expect(frameSrc()).toMatch(/#page=3$/);
  });
});

/** `highlight` of `targets` in `path`. */
function highlight(targets: Target[], path = B, clear = false): AgentRequest {
  return { tool: "highlight", path, targets, clear };
}

/** Stands in for the CSS Custom Highlight API, which jsdom lacks. */
class FakeHighlight extends Set<Range> {
  priority = 0;
  constructor(...ranges: Range[]) {
    super(ranges);
  }
}

function stubHighlights(): void {
  vi.stubGlobal("Highlight", FakeHighlight);
  vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
}

/** The agent's layer, as painted. */
function agentLayer(): FakeHighlight | undefined {
  return CSS.highlights.get("agent") as unknown as FakeHighlight | undefined;
}

/** The text the agent's layer paints, range by range. */
function agentPainted(): string[] {
  return [...(agentLayer() ?? [])].map((r) => r.toString());
}

describe("an agent's highlights", () => {
  it("paint passages of a note on a layer of their own, cleared by a switch to another file", async () => {
    stubHighlights();
    const { fake, app } = await launched(B);
    const reply = await fake.agentRequest(
      highlight([{ heading: "Bee" }, { quote: "Buzz" }]),
      "Claude",
      [placed(1, 1, [0, 3]), placed(3, 3, [4, 8])],
    );
    expect(reply).toEqual({ kind: "highlighted", count: 2 });
    expect(agentPainted()).toEqual(["Bee", "Buzz"]);
    // Over the comments', under find's.
    expect(agentLayer()?.priority).toBe(2);
    await app.open(C, { push: true });
    expect(CSS.highlights.has("agent")).toBe(false);
    // Back on the file, they don't come back.
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(B);
    expect(CSS.highlights.has("agent")).toBe(false);
  });

  it("are twenty at most: the oldest goes first", async () => {
    stubHighlights();
    const { fake } = await launched(B);
    await fake.agentRequest(highlight([{ quote: "Buzz" }]), "Claude", [placed(3, 3, [4, 8])]);
    // Twenty other passages of "Bee Buzz.", each its own: within "Bee", and within "Buzz.".
    const spans: [number, number][] = [];
    for (const [from, to] of [
      [0, 3],
      [4, 9],
    ] as const) {
      for (let s = from; s < to; s++) {
        for (let e = s + 1; e <= to; e++) if (!(s === 4 && e === 8)) spans.push([s, e]);
      }
    }
    const mark = ([s, e]: [number, number]) =>
      fake.agentRequest(highlight([{ quote: "Bee Buzz.".slice(s, e) }]), "Claude", [
        placed(1, 3, [s, e]),
      ]);
    for (const span of spans.slice(0, 19)) await mark(span);
    expect(agentPainted()).toHaveLength(20);
    expect(agentPainted()[0]).toBe("Buzz");
    // The 21st.
    expect(await mark(spans[19] ?? [0, 1])).toEqual({ kind: "highlighted", count: 20 });
    expect(agentPainted()).toEqual(spans.map(([s, e]) => "Bee Buzz.".slice(s, e)));
  });

  it("paint a passage known only by its lines over their blocks, and clear on request", async () => {
    stubHighlights();
    const { fake } = await launched(B);
    await fake.agentRequest(highlight([{ quote: "**Buzz.**" }]), "Claude", [placed(3, 3)]);
    expect(agentPainted()).toEqual(["Buzz."]);
    expect(await fake.agentRequest(highlight([], B, true))).toEqual({
      kind: "highlighted",
      count: 0,
    });
    expect(CSS.highlights.has("agent")).toBe(false);
    // Cleared and painted again at once.
    expect(
      await fake.agentRequest(highlight([{ heading: "Bee" }], B, true), "Claude", [
        placed(1, 1, [0, 3]),
      ]),
    ).toEqual({ kind: "highlighted", count: 1 });
    expect(agentPainted()).toEqual(["Bee"]);
  });

  it("paint a list item known only by its lines whole, not just a link in it", async () => {
    stubHighlights();
    const list =
      '<ul data-sourcepos="1:1-2:9"><li data-sourcepos="1:1-1:30">' +
      '<a data-sourcepos="1:3-1:31" href="#">Offline sync</a>: the endpoint</li>' +
      '<li data-sourcepos="2:1-2:9">Seedbed</li></ul>';
    const { fake } = await launched(B);
    fake.setDoc(B, list);
    fake.emit("doc-changed", { path: B });
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-doc li")).not.toBeNull();
    });
    await fake.agentRequest(highlight([{ quote: "[[sync|Offline sync]]: the" }]), "Claude", [
      placed(1, 1),
    ]);
    expect(agentPainted()).toEqual(["Offline sync: the endpoint"]);
  });

  it("count a passage already painted once, however often it is asked for", async () => {
    stubHighlights();
    const { fake } = await launched(B);
    const buzz = placed(3, 3, [4, 8]);
    expect(
      await fake.agentRequest(highlight([{ quote: "Buzz" }, { quote: "Buzz" }]), "Claude", [
        buzz,
        buzz,
      ]),
    ).toEqual({ kind: "highlighted", count: 1 });
    expect(await fake.agentRequest(highlight([{ quote: "Buzz" }]), "Claude", [buzz])).toEqual({
      kind: "highlighted",
      count: 1,
    });
    expect(agentPainted()).toEqual(["Buzz"]);
    // Another passage is another highlight.
    expect(
      await fake.agentRequest(highlight([{ heading: "Bee" }]), "Claude", [placed(1, 1, [0, 3])]),
    ).toEqual({ kind: "highlighted", count: 2 });
  });

  it("are for the file on screen only", async () => {
    const { fake } = await launched(B);
    expect(await fake.agentRequest(highlight([{ line: 1 }], A), "Claude", [placed(1, 1)])).toEqual({
      kind: "failed",
      message: "a.md isn't shown in Lectern. Show it first.",
    });
  });
});

/** An HTML page's text, as its bridge reported it: Weekly report › Results, twice. */
function reportPage(): PageText {
  const blocks = [
    { text: "Weekly report", headingLevel: 1 },
    { text: "Results", headingLevel: 2 },
    { text: "Rendered by script" },
    { text: "Archive", headingLevel: 2 },
    { text: "Results", headingLevel: 3 },
    { text: "Rendered again" },
  ];
  const starts: number[] = [];
  let at = 0;
  for (const block of blocks) {
    starts.push(at);
    at += block.text.length + 1;
  }
  const headings = blocks.flatMap((b, i) =>
    b.headingLevel === undefined
      ? []
      : [{ level: b.headingLevel, text: b.text, id: "", offset: starts[i] ?? 0 }],
  );
  return {
    text: blocks.map((b) => b.text).join(" "),
    blocks,
    starts,
    headings,
    ready: true,
    truncated: false,
    paint: vi.fn(),
    clear: vi.fn(),
    headingPath: () => [],
    selection: null,
  };
}

describe("a target in an HTML page", () => {
  it("is a quote found in the page's text, told apart by the text around it", () => {
    const page = reportPage();
    // "Weekly report Results Rendered by script Archive Results Rendered again"
    expect(pageSpan(page, { quote: "Results" })).toEqual({ start: 14, end: 21 });
    expect(pageSpan(page, { quote: "Results", prefix: "Archive " })).toEqual({
      start: 49,
      end: 56,
    });
    expect(pageSpan(page, { quote: "Results", suffix: " Rendered again" })).toEqual({
      start: 49,
      end: 56,
    });
    expect(pageSpan(page, { quote: "Rendered  by\nscript" })).toEqual({ start: 22, end: 40 });
    expect(pageSpan(page, { quote: "rolled back" })).toBeNull();
  });

  it("is a heading by its text or its path, the nearest by depth, in any letter case", () => {
    const page = reportPage();
    expect(pageSpan(page, { heading: "results" })).toEqual({ start: 14, end: 21 });
    expect(pageSpan(page, { heading: "Archive › Results" })).toEqual({ start: 49, end: 56 });
    expect(pageSpan(page, { heading: "Weekly report › Archive › results" })).toEqual({
      start: 49,
      end: 56,
    });
    expect(pageSpan(page, { heading: "Archive › Weekly report" })).toBeNull();
  });
});

/**
 * A page as its bridge reported it: Weekly report › Rollout plan (its own id `plan`), then Results
 * twice. The others have the ids the bridge makes up.
 */
function anchoredPage(): PageText {
  const blocks = [
    { text: "Weekly report", headingLevel: 1 },
    { text: "Rollout plan", headingLevel: 2 },
    { text: "Steps." },
    { text: "Results", headingLevel: 2 },
    { text: "Rows." },
    { text: "Results", headingLevel: 3 },
    { text: "More rows." },
  ];
  const ids = ["lx-heading-1", "plan", "", "lx-heading-2", "", "lx-heading-3", ""];
  const page = reportPage();
  const starts: number[] = [];
  let at = 0;
  for (const block of blocks) {
    starts.push(at);
    at += block.text.length + 1;
  }
  return Object.assign(page, {
    text: blocks.map((b) => b.text).join(" "),
    blocks,
    starts,
    headings: blocks.flatMap((b, i) =>
      b.headingLevel === undefined
        ? []
        : [{ level: b.headingLevel, text: b.text, id: ids[i] ?? "", offset: starts[i] ?? 0 }],
    ),
  });
}

describe("an HTML page's #anchor", () => {
  // "Weekly report Rollout plan Steps. Results Rows. Results More rows."
  const rollout = { start: 14, end: 26 };

  it("is the heading with that id, or whose text's slug it is, as Lectern's links find one", () => {
    const page = anchoredPage();
    expect(pageSpan(page, { anchor: "rollout-plan" })).toEqual(rollout);
    expect(pageSpan(page, { anchor: "plan" })).toEqual(rollout);
    expect(pageSpan(page, { anchor: "Rollout%20plan" })).toEqual(rollout);
    expect(pageSpan(page, { anchor: "Rollout plan" })).toEqual(rollout);
    // Two of a kind: the second's slug is numbered, as a note's heading ids are.
    expect(pageSpan(page, { anchor: "results" })).toEqual({ start: 34, end: 41 });
    expect(pageSpan(page, { anchor: "results-1" })).toEqual({ start: 48, end: 55 });
    expect(pageSpan(page, { anchor: "lx-heading-3" })).toEqual({ start: 48, end: 55 });
    expect(pageSpan(page, { anchor: "risks" })).toBeNull();
  });

  it("is shown in the page on screen, or is the error a missing heading is", async () => {
    const page = anchoredPage();
    const { host, move } = agentHost("html", REPORT, page);
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, { anchor: "rollout-plan" })))).toEqual({
      kind: "shown",
    });
    const landed = move.mock.calls[0]?.[0];
    expect(landed && "aim" in landed ? landed.aim(page) : null).toBe(14);
    const missing = { kind: "failed", message: "That passage isn't in report.html." };
    expect(await agent.answer(call(show(REPORT, { anchor: "risks" })))).toEqual(missing);
    expect(await agent.answer(call(show(REPORT, { heading: "Risks" })))).toEqual(missing);
  });

  it("is named on the bar that asks about it, as written", async () => {
    const { host } = opening(anchoredPage(), true, "ask");
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, { anchor: "rollout-plan" })))).toEqual({
      kind: "asked",
    });
    expect(host.bar.textContent).toContain("Claude wants to show report.html › rollout-plan");
  });
});

/** A page's session whose text is `text`, one block. */
function onePage(text: string): Pick<PageText, "text" | "blocks" | "starts" | "headings"> {
  return { text, blocks: [{ text }], starts: [0], headings: [] };
}

describe("a quote in an HTML page, told apart by long context", () => {
  it("is found quickly on a long repetitive page, its context compared 256 characters at most", () => {
    const page = onePage("a".repeat(200_000));
    const started = performance.now();
    const span = pageSpan(page, { quote: "a", prefix: "a".repeat(2000) });
    const took = performance.now() - started;
    expect(span).not.toBeNull();
    expect(took).toBeLessThan(100);
  });

  it("is still told apart by 256 characters of context, before or after it", () => {
    const same = "x".repeat(255);
    // "Q" twice, the text before each differing only 256 characters back, and after each only
    // 256 characters on.
    const page = onePage(`A${same}Q${same}B C${same}Q${same}D`);
    const second = 1 + 255 + 1 + 255 + 3 + 255;
    expect(pageSpan(page, { quote: "Q", prefix: `C${same}` })).toEqual({
      start: second,
      end: second + 1,
    });
    expect(pageSpan(page, { quote: "Q", suffix: `${same}D` })).toEqual({
      start: second,
      end: second + 1,
    });
    // Without context, the first.
    expect(pageSpan(page, { quote: "Q" })).toEqual({ start: 256, end: 257 });
  });
});

/**
 * A window showing `doc` (with `page`, its session, for an HTML page), as `Agent` sees it. Its
 * comments save an operation as `review` does.
 */
function agentHost(
  kind: DocKind,
  path: string,
  page: PageText | null = null,
  review: AgentHost["review"] = () => Promise.reject(new Error("no comments here")),
) {
  const doc: DocPayload = { ...docPayload(null), path, kind };
  const move = vi.fn<AgentHost["move"]>(() => true);
  const host: AgentHost = {
    doc: () => doc,
    docElement: () => document.body,
    scroller: () => document.body,
    page: () => page,
    shownPath: () => path,
    opens: () => "ask",
    open: () => Promise.resolve(null),
    move,
    pageReady: () => Promise.resolve(page?.ready === true ? page : null),
    navigation: () => 1,
    places: () => Promise.resolve([]),
    aimPage: () => undefined,
    answered: () => Promise.resolve(),
    review,
    forgetThreads: () => undefined,
    dropOpen: () => Promise.resolve(),
    recovered: () => Promise.resolve(),
    bar: document.createElement("div"),
  };
  return { host, move };
}

function call(request: AgentRequest) {
  return { id: 1, request, agent: "Claude" };
}

describe("an agent in an HTML page or a PDF", () => {
  it("highlights a page's passages through its bridge, all of them or none", async () => {
    const page = reportPage();
    const agent = new Agent(agentHost("html", REPORT, page).host);
    expect(
      await agent.answer(call(highlight([{ quote: "Results", prefix: "Archive" }], REPORT))),
    ).toEqual({ kind: "highlighted", count: 1 });
    expect(page.paint).toHaveBeenLastCalledWith("agent", [{ start: 49, end: 56 }]);
    vi.mocked(page.paint).mockClear();
    expect(
      await agent.answer(
        call(highlight([{ heading: "Results" }, { quote: "rolled back" }], REPORT)),
      ),
    ).toEqual({ kind: "failed", message: "That passage isn't in report.html." });
    expect(page.paint).not.toHaveBeenCalled();
    expect(await agent.answer(call(highlight([], REPORT, true)))).toEqual({
      kind: "highlighted",
      count: 0,
    });
    expect(page.clear).toHaveBeenCalledWith("agent");
  });

  it("counts a page's passage already painted once", async () => {
    const page = reportPage();
    const agent = new Agent(agentHost("html", REPORT, page).host);
    const twice = highlight([{ heading: "Archive" }, { quote: "Archive" }], REPORT);
    expect(await agent.answer(call(twice))).toEqual({ kind: "highlighted", count: 1 });
    expect(await agent.answer(call(twice))).toEqual({ kind: "highlighted", count: 1 });
    expect(page.paint).toHaveBeenLastCalledWith("agent", [{ start: 41, end: 48 }]);
  });

  it("gives a page's text as its bridge reported it, once it has", async () => {
    const page = reportPage();
    const agent = new Agent(agentHost("html", REPORT, page).host);
    expect(await agent.answer(call({ tool: "read", path: REPORT }))).toEqual({
      kind: "pageText",
      blocks: page.blocks,
    });
    // A page whose text stopped at its limit says so.
    const cut = new Agent(agentHost("html", REPORT, { ...page, truncated: true }).host);
    expect(await cut.answer(call({ tool: "read", path: REPORT }))).toEqual({
      kind: "pageText",
      blocks: page.blocks,
      truncated: true,
    });
    const loading = new Agent(agentHost("html", REPORT, { ...page, ready: false }).host);
    const notYet = {
      kind: "failed",
      message: "The page hasn't loaded yet; try again in a moment.",
    };
    expect(await loading.answer(call({ tool: "read", path: REPORT }))).toEqual(notYet);
    expect(await loading.answer(call(highlight([{ quote: "Results" }], REPORT)))).toEqual(notYet);
  });

  it("refuses a highlight in a PDF, which can't be painted, going nowhere", async () => {
    const { host, move } = agentHost("pdf", PDF);
    const agent = new Agent(host);
    for (const request of [highlight([{ line: 4 }], PDF), highlight([], PDF, true)]) {
      expect(await agent.answer(call(request))).toEqual({
        kind: "failed",
        message: "PDFs can't be highlighted; use show with a page target.",
      });
    }
    expect(move).not.toHaveBeenCalled();
  });
});

/**
 * A window with A on screen that opens the report page (its session `page`) when asked, at once
 * or (`opens: "ask"`) once the reader says Show: the page has loaded when `ready`, else not within
 * the time it is waited for, which `waited` records. `loading` holds the page's load until it is
 * resolved, `leave` has the reader open another file, and `landings` are where each open went.
 */
function opening(page: PageText, ready: boolean, opens: AgentOpens = "switch") {
  let shown: DocPayload = { ...docPayload(null), path: A, kind: "markdown" };
  let navigation = 1;
  const waited: number[] = [];
  const landings: (Landing | null)[] = [];
  const loading = { gate: null as Deferred | null };
  const move = vi.fn<AgentHost["move"]>(() => true);
  const aimPage = vi.fn<AgentHost["aimPage"]>();
  const answered = vi.fn<AgentHost["answered"]>(() => Promise.resolve());
  const host: AgentHost = {
    doc: () => shown,
    docElement: () => document.body,
    scroller: () => document.body,
    page: () => (shown.kind === "html" ? page : null),
    shownPath: () => shown.path,
    opens: () => opens,
    open: (path, landing) => {
      navigation += 1;
      landings.push(landing);
      shown = { ...docPayload(null), path, kind: "html" };
      return Promise.resolve(null);
    },
    move,
    pageReady: async (within) => {
      waited.push(within);
      await loading.gate?.promise;
      return ready ? page : null;
    },
    navigation: () => navigation,
    places: () => Promise.resolve([]),
    aimPage,
    answered,
    review: () => Promise.reject(new Error("no comments here")),
    forgetThreads: () => undefined,
    dropOpen: () => Promise.resolve(),
    recovered: () => Promise.resolve(),
    bar: document.createElement("div"),
  };
  const leave = (): void => {
    navigation += 1;
    shown = { ...docPayload(null), path: C, kind: "markdown" };
  };
  /** The page loads again, as on a change on disk: a new showing of it, the same navigation. */
  const reload = (): void => {
    shown = { ...shown };
  };
  /** The reader opens `path` themselves. */
  const visit = (path: string, kind: DocKind = "html"): void => {
    navigation += 1;
    shown = { ...docPayload(null), path, kind };
  };
  /** Opens fail from now on, as for a file that's gone. */
  const fail = (): void => {
    host.open = () => Promise.resolve("It was moved or deleted.");
  };
  return { host, move, waited, landings, loading, leave, reload, visit, fail, answered, aimPage };
}

/** Where `landing` goes in `page`, for one that finds a passage there once it has loaded. */
function aimed(landing: Landing | null | undefined, page: PageText): number | null | undefined {
  return landing && "aim" in landing ? landing.aim(page) : undefined;
}

describe("an agent's show of an HTML page", () => {
  it("waits for a page it opens to load, as long as Rust waits, then answers if the passage is there", async () => {
    const { host, waited } = opening(reportPage(), true);
    const agent = new Agent(host);
    const shown = {
      ...call(show(REPORT, { quote: "Results", prefix: "Archive" })),
      withinMs: 4000,
    };
    expect(await agent.answer(shown)).toEqual({ kind: "shown" });
    // Less a margin for the answer to get back in time.
    expect(waited[0]).toBeGreaterThan(3000);
    expect(waited[0]).toBeLessThanOrEqual(3500);
    const missing = { kind: "failed", message: "That passage isn't in report.html." };
    const { host: other } = opening(reportPage(), true);
    expect(await new Agent(other).answer(call(show(REPORT, { quote: "rolled back" })))).toEqual(
      missing,
    );
    // On screen already, the same.
    expect(await agent.answer(call(show(REPORT, { heading: "Nowhere" })))).toEqual(missing);
  });

  it("answers a passage the page hadn't loaded in time for as pending, and goes there once it does", async () => {
    const { host, move } = opening(reportPage(), false);
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, { quote: "Results" })))).toEqual({
      kind: "shown",
      target: "pending",
    });
    // On screen already, still loading: it moves there once it has.
    expect(await agent.answer(call(show(REPORT, { heading: "Archive" })))).toEqual({
      kind: "shown",
      target: "pending",
    });
    expect(move).toHaveBeenCalledTimes(1);
  });
});

describe("a page's target, stale before the page loaded", () => {
  it("is dropped when a newer request takes its place: no scroll, answered replaced", async () => {
    const page = reportPage();
    const { host, landings, loading } = opening(page, true);
    loading.gate = deferred();
    const agent = new Agent(host);
    const first = agent.answer(call(show(REPORT, { quote: "Results", prefix: "Archive" })));
    await settle();
    // A newer show while the page loads, of the page itself, without a target.
    expect(await agent.answer({ ...call(show(REPORT)), id: 2 })).toEqual({ kind: "shown" });
    loading.gate.resolve();
    expect(await first).toEqual({ kind: "replaced" });
    expect(aimed(landings[0], page)).toBeNull();
  });

  it("is dropped when the reader opens another file meanwhile: no scroll, and not shown", async () => {
    const page = reportPage();
    const { host, landings, loading, leave } = opening(page, true);
    loading.gate = deferred();
    const agent = new Agent(host);
    const answer = agent.answer(call(show(REPORT, { quote: "Results" })));
    await settle();
    leave();
    loading.gate.resolve();
    expect(await answer).toEqual({
      kind: "failed",
      message: "report.html isn't on screen any more: another file took its place.",
    });
    expect(aimed(landings[0], page)).toBeNull();
  });
});

describe("a page's target whose request ended unreached", () => {
  /** The bar's button `name` in `host`'s bar. */
  function click(host: AgentHost, name: string): void {
    [...host.bar.querySelectorAll("button")].find((b) => b.textContent === name)?.click();
  }

  it("goes nowhere once the reader says Not now, nor when they open the page later", async () => {
    const { host, visit, aimPage, answered } = opening(reportPage(), true, "ask");
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, { quote: "Results" })))).toEqual({
      kind: "asked",
    });
    click(host, "Not now");
    await vi.waitFor(() => {
      expect(answered).toHaveBeenCalledWith(1, "dismissed", undefined);
    });
    visit(REPORT);
    agent.docShown();
    expect(aimPage).not.toHaveBeenCalled();
  });

  it("goes nowhere once a newer request takes its place", async () => {
    const { host, visit, aimPage } = opening(reportPage(), true, "ask");
    const agent = new Agent(host);
    await agent.answer(call(show(REPORT, { quote: "Results" })));
    expect(await agent.answer({ ...call(show(C)), id: 2 })).toEqual({ kind: "asked" });
    visit(REPORT);
    agent.docShown();
    expect(aimPage).not.toHaveBeenCalled();
  });

  it("goes nowhere when Show on the bar fails to open the page", async () => {
    const { host, visit, fail, aimPage, answered } = opening(reportPage(), true, "ask");
    const agent = new Agent(host);
    await agent.answer(call(show(REPORT, { quote: "Results" })));
    fail();
    click(host, "Show");
    await vi.waitFor(() => {
      expect(answered).toHaveBeenCalledWith(
        1,
        "failed",
        "report.html didn't open: It was moved or deleted.",
      );
    });
    visit(REPORT);
    agent.docShown();
    expect(aimPage).not.toHaveBeenCalled();
  });

  it("goes nowhere when an immediate open of the page fails", async () => {
    const { host, visit, fail, aimPage } = opening(reportPage(), true, "switch");
    const agent = new Agent(host);
    fail();
    expect(await agent.answer(call(show(REPORT, { quote: "Results" })))).toEqual({
      kind: "failed",
      message: "report.html didn't open: It was moved or deleted.",
    });
    visit(REPORT);
    agent.docShown();
    expect(aimPage).not.toHaveBeenCalled();
  });
});

describe("a page's target the reader said Show to", () => {
  /** Asks about the report at `target`, then says Show: what the window tells Rust. */
  async function confirmed(target: Target, ready = true) {
    const page = reportPage();
    const { host, answered, landings, waited } = opening(page, ready, "ask");
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, target)))).toEqual({ kind: "asked" });
    const button = [...host.bar.querySelectorAll("button")].find((b) => b.textContent === "Show");
    button?.click();
    await vi.waitFor(() => {
      expect(answered).toHaveBeenCalled();
    });
    return { answered, landing: landings[0], page, waited };
  }

  it("fails a passage that isn't there, as an immediate show does", async () => {
    const { answered } = await confirmed({ quote: "rolled back" });
    expect(answered).toHaveBeenCalledWith(1, "failed", "That passage isn't in report.html.");
  });

  it("is shown once the page has loaded with it, and goes there", async () => {
    const { answered, landing, page, waited } = await confirmed({ heading: "Archive › Results" });
    expect(answered).toHaveBeenCalledWith(1, "shown", undefined);
    expect(aimed(landing, page)).toBe(49);
    // Waited for within the time an answer is given.
    expect(waited[0]).toBeGreaterThan(0);
    expect(waited[0]).toBeLessThanOrEqual(4500);
  });
});

describe("an agent's highlights after the text changes", () => {
  it("in a note, are found again by core, painted only once they are, and dropped when gone", async () => {
    stubHighlights();
    const { fake, app } = await launched(B);
    await fake.agentRequest(highlight([{ quote: "Buzz" }, { heading: "Bee" }]), "Claude", [
      placed(3, 3, [4, 8]),
      placed(1, 1, [0, 3]),
    ]);
    expect(agentPainted()).toEqual(["Buzz", "Bee"]);
    const finding = deferred();
    const places = vi.spyOn(fake, "agentPlaces").mockImplementation(async () => {
      await finding.promise;
      return [placed(3, 3, [7, 11]), null];
    });
    // B changes on disk: Buzz moves, Bee is gone.
    fake.setDoc(
      B,
      "<h1 id='b' data-sourcepos='1:1-1:5'>Be</h1><p data-sourcepos='3:1-3:20'>Now Buzz moved.</p>",
    );
    fake.emit("doc-changed", { path: B });
    await vi.waitFor(() => {
      expect(app.state.doc?.html).toContain("Now Buzz moved.");
    });
    // Never the old places on the new text.
    expect(CSS.highlights.has("agent")).toBe(false);
    expect(places).toHaveBeenCalledWith(B, [{ quote: "Buzz" }, { heading: "Bee" }]);
    finding.resolve();
    await vi.waitFor(() => {
      expect(agentPainted()).toEqual(["Buzz"]);
    });
  });

  it("in an HTML page, are found again in its new text, and dropped when gone", async () => {
    const page = reportPage();
    const { host } = agentHost("html", REPORT, page);
    const agent = new Agent(host);
    await agent.answer(
      call(highlight([{ quote: "Results", prefix: "Archive" }, { heading: "Archive" }], REPORT)),
    );
    expect(page.paint).toHaveBeenLastCalledWith("agent", [
      { start: 49, end: 56 },
      { start: 41, end: 48 },
    ]);
    // The page's script rewrites it: the Archive heading goes, and Results moves.
    const blocks = [
      { text: "Weekly report", headingLevel: 1 },
      { text: "Summary", headingLevel: 2 },
      { text: "Archive notes Results" },
    ];
    Object.assign(page, {
      text: "Weekly report Summary Archive notes Results",
      blocks,
      starts: [0, 14, 22],
      headings: [
        { level: 1, text: "Weekly report", id: "", offset: 0 },
        { level: 2, text: "Summary", id: "", offset: 14 },
      ],
    });
    agent.pageChanged();
    expect(page.paint).toHaveBeenLastCalledWith("agent", [{ start: 36, end: 43 }]);
    // Nothing left of them: cleared.
    Object.assign(page, { text: "Weekly report", blocks: [blocks[0]], starts: [0] });
    agent.pageChanged();
    expect(page.clear).toHaveBeenLastCalledWith("agent");
  });
});

describe("a page's target through a reload", () => {
  it("is aimed again at the page that loads in its place, then answered as reached", async () => {
    const page = reportPage();
    const { host, loading, reload, aimPage } = opening(page, true);
    loading.gate = deferred();
    const agent = new Agent(host);
    const answer = agent.answer(call(show(REPORT, { quote: "Results", prefix: "Archive" })));
    await settle();
    // The page changes on disk while it loads: a new frame takes the old one's place.
    reload();
    agent.docShown();
    expect(aimPage).toHaveBeenCalledTimes(1);
    const aim = aimPage.mock.calls[0]?.[0];
    expect(aim?.(page)).toBe(49);
    loading.gate.resolve();
    expect(await answer).toEqual({ kind: "shown" });
    // Reached: another reload aims nothing again.
    reload();
    agent.docShown();
    expect(aimPage).toHaveBeenCalledTimes(1);
  });

  it("stays pending through a reload when it wasn't reached in time", async () => {
    const page = reportPage();
    const { host, reload, aimPage } = opening(page, false);
    const agent = new Agent(host);
    expect(await agent.answer(call(show(REPORT, { quote: "Results" })))).toEqual({
      kind: "shown",
      target: "pending",
    });
    reload();
    agent.docShown();
    expect(aimPage.mock.calls[0]?.[0](page)).toBe(14);
  });
});

describe("an HTML page's heading by its whole text", () => {
  it("is found in the page's blocks, beyond what its outline lists", () => {
    const long = `Results for ${"the lanternfish survey, week by week, ".repeat(12)}`.trim();
    const page = reportPage();
    const blocks = [
      { text: "Weekly report", headingLevel: 1 },
      { text: long, headingLevel: 2 },
      { text: "Rows." },
    ];
    Object.assign(page, {
      text: blocks.map((b) => b.text).join(" "),
      blocks,
      starts: [0, 14, 15 + long.length],
      // The outline's list, cut at 300 characters a heading, as the bridge cuts it.
      headings: [
        { level: 1, text: "Weekly report", id: "", offset: 0 },
        { level: 2, text: long.slice(0, 300), id: "", offset: 14 },
      ],
    });
    expect(long.length).toBeGreaterThan(300);
    expect(pageSpan(page, { heading: long })).toEqual({ start: 14, end: 14 + long.length });
    expect(pageSpan(page, { heading: `Weekly report › ${long}` })).toEqual({
      start: 14,
      end: 14 + long.length,
    });
  });
});

describe("a target a window started with", () => {
  it("goes nowhere once a newer show comes before its page loads", async () => {
    const fake = new FakeBackend(
      { ...fixtures({ [REPORT]: rendered("report.html", "") }), kinds: { [REPORT]: "html" } },
      { initial: REPORT, agentTarget: { target: { quote: "Results", prefix: "Archive" } } },
    );
    const app = new App(fake, appRoot());
    const aimed = vi.spyOn(app.view, "aimPage");
    await app.start();
    const aim = aimed.mock.calls[0]?.[0];
    expect(aim).toBeDefined();
    const page = reportPage() as unknown as Parameters<NonNullable<typeof aim>>[0];
    expect(aim?.(page)).toBe(49);
    // A newer show, of a note the reader is asked about.
    expect(await fake.agentRequest(show(A))).toEqual({ kind: "asked" });
    expect(aim?.(page)).toBeNull();
  });
});

/** What the window's comments answer an operation with: the review as saved, `changed` its thread. */
function saved(changed: number, status: CommentStatus): ReviewPayload {
  return {
    notePath: REPORT,
    sidecarPath: `${REPORT}.review.md`,
    noteWslPath: null,
    sidecarWslPath: null,
    exists: true,
    readOnly: null,
    comments: [
      {
        id: changed,
        status,
        state: "anchored",
        startLine: 1,
        endLine: 1,
        headingPath: [],
        jumpLine: 1,
        pinnedHeading: null,
        pinnedAt: null,
        quote: "Results",
        textStart: null,
        textEnd: null,
        currentText: null,
        entries: [],
      },
    ],
    unreadable: [],
    openCount: 1,
    changed,
  };
}

describe("an agent's review threads in an HTML page", () => {
  it("start with the agent's comment on a passage found in the page's text", async () => {
    const review = vi.fn<AgentHost["review"]>(() => Promise.resolve(saved(7, "question")));
    const agent = new Agent(agentHost("html", REPORT, reportPage(), review).host);
    const request: AgentRequest = {
      tool: "comment",
      path: REPORT.toUpperCase(),
      target: { quote: "Results", prefix: "Archive" },
      text: "Why twice?",
    };
    expect(await agent.answer(call(request))).toEqual({
      kind: "commented",
      id: 7,
      status: "question",
    });
    // On the page's own path, the agent its author: a selection's anchor carries no lines, and
    // where the window found it, so core anchors it on that "Results" of the two.
    expect(review).toHaveBeenCalledWith(REPORT, {
      op: "add",
      anchor: {
        startLine: 0,
        endLine: 0,
        quote: "Results",
        prefix: "ults Rendered by script Archive ",
        textStart: 49,
      },
      text: "Why twice?",
      author: { name: "Claude", kind: "question" },
    });
    // A heading, as the agent asks for it.
    await agent.answer({
      id: 2,
      agent: "Codex",
      request: {
        tool: "comment",
        path: REPORT,
        target: { heading: "Archive" },
        text: "Too old.",
        kind: "pushback",
      },
    });
    expect(review).toHaveBeenLastCalledWith(REPORT, {
      op: "add",
      anchor: {
        startLine: 0,
        endLine: 0,
        quote: "Archive",
        prefix: "port Results Rendered by script ",
        textStart: 41,
      },
      text: "Too old.",
      author: { name: "Codex", kind: "pushback" },
    });
  });

  it("refuse a passage that isn't there, a page still loading, and a file not on screen, saving nothing", async () => {
    const review = vi.fn<AgentHost["review"]>(() => Promise.resolve(saved(1, "question")));
    const comment = (path: string, quote: string): AgentRequest => ({
      tool: "comment",
      path,
      target: { quote },
      text: "Why?",
    });
    const agent = new Agent(agentHost("html", REPORT, reportPage(), review).host);
    expect(await agent.answer(call(comment(REPORT, "rolled back")))).toEqual({
      kind: "failed",
      message: "That passage isn't in report.html.",
    });
    expect(await agent.answer(call(comment(A, "Results")))).toEqual({
      kind: "failed",
      message: "a.md isn't shown in Lectern. Show it first.",
    });
    const loading = new Agent(
      agentHost("html", REPORT, { ...reportPage(), ready: false }, review).host,
    );
    expect(await loading.answer(call(comment(REPORT, "Results")))).toEqual({
      kind: "failed",
      message: "The page hasn't loaded yet; try again in a moment.",
    });
    // A note's comments are Rust's to write.
    const note = new Agent(agentHost("markdown", A, null, review).host);
    expect(await note.answer(call(comment(A, "Aye")))).toEqual({
      kind: "failed",
      message: "Only an HTML page's comments go through its window.",
    });
    expect(review).not.toHaveBeenCalled();
    // Refused by the store: the agent hears why.
    const refused = new Agent(
      agentHost("html", REPORT, reportPage(), () =>
        Promise.reject(new Error("Couldn't save the comment: the file kept changing.")),
      ).host,
    );
    expect(await refused.answer(call(comment(REPORT, "Results")))).toEqual({
      kind: "failed",
      message: "Couldn't save the comment: the file kept changing.",
    });
  });

  it("reply and resolve with the agent's entries, of the kind asked for", async () => {
    const review = vi.fn<AgentHost["review"]>(() => Promise.resolve(saved(3, "pushback")));
    const agent = new Agent(agentHost("html", REPORT, reportPage(), review).host);
    expect(
      await agent.answer({
        id: 1,
        agent: "Codex",
        request: { tool: "reply", path: REPORT, id: "C3", text: "Not yet.", kind: "pushback" },
      }),
    ).toEqual({ kind: "commented", id: 3, status: "pushback" });
    expect(review).toHaveBeenLastCalledWith(REPORT, {
      op: "reply",
      id: 3,
      text: "Not yet.",
      author: { name: "Codex", kind: "pushback" },
    });
    await agent.answer(call({ tool: "reply", path: REPORT, id: "C3", text: "Weekly." }));
    expect(review).toHaveBeenLastCalledWith(REPORT, {
      op: "reply",
      id: 3,
      text: "Weekly.",
      author: { name: "Claude", kind: "reply" },
    });
    await agent.answer(call({ tool: "resolve", path: REPORT, id: "C3" }));
    expect(review).toHaveBeenLastCalledWith(REPORT, {
      op: "reply",
      id: 3,
      text: "Resolved.",
      author: { name: "Claude", kind: "resolved" },
    });
    await agent.answer(call({ tool: "resolve", path: REPORT, id: "C3", text: "Fixed." }));
    expect(review).toHaveBeenLastCalledWith(REPORT, {
      op: "reply",
      id: 3,
      text: "Fixed.",
      author: { name: "Claude", kind: "resolved" },
    });
  });
});

describe("an agent's show at a comment thread", () => {
  it("goes to the thread on screen, or opens its file at it", async () => {
    const { host, move } = agentHost("markdown", A);
    const opened = vi.spyOn(host, "open");
    const agent = new Agent(host);
    expect(await agent.answer(call(show(A, { comment: "C12" })))).toEqual({ kind: "shown" });
    expect(move).toHaveBeenCalledWith({ comment: 12 });
    const blank = agentHost("markdown", A);
    blank.host.shownPath = () => null;
    blank.host.doc = () => null;
    const opens = vi.spyOn(blank.host, "open");
    expect(await new Agent(blank.host).answer(call(show(B, { comment: "C2" })))).toEqual({
      kind: "shown",
    });
    expect(opens).toHaveBeenCalledWith(B, { comment: 2 });
    expect(opened).not.toHaveBeenCalled();
  });

  it("opens the Comments tab on the thread and goes to its passage, through the app", async () => {
    const { fake, app } = await launched(B);
    fake.agentComment(B, {
      startLine: 3,
      endLine: 3,
      quote: "Buzz.",
      headingPath: ["Bee"],
      name: "Claude",
      kind: "question",
      text: "Why buzz?",
    });
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    await vi.waitFor(() => {
      expect(
        document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
      ).toBe(true);
    });
    expect(document.getElementById("lx-tab-comments")?.getAttribute("aria-selected")).toBe("true");
    expect(scrolled).toHaveBeenCalledWith(3);
  });
});

describe("an agent's show at a thread whose review is still loading", () => {
  it("is dropped when a newer show goes elsewhere in the file meanwhile", async () => {
    const { fake, app } = await launched(B);
    await vi.waitFor(() => {
      expect(document.querySelector<HTMLElement>("#lx-comments-pane .comments-head")?.hidden).toBe(
        false,
      );
    });
    fake.agentComment(B, {
      startLine: 1,
      endLine: 1,
      quote: "Bee",
      headingPath: ["Bee"],
      name: "Claude",
      kind: "question",
      text: "Why bee?",
    });
    // The review the thread's show loads again is held until the newer show has landed.
    const held = deferred();
    const load = fake.loadReview.bind(fake);
    vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path, page) => {
      await held.promise;
      return load(path, page);
    });
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    const passages = passageLandings(app);
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    expect(
      await fake.agentRequest(show(B, { quote: "Buzz" }), "Claude", [placed(3, 3, [4, 8], 0)]),
    ).toEqual({ kind: "shown" });
    expect(passages).toHaveBeenLastCalledWith(3, expect.any(Function));
    held.resolve();
    await vi.waitFor(() => {
      expect(document.querySelector('.comment-card[data-id="1"]')).not.toBeNull();
    });
    await settle();
    // The thread's passage on line 1 never took the reader back there.
    expect(scrolled).not.toHaveBeenCalled();
    expect(passages).toHaveBeenCalledTimes(1);
    expect(
      document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
    ).toBe(false);
  });
});

/**
 * B on screen with its comments loaded, then a thread C1 on "Bee" (line 1) written to its sidecar,
 * whose review loads again only once `held` is resolved.
 */
async function threadStillLoading() {
  const { fake, app } = await launched(B);
  await vi.waitFor(() => {
    expect(document.querySelector<HTMLElement>("#lx-comments-pane .comments-head")?.hidden).toBe(
      false,
    );
  });
  fake.agentComment(B, {
    startLine: 1,
    endLine: 1,
    quote: "Bee",
    headingPath: ["Bee"],
    name: "Claude",
    kind: "question",
    text: "Why bee?",
  });
  const held = deferred();
  const load = fake.loadReview.bind(fake);
  vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path, page) => {
    await held.promise;
    return load(path, page);
  });
  return { fake, app, held };
}

describe("a thread still loading for an agent's show", () => {
  it("is dropped when a newer show asks the reader first", async () => {
    const { fake, app, held } = await threadStillLoading();
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    expect(await fake.agentRequest(show(C))).toEqual({ kind: "asked" });
    held.resolve();
    await vi.waitFor(() => {
      expect(document.querySelector('.comment-card[data-id="1"]')).not.toBeNull();
    });
    await settle();
    expect(scrolled).not.toHaveBeenCalled();
    expect(document.getElementById("lx-tab-comments")?.getAttribute("aria-selected")).toBe("false");
  });

  it("is dropped when a newer show of the file on screen has no target", async () => {
    const { fake, app, held } = await threadStillLoading();
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    expect(await fake.agentRequest(show(B))).toEqual({ kind: "shown" });
    held.resolve();
    await vi.waitFor(() => {
      expect(document.querySelector('.comment-card[data-id="1"]')).not.toBeNull();
    });
    await settle();
    expect(scrolled).not.toHaveBeenCalled();
    expect(
      document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
    ).toBe(false);
  });

  it("is dropped when the reader opens the same file at a line meanwhile", async () => {
    const { fake, app, held } = await threadStillLoading();
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    // The reader's own navigation within the file: a link to line 3, quick open, Back.
    await app.open(B, { line: 3 });
    expect(scrolled).toHaveBeenLastCalledWith(3);
    held.resolve();
    await vi.waitFor(() => {
      expect(document.querySelector('.comment-card[data-id="1"]')).not.toBeNull();
    });
    await settle();
    expect(scrolled.mock.calls).toEqual([[3]]);
    expect(
      document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
    ).toBe(false);
  });
});

describe("an agent's open at a thread, overtaken by the reader", () => {
  it("drops the reveal when the reader opens the same file before the agent's open lands", async () => {
    const { fake, app } = await launched(A);
    fake.agentComment(B, {
      startLine: 1,
      endLine: 1,
      quote: "Bee",
      headingPath: ["Bee"],
      name: "Claude",
      kind: "question",
      text: "Why bee?",
    });
    // The agent's open switches at once, so it goes through `openForAgent`.
    fake.settingsElsewhere({ agentOpens: "switch" });
    // Hold the agent's own open of B, so the reader can overtake it before it lands.
    const held = deferred();
    const open = fake.openDocument.bind(fake);
    vi.spyOn(fake, "openDocument").mockImplementationOnce(async (path, generation) => {
      await held.promise;
      return open(path, generation);
    });
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    const request = fake.agentRequest(show(B, { comment: "C1" }));
    await settle();
    // The reader opens the same file at line 3: a newer navigation that lands first.
    await app.open(B, { line: 3 });
    expect(scrolled).toHaveBeenLastCalledWith(3);
    held.resolve();
    expect(await request).toEqual({ kind: "shown" });
    await settle();
    // The agent's open was superseded; its reveal carries that open's navigation, not the
    // reader's, so it never jumps back to C1.
    expect(scrolled.mock.calls).toEqual([[3]]);
    expect(
      document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
    ).toBe(false);
  });
});

describe("two overlapping agent opens of the same file at threads", () => {
  it("shows the newer thread once the comments load, the older open landing late", async () => {
    const { fake, app } = await launched(A);
    await vi.waitFor(() => {
      expect(document.querySelector<HTMLElement>("#lx-comments-pane .comments-head")?.hidden).toBe(
        false,
      );
    });
    for (const [line, quote, text] of [
      [1, "Bee", "Why bee?"],
      [3, "Buzz.", "Why buzz?"],
    ] as const) {
      fake.agentComment(B, {
        startLine: line,
        endLine: line,
        quote,
        headingPath: ["Bee"],
        name: "Claude",
        kind: "question",
        text,
      });
    }
    fake.settingsElsewhere({ agentOpens: "switch" });
    // The older show's open of B is held; the newer one's goes through and lands first.
    const heldOpen = deferred();
    const open = fake.openDocument.bind(fake);
    vi.spyOn(fake, "openDocument").mockImplementationOnce(async (path, generation) => {
      await heldOpen.promise;
      return open(path, generation);
    });
    // B's review is held, so the newer thread waits for the comments.
    const heldReview = deferred();
    const load = fake.loadReview.bind(fake);
    vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path, page) => {
      await heldReview.promise;
      return load(path, page);
    });
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    const older = fake.agentRequest(show(B, { comment: "C1" }));
    await settle();
    expect(await fake.agentRequest(show(B, { comment: "C2" }))).toEqual({ kind: "shown" });
    // The older open completes, superseded: its stale reveal must not displace the newer one.
    heldOpen.resolve();
    expect(await older).toEqual({ kind: "shown" });
    await settle();
    heldReview.resolve();
    await vi.waitFor(() => {
      expect(
        document.querySelector('.comment-card[data-id="2"]')?.classList.contains("selected"),
      ).toBe(true);
    });
    expect(scrolled).toHaveBeenCalledWith(3);
    expect(
      document.querySelector('.comment-card[data-id="1"]')?.classList.contains("selected"),
    ).toBe(false);
  });
});

describe("an agent's show at a thread in focus mode", () => {
  it("goes to its passage alone, and changes none of the reader's settings", async () => {
    const { fake, app } = await launched(B);
    await vi.waitFor(() => {
      expect(document.querySelector<HTMLElement>("#lx-comments-pane .comments-head")?.hidden).toBe(
        false,
      );
    });
    fake.agentComment(B, {
      startLine: 3,
      endLine: 3,
      quote: "Buzz.",
      headingPath: ["Bee"],
      name: "Claude",
      kind: "question",
      text: "Why buzz?",
    });
    expect(app.state.settings.outlineVisible).toBe(true);
    app.actions.run("focus");
    expect(app.actions.inFocusMode).toBe(true);
    const scrolled = vi.spyOn(app.view, "scrollToLine");
    expect(await fake.agentRequest(show(B, { comment: "C1" }))).toEqual({ kind: "shown" });
    await vi.waitFor(() => {
      expect(scrolled).toHaveBeenCalledWith(3);
    });
    await settle();
    expect(app.state.settings.outlineVisible).toBe(true);
    expect(document.getElementById("lx-tab-comments")?.getAttribute("aria-selected")).toBe("false");
    app.actions.run("focus");
    expect(app.actions.inFocusMode).toBe(false);
    expect(app.state.settings.outlineVisible).toBe(true);
  });
});
