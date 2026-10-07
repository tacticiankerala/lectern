import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { CommentsController, type CommentsHost } from "../src/comments";
import type { CommentView } from "../src/generated/CommentView";
import type { EntryView } from "../src/generated/EntryView";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import { RightPanel } from "../src/right-panel";
import { A, B, deferred, fixtures, settle } from "./helpers";

const HTML = `<h1 id="tide-sync" data-sourcepos="1:1-1:11">Tide sync</h1>
<p data-sourcepos="3:1-3:60">The client sends at most <em data-sourcepos="3:26-3:32">fifty</em> changes per batch to the hub.</p>
<ul data-sourcepos="5:1-6:20">
<li data-sourcepos="5:1-5:30">Retry with a short backoff</li>
<li data-sourcepos="6:1-6:20">Log every failure</li>
</ul>`;

function you(text: string): EntryView {
  return { author: "you", name: "You", kind: null, text, html: `<p>${text}</p>` };
}

function comment(over: Partial<CommentView> & { id: number }): CommentView {
  return {
    status: "open",
    state: "anchored",
    startLine: 1,
    endLine: 1,
    headingPath: ["Tide sync"],
    jumpLine: 1,
    pinnedHeading: null,
    quote: "",
    textStart: null,
    textEnd: null,
    currentText: null,
    entries: [],
    ...over,
  };
}

function review(path: string): ReviewPayload {
  return {
    notePath: path,
    sidecarPath: path.replace(/\.md$/, ".review.md"),
    noteWslPath: null,
    sidecarWslPath: null,
    exists: true,
    readOnly: null,
    comments: [
      comment({
        id: 1,
        startLine: 3,
        endLine: 3,
        jumpLine: 3,
        quote: "sends at most fifty changes",
        entries: [you("Why fifty?")],
      }),
      comment({
        id: 2,
        status: "question",
        startLine: 5,
        endLine: 5,
        jumpLine: 5,
        quote: "Retry with a short backoff",
        entries: [
          you("How short?"),
          {
            author: "agent",
            name: "Claude",
            kind: "question",
            text: "Seconds?",
            html: "<p>Seconds?</p>",
          },
        ],
      }),
      comment({
        id: 3,
        status: "resolved",
        startLine: 6,
        endLine: 6,
        jumpLine: 6,
        quote: "Log every failure",
        entries: [you("Too noisy?")],
      }),
      comment({
        id: 4,
        state: "detached",
        startLine: 9,
        endLine: 9,
        jumpLine: null,
        quote: "an old paragraph",
        entries: [you("Still true?")],
      }),
    ],
    unreadable: [],
    openCount: 3,
  };
}

function setup(
  opts: { visible?: boolean; panelOpen?: boolean; focusMode?: boolean; html?: string } = {},
) {
  const html = opts.html ?? HTML;
  document.body.innerHTML = `<main id="pane"><article class="doc">${html}</article></main><aside id="side"></aside>`;
  const pane = document.getElementById("pane");
  const aside = document.getElementById("side");
  const docEl = document.querySelector<HTMLElement>(".doc");
  if (!pane || !aside || !docEl) throw new Error("no layout");
  const fake = new FakeBackend(fixtures());
  fake.setReview(A, review(A));
  const panel = new RightPanel(aside);
  const state = { path: A as string | null, visible: opts.visible ?? true };
  const docListeners = new Set<() => void>();
  const host = {
    backend: fake,
    doc: () => docEl,
    docPath: () => state.path,
    panel,
    scroller: pane,
    toast: vi.fn<(m: string) => void>(),
    setBadge: vi.fn<(n: number) => void>(),
    jumpToLine: vi.fn<(line: number) => void>(),
    follow: vi.fn<(a: HTMLAnchorElement) => void>(),
    onDoc: (cb: () => void) => {
      docListeners.add(cb);
      return () => {
        docListeners.delete(cb);
      };
    },
    visible: () => state.visible,
    panelOpen: () => opts.panelOpen ?? true,
    openPanel: vi.fn<() => void>(),
    pulseBadge: vi.fn<() => void>(),
    focusMode: () => opts.focusMode ?? false,
    showComments: vi.fn<() => void>(() => {
      state.visible = true;
    }),
  } satisfies CommentsHost;
  const controller = new CommentsController(host);
  controllers.push(controller);
  const rerender = () => {
    docEl.innerHTML = html;
    for (const cb of [...docListeners]) cb();
  };
  return { fake, panel, host, state, controller, docEl, rerender };
}

function cards(panel: RightPanel): HTMLElement[] {
  return [...panel.commentsPane.querySelectorAll<HTMLElement>("article.comment-card")];
}

function card(panel: RightPanel, id: number): HTMLElement {
  const el = panel.commentsPane.querySelector<HTMLElement>(
    `article.comment-card[data-id="${String(id)}"]`,
  );
  if (!el) throw new Error(`no card C${String(id)}`);
  return el;
}

function action(panel: RightPanel, id: number, name: string): HTMLButtonElement {
  const el = card(panel, id).querySelector<HTMLButtonElement>(`[data-action="${name}"]`);
  if (!el) throw new Error(`no ${name} on C${String(id)}`);
  return el;
}

function typeReply(panel: RightPanel, id: number, text: string): HTMLTextAreaElement {
  const area = card(panel, id).querySelector<HTMLTextAreaElement>("textarea");
  if (!area) throw new Error(`no reply box on C${String(id)}`);
  area.value = text;
  area.dispatchEvent(new Event("input", { bubbles: true }));
  return area;
}

function ctrlEnter(el: HTMLElement): void {
  el.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true }),
  );
}

/** Selects from the start of `from` to the end of `to`, each the first text in `root` holding it. */
function selectBetween(root: Element, from: string, to: string): Range {
  const textWith = (word: string): Text => {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (n instanceof Text && n.data.includes(word)) return n;
    }
    throw new Error(`no text with ${word}`);
  };
  const start = textWith(from);
  const end = textWith(to);
  const range = document.createRange();
  range.setStart(start, start.data.indexOf(from));
  range.setEnd(end, end.data.indexOf(to) + to.length);
  const selection = document.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  return range;
}

/** Lets `n` animation frames go by. */
async function frames(n: number): Promise<void> {
  for (let i = 0; i < n; i++) {
    await new Promise((resolve) => requestAnimationFrame(resolve));
  }
}

/** The selection changed: lets the controller's frame run. */
async function selectionSettles(): Promise<void> {
  document.dispatchEvent(new Event("selectionchange"));
  await new Promise((resolve) => requestAnimationFrame(resolve));
}

/** A client rect `height` tall from `top`, full width. */
function box(top: number, height: number): DOMRect {
  return {
    x: 0,
    y: top,
    top,
    bottom: top + height,
    left: 0,
    right: 800,
    width: 800,
    height,
    toJSON: () => ({}),
  };
}

function editorText(): HTMLTextAreaElement {
  const el = document.querySelector<HTMLTextAreaElement>(".comment-editor textarea");
  if (!el || el.closest<HTMLElement>(".comment-editor")?.hidden) throw new Error("no editor open");
  return el;
}

/** Stands in for the CSS Custom Highlight API, which jsdom lacks. */
class FakeHighlight extends Set<Range> {
  priority = 0;
  constructor(...ranges: Range[]) {
    super(ranges);
  }
}

/** Every controller made, disposed after each test so none keeps listening to the document. */
const controllers: CommentsController[] = [];

afterEach(() => {
  for (const c of controllers.splice(0)) c.dispose();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("CommentsController", () => {
  it("renders cards and badge", async () => {
    const { panel, host, controller, docEl } = setup();
    await controller.load();
    // Open ones: the detached first, then by line; C3 is resolved.
    expect(cards(panel).map((c) => c.dataset.id)).toEqual(["4", "1", "2"]);
    expect(host.setBadge).toHaveBeenLastCalledWith(3);
    expect(
      panel.commentsPane.ownerDocument.querySelector("[role=tab]:last-child")?.textContent,
    ).toBe("Comments (3)");
    const detached = card(panel, 4);
    expect(detached.dataset.state).toBe("detached");
    expect(detached.closest(".comment-group")?.querySelector("h3")?.textContent).toBe("Detached");
    expect(detached.querySelector(".comment-quote")?.textContent).toBe("an old paragraph");
    expect(detached.textContent).toContain("Detached");

    const first = card(panel, 1);
    expect(first.dataset.status).toBe("open");
    expect(first.querySelector("button.comment-place")?.textContent).toBe("L3 · Tide sync");
    expect(first.querySelector("blockquote.comment-quote")?.textContent).toBe(
      "sends at most fifty changes",
    );
    expect(first.querySelector(".comment-entry.you .comment-author")?.textContent).toBe("You");
    expect(first.querySelector(".comment-entry.you .comment-body")?.innerHTML).toBe(
      "<p>Why fifty?</p>",
    );
    const second = card(panel, 2);
    expect(second.dataset.status).toBe("question");
    expect(second.querySelector(".comment-status")?.textContent).toBe("question");
    expect(second.querySelector(".comment-entry.agent .comment-author")?.textContent).toBe(
      "Claude question",
    );
    // The outline's selectors never meet a card.
    expect(
      panel.commentsPane.querySelectorAll(".comment-card > li, .comment-card > a"),
    ).toHaveLength(0);
    // A dot on each block with an open comment in the note.
    const dots = [...docEl.querySelectorAll<HTMLElement>(".lx-cdot")];
    expect(dots.map((d) => d.dataset.comments)).toEqual(["1", "2"]);
    expect(dots.map((d) => d.getAttribute("aria-label"))).toEqual(["Comment C1", "Comment C2"]);
    expect(dots[0]?.parentElement?.classList.contains("lx-has-comment")).toBe(true);

    // All: the resolved one too, with Reopen.
    panel.commentsPane.querySelector<HTMLButtonElement>('[data-filter="all"]')?.click();
    expect(cards(panel).map((c) => c.dataset.id)).toEqual(["4", "1", "2", "3"]);
    expect(action(panel, 3, "reopen").textContent).toBe("Reopen");
    // Re-attach is for moved and detached comments only.
    expect(card(panel, 3).querySelector('[data-action="reattach"]')).toBeNull();
    expect(action(panel, 4, "reattach").disabled).toBe(false);
  });

  it("reply sends a reply op and re-renders", async () => {
    const { fake, panel, controller } = setup();
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    action(panel, 1, "reply").click();
    const area = typeReply(panel, 1, "  Make it a setting. ");
    expect(document.activeElement).toBe(area);
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelectorAll(".comment-entry.you")).toHaveLength(2);
    });
    expect(reviewOp).toHaveBeenCalledWith(A, { op: "reply", id: 1, text: "Make it a setting." });
    expect(card(panel, 1).querySelector("textarea")).toBeNull();

    // An empty reply isn't sent.
    action(panel, 2, "reply").click();
    const empty = typeReply(panel, 2, "   ");
    expect(card(panel, 2).querySelector<HTMLButtonElement>('[data-action="send"]')?.disabled).toBe(
      true,
    );
    ctrlEnter(empty);
    await settle();
    expect(reviewOp).toHaveBeenCalledTimes(1);
    // Esc closes it.
    empty.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(card(panel, 2).querySelector("textarea")).toBeNull();
  });

  it("resolve and dismiss", async () => {
    const { fake, panel, host, controller } = setup();
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    action(panel, 1, "resolve").click();
    await vi.waitFor(() => {
      expect(cards(panel).map((c) => c.dataset.id)).toEqual(["4", "2"]);
    });
    expect(reviewOp).toHaveBeenCalledWith(A, { op: "setStatus", id: 1, change: "resolve" });
    action(panel, 2, "dismiss").click();
    await vi.waitFor(() => {
      expect(cards(panel).map((c) => c.dataset.id)).toEqual(["4"]);
    });
    expect(host.setBadge).toHaveBeenLastCalledWith(1);
    panel.commentsPane.querySelector<HTMLButtonElement>('[data-filter="all"]')?.click();
    expect(card(panel, 1).dataset.status).toBe("resolved");
    expect(card(panel, 2).dataset.status).toBe("dismissed");
    expect(card(panel, 2).querySelector('[data-action="dismiss"]')).toBeNull();
    action(panel, 1, "reopen").click();
    await vi.waitFor(() => {
      expect(card(panel, 1).dataset.status).toBe("open");
    });
  });

  it("stale review responses are dropped", async () => {
    const { fake, panel, host, state, controller } = setup();
    fake.setReview(B, { ...review(B), comments: [], openCount: 0 });
    const gate = deferred();
    const real = fake.loadReview.bind(fake);
    const loadReview = vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path) => {
      const payload = await real(path);
      await gate.promise;
      return payload;
    });
    const forA = controller.load();
    // A's answer is on its way when B comes on screen.
    await vi.waitFor(() => {
      expect(loadReview).toHaveBeenCalledTimes(1);
    });
    state.path = B;
    const forB = controller.load();
    gate.resolve();
    await forA;
    await forB;
    expect(loadReview).toHaveBeenLastCalledWith(B);
    expect(cards(panel)).toHaveLength(0);
    expect(host.setBadge).not.toHaveBeenCalledWith(3);

    // Nor does one that lands after the note changed, with no newer load.
    state.path = A;
    const gate2 = deferred();
    loadReview.mockImplementationOnce(async (path) => {
      const payload = await real(path);
      await gate2.promise;
      return payload;
    });
    const again = controller.load();
    await vi.waitFor(() => {
      expect(loadReview).toHaveBeenCalledTimes(3);
    });
    state.path = B;
    gate2.resolve();
    await again;
    expect(cards(panel)).toHaveLength(0);
  });

  it("review-changed for another note is ignored", async () => {
    const { fake, panel, host, controller } = setup({ panelOpen: false });
    await controller.load();
    const load = vi.spyOn(fake, "loadReview");
    fake.emit("review-changed", { path: B });
    await settle();
    expect(load).not.toHaveBeenCalled();

    fake.agentReply(A, 1, "Claude", "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelectorAll(".comment-entry.agent")).toHaveLength(1);
    });
    expect(card(panel, 1).dataset.status).toBe("replied");
    expect(panel.commentsPane.ownerDocument.querySelector("[role=tab].flash")).not.toBeNull();
    // The panel is closed, so the header badge pulses too.
    expect(host.pulseBadge).toHaveBeenCalledTimes(1);
  });

  it("names each agent, and an agent-started thread goes on with your reply", async () => {
    const { fake, panel, controller } = setup();
    await controller.load();
    // An agent appends a comment of its own to the sidecar.
    fake.agentComment(A, {
      startLine: 3,
      endLine: 3,
      quote: "to the hub",
      headingPath: ["Tide sync"],
      name: "Codex",
      kind: "question",
      text: "Which hub, the station's or the server's?",
    });
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(cards(panel).map((c) => c.dataset.id)).toEqual(["4", "1", "5", "2"]);
    });
    const started = card(panel, 5);
    expect(started.dataset.status).toBe("question");
    expect(started.querySelector(".comment-quote")?.textContent).toBe("to the hub");
    const first = started.querySelector(".comment-entry");
    expect(first?.className).toBe("comment-entry agent");
    expect(first?.querySelector(".comment-author")?.textContent).toBe("Codex question");
    // Reply is there; Edit is only ever on your own entries.
    expect(action(panel, 5, "reply").disabled).toBe(false);
    expect(started.querySelector('[data-action="edit"]')).toBeNull();

    action(panel, 5, "reply").click();
    ctrlEnter(typeReply(panel, 5, "The station's."));
    await vi.waitFor(() => {
      expect(card(panel, 5).querySelectorAll(".comment-entry")).toHaveLength(2);
    });
    expect(card(panel, 5).dataset.status).toBe("open");
    const authors = () =>
      [...card(panel, 5).querySelectorAll(".comment-author")].map((a) => a.textContent);
    expect(authors()).toEqual(["Codex question", "You"]);
    const edits = card(panel, 5).querySelectorAll<HTMLElement>('[data-action="edit"]');
    expect([...edits].map((e) => e.dataset.entry)).toEqual(["1"]);

    // Another agent answers: its own name, and its kind sets the status.
    fake.agentReply(A, 5, "Gemini", "pushback", "The server's: stations relay.");
    fake.agentReply(A, 2, "Claude", null, "A few seconds.");
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(card(panel, 5).dataset.status).toBe("pushback");
    });
    expect(authors()).toEqual(["Codex question", "You", "Gemini pushback"]);
    expect(
      card(panel, 2).querySelector(".comment-entry:last-child .comment-author")?.textContent,
    ).toBe("Claude");
    expect(card(panel, 2).dataset.status).toBe("replied");
  });

  it("hidden removes highlights and dots", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    const { state, controller, docEl, rerender } = setup();
    await controller.load();
    expect(CSS.highlights.has("comment")).toBe(true);
    const painted = [...(CSS.highlights.get("comment") as unknown as FakeHighlight)];
    expect(painted.map((r) => r.toString())).toEqual([
      "sends at most fifty changes",
      "Retry with a short backoff",
    ]);
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(2);

    state.visible = false;
    controller.setVisible(false);
    expect(CSS.highlights.has("comment")).toBe(false);
    expect(CSS.highlights.has("comment-focus")).toBe(false);
    expect(docEl.querySelectorAll(".lx-cdot, .lx-has-comment")).toHaveLength(0);
    // A re-render while hidden adds none.
    rerender();
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(0);

    state.visible = true;
    controller.setVisible(true);
    expect(CSS.highlights.has("comment")).toBe(true);
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(2);
    // A re-render wipes them; they come back with it.
    rerender();
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(2);

    controller.dispose();
    expect(CSS.highlights.has("comment")).toBe(false);
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(0);
  });

  it("marks a comment on a later footnote", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    // Two footnotes, as core renders them: the section carries only the first one's lines.
    const { fake, controller, docEl } = setup({
      html: `<p data-sourcepos="3:1-3:56">The sync runs nightly.<sup data-sourcepos="3:23-3:26" class="footnote-ref"><a href="#fn-1" id="fnref-1" data-footnote-ref="">1</a></sup> It batches fifty changes.<sup data-sourcepos="3:53-3:56" class="footnote-ref"><a href="#fn-2" id="fnref-2" data-footnote-ref="">2</a></sup></p>
<section data-sourcepos="5:1-5:31" class="footnotes" data-footnotes="">
<ol>
<li data-sourcepos="5:1-5:31" id="fn-1">
<p data-sourcepos="5:7-5:31">Since the spring release. <a href="#fnref-1" class="footnote-backref" data-footnote-backref="" data-footnote-backref-idx="1" aria-label="Back to reference 1">↩</a></p>
</li>
<li data-sourcepos="7:1-7:60" id="fn-2">
<p data-sourcepos="7:7-7:60">The batch size came from the load test on the old hub. <a href="#fnref-2" class="footnote-backref" data-footnote-backref="" data-footnote-backref-idx="2" aria-label="Back to reference 2">↩</a></p>
</li>
</ol>
</section>`,
    });
    fake.setReview(A, {
      ...review(A),
      comments: [
        comment({
          id: 1,
          startLine: 7,
          endLine: 7,
          jumpLine: 7,
          quote: "load test on the old hub",
          entries: [you("Which hub?")],
        }),
      ],
      openCount: 1,
    });
    await controller.load();
    const painted = [...(CSS.highlights.get("comment") as unknown as FakeHighlight)];
    expect(painted.map((r) => r.toString())).toEqual(["load test on the old hub"]);
    const dot = docEl.querySelector<HTMLElement>(".lx-cdot");
    expect(dot?.dataset.comments).toBe("1");
    expect(dot?.parentElement?.getAttribute("data-sourcepos")).toBe("7:7-7:60");
  });

  it("a dot opens the Comments tab and focuses its card", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    const { panel, host, controller, docEl } = setup({ panelOpen: false });
    await controller.load();
    expect(panel.tab).toBe("outline");
    docEl.querySelector<HTMLElement>('.lx-cdot[data-comments="2"]')?.click();
    expect(panel.tab).toBe("comments");
    expect(host.openPanel).toHaveBeenCalledTimes(1);
    expect(card(panel, 2).classList.contains("selected")).toBe(true);
    const focus = CSS.highlights.get("comment-focus") as unknown as FakeHighlight;
    expect([...focus].map((r) => r.toString())).toEqual(["Retry with a short backoff"]);
    expect(focus.priority).toBe(1);
  });

  it("a read-only review shows why, changes nothing and lists what it couldn't read", async () => {
    const { fake, panel, controller } = setup();
    fake.setReview(A, {
      ...review(A),
      readOnly: "This review belongs to another note, so it's read-only.",
      unreadable: [{ raw: "## C9 · open · Lx\nnot a comment" }],
    });
    await controller.load();
    const banner = panel.commentsPane.querySelector<HTMLElement>(".comments-banner");
    expect(banner?.hidden).toBe(false);
    expect(banner?.textContent).toBe("This review belongs to another note, so it's read-only.");
    for (const name of ["reply", "resolve", "dismiss"]) {
      expect(action(panel, 1, name).disabled, name).toBe(true);
    }
    expect(action(panel, 4, "reattach").disabled).toBe(true);
    expect(action(panel, 1, "copy").disabled).toBe(false);
    const unreadable = panel.commentsPane.querySelector(".comment-card.unreadable");
    expect(unreadable?.textContent).toContain("Couldn't read this comment");
    expect(unreadable?.querySelector("pre")?.textContent).toBe("## C9 · open · Lx\nnot a comment");
    expect(unreadable?.querySelector("button")).toBeNull();
  });

  it("follows links in comments, and jumps to a comment's text", async () => {
    const { fake, panel, host, controller } = setup();
    const p = review(A);
    p.comments[0]?.entries.push({
      author: "agent",
      name: "Claude",
      kind: null,
      text: "See [the notes](notes.md).",
      html: '<p>See <a href="#" data-kind="doc" data-target="C:\\V\\notes.md">the notes</a>.</p>',
    });
    fake.setReview(A, p);
    await controller.load();
    card(panel, 1).querySelector<HTMLAnchorElement>(".comment-body a")?.click();
    expect(host.follow).toHaveBeenCalledTimes(1);
    expect(host.follow.mock.calls[0]?.[0].textContent).toBe("the notes");
    card(panel, 2).querySelector<HTMLButtonElement>("button.comment-place")?.click();
    expect(host.jumpToLine).toHaveBeenCalledWith(5);
    expect(card(panel, 2).classList.contains("selected")).toBe(true);
    // Without a place to go (none here, though core always gives one now), nothing to press.
    expect(card(panel, 4).querySelector<HTMLButtonElement>("button.comment-place")?.disabled).toBe(
      true,
    );
  });

  it("a detached comment with no heading left goes to the note's top", async () => {
    const { fake, panel, host, controller } = setup();
    const p = review(A);
    // As core sends one whose headings are all gone: pinned to nothing, its place the first line.
    const c4 = p.comments[3];
    if (!c4) throw new Error("no C4");
    c4.jumpLine = 1;
    fake.setReview(A, p);
    await controller.load();
    const place = card(panel, 4).querySelector<HTMLButtonElement>("button.comment-place");
    expect(place?.disabled).toBe(false);
    place?.click();
    expect(host.jumpToLine).toHaveBeenCalledWith(1);
    expect(card(panel, 4).classList.contains("selected")).toBe(true);
  });

  it("a click on a card's quote or header goes to its text and flashes it", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const { panel, host, controller, docEl } = setup();
      await controller.load();
      const paragraph = docEl.querySelector("p");
      card(panel, 1).querySelector<HTMLElement>(".comment-quote")?.click();
      expect(host.jumpToLine).toHaveBeenLastCalledWith(3);
      expect(card(panel, 1).classList.contains("selected")).toBe(true);
      expect(paragraph?.classList.contains("lx-flash")).toBe(true);
      vi.advanceTimersByTime(600);
      expect(paragraph?.classList.contains("lx-flash")).toBe(false);

      card(panel, 2).querySelector<HTMLElement>(".comment-id")?.click();
      expect(host.jumpToLine).toHaveBeenLastCalledWith(5);
      expect(card(panel, 2).classList.contains("selected")).toBe(true);
      expect(docEl.querySelector("li")?.classList.contains("lx-flash")).toBe(true);

      // Its buttons and its thread don't go anywhere.
      host.jumpToLine.mockClear();
      action(panel, 1, "reply").click();
      card(panel, 1).querySelector<HTMLElement>(".comment-body")?.click();
      expect(host.jumpToLine).not.toHaveBeenCalled();
      expect(card(panel, 2).classList.contains("selected")).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("highlights a quote where core found it in the note's text", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    const gauge = "check the gauge";
    /** The note: a title, then one paragraph saying `text` twice over. */
    const note = (text: string): string =>
      `<h1 data-sourcepos="1:1-1:3">A</h1>\n<p data-sourcepos="3:1-3:${String(text.length)}">${text}</p>`;
    /** Each painted range: its text, and where it starts in the paragraph. */
    const painted = (): [string, number][] =>
      [...(CSS.highlights.get("comment") as unknown as FakeHighlight)].map((r) => [
        r.toString(),
        r.startOffset,
      ]);
    const on = (textStart: number | null, textEnd: number | null): CommentView =>
      comment({
        id: 1,
        startLine: 3,
        endLine: 3,
        jumpLine: 3,
        quote: gauge,
        textStart,
        textEnd,
        entries: [you("Which gauge?")],
      });

    // The first of two, in a note unchanged since: the note's text is "A check the gauge now. …".
    const twice = "check the gauge now. check the gauge later.";
    const first = setup({ html: note(twice) });
    first.fake.setReview(A, { ...review(A), comments: [on(2, 17)], openCount: 1 });
    await first.controller.load();
    expect(painted()).toEqual([[gauge, 0]]);
    first.controller.dispose();

    // Edited outside Lectern, so that the text before each one points at the other: core still
    // found the first, by what follows it, and the highlight goes there too.
    const swapped =
      "Today: check the gauge now and log the reading in the book. Before: check the gauge later.";
    const second = setup({ html: note(swapped) });
    second.fake.setReview(A, { ...review(A), comments: [on(9, 24)], openCount: 1 });
    await second.controller.load();
    expect(painted()).toEqual([[gauge, swapped.indexOf(gauge)]]);
    second.controller.dispose();

    // Where the note's text there isn't the quote (the two sides disagree), the quote's first place
    // in its blocks.
    const third = setup({ html: note(twice) });
    third.fake.setReview(A, { ...review(A), comments: [on(24, 39)], openCount: 1 });
    await third.controller.load();
    expect(painted()).toEqual([[gauge, 0]]);
    third.fake.setReview(A, { ...review(A), comments: [on(23, 38)], openCount: 1 });
    await third.controller.load();
    expect(painted()).toEqual([[gauge, twice.lastIndexOf(gauge)]]);
  });

  it("indexes the note's text once per render, and only for comments that need it", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    const { fake, controller, docEl, rerender } = setup();
    const walk = vi.spyOn(document, "createTreeWalker");
    // The note's text indexed whole: a walk over its text and elements from its root.
    const builds = (): number =>
      walk.mock.calls.filter(
        ([root, show]) =>
          root === docEl && show === (NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT),
      ).length;
    // Without places in the text, nothing is indexed whole.
    await controller.load();
    expect(builds()).toBe(0);

    // "Tide sync The client sends at most fifty changes per batch to the hub. Retry …"
    const p = review(A);
    const [c1, c2] = p.comments;
    if (!c1 || !c2) throw new Error("no C1 or C2");
    Object.assign(c1, { textStart: 21, textEnd: 48 });
    Object.assign(c2, { textStart: 71, textEnd: 97 });
    fake.setReview(A, p);
    await controller.load();
    expect(builds()).toBe(1);
    await controller.load();
    expect(builds()).toBe(1);
    const ranges = () => [...(CSS.highlights.get("comment") as unknown as FakeHighlight)];
    expect(ranges().map((r) => r.toString())).toEqual([
      "sends at most fifty changes",
      "Retry with a short backoff",
    ]);

    // A new render is a new text: indexed again, and the highlights are in it.
    rerender();
    await controller.load();
    expect(builds()).toBe(2);
    expect(ranges().every((r) => r.startContainer.isConnected)).toBe(true);
    expect(ranges().map((r) => r.toString())).toEqual([
      "sends at most fifty changes",
      "Retry with a short backoff",
    ]);
  });

  it("a file that isn't a Lectern comments file says so and offers nothing to add", async () => {
    const { fake, panel, host, controller, docEl } = setup();
    const reason = "This file isn't a Lectern comments file, so Lectern won't change it.";
    fake.setReview(A, { ...review(A), readOnly: reason, comments: [], openCount: 0 });
    await controller.load();
    const banner = panel.commentsPane.querySelector<HTMLElement>(".comments-banner");
    expect(banner?.hidden).toBe(false);
    expect(banner?.textContent).toBe(reason);
    expect(panel.commentsPane.querySelector(".comment-card")).toBeNull();

    selectBetween(docEl, "sends", "fifty");
    await selectionSettles();
    expect(host.scroller.querySelector<HTMLElement>("button.lx-sel-comment")?.hidden).toBe(true);
    controller.addFromSelection();
    controller.addAtTop();
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    expect(host.toast).toHaveBeenLastCalledWith(reason);
    docEl.querySelector("p")?.dispatchEvent(new MouseEvent("pointermove", { bubbles: true }));
    expect(host.scroller.querySelector<HTMLElement>("button.lx-block-plus")?.hidden).toBe(true);
  });

  it("op failure toasts and keeps the reply draft", async () => {
    const { fake, panel, host, controller } = setup();
    await controller.load();
    fake.failNextReviewOp("Couldn't save the comment: the file kept changing");
    action(panel, 1, "reply").click();
    const area = typeReply(panel, 1, "Keep this text");
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith("Couldn't save the comment: the file kept changing");
    });
    expect(card(panel, 1).querySelector("textarea")?.value).toBe("Keep this text");
    // A reload while the draft is open keeps it.
    fake.emit("review-changed", { path: A });
    await settle();
    expect(card(panel, 1).querySelector("textarea")?.value).toBe("Keep this text");
    expect(fake.reviewCalls()).toBeGreaterThan(0);
  });

  it("a reply that fails after the note changed leaves the other note's drafts alone", async () => {
    const { fake, panel, host, state, controller, rerender } = setup();
    fake.setReview(B, review(B));
    // B, with a C1 of its own and a draft on its C2, then back to A.
    state.path = B;
    await controller.load();
    action(panel, 2, "reply").click();
    typeReply(panel, 2, "B's reply");
    state.path = A;
    rerender();
    await vi.waitFor(() => {
      expect(cards(panel)).toHaveLength(3);
    });
    expect(card(panel, 2).querySelector("textarea")).toBeNull();
    const gate = deferred();
    const reviewOp = vi.spyOn(fake, "reviewOp").mockImplementationOnce(async () => {
      await gate.promise;
      throw new Error("Couldn't save the comment: the file kept changing");
    });
    action(panel, 1, "reply").click();
    ctrlEnter(typeReply(panel, 1, "A's reply"));
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(1);
    });
    // B again while A's reply is on its way; it fails there.
    state.path = B;
    rerender();
    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith(
        "Error: Couldn't save the comment: the file kept changing",
      );
    });
    await vi.waitFor(() => {
      expect(cards(panel)).toHaveLength(3);
    });
    expect(card(panel, 1).querySelector("textarea")).toBeNull();
    expect(card(panel, 2).querySelector("textarea")?.value).toBe("B's reply");
    // Nor does A's text turn up on B after a reload, or when B's C1 opens a reply.
    fake.emit("review-changed", { path: B });
    await settle();
    expect(card(panel, 1).querySelector("textarea")).toBeNull();
    action(panel, 1, "reply").click();
    expect(card(panel, 1).querySelector("textarea")?.value).toBe("");
    expect(card(panel, 2).querySelector("textarea")?.value).toBe("B's reply");
    // Back on A, its reply is still its draft, ready to send again.
    state.path = A;
    rerender();
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelector("textarea")?.value).toBe("A's reply");
    });
    expect(card(panel, 1).querySelector("textarea")?.disabled).toBe(false);
  });

  it("disables a reply box while its reply is saved, and gives it back intact on failure", async () => {
    const { fake, panel, host, controller } = setup();
    await controller.load();
    const gate = deferred();
    const reviewOp = vi.spyOn(fake, "reviewOp").mockImplementationOnce(async () => {
      await gate.promise;
      throw new Error("Couldn't save the comment: the file kept changing");
    });
    action(panel, 1, "reply").click();
    const area = typeReply(panel, 1, "Keep this text");
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(1);
    });
    const send = () =>
      card(panel, 1).querySelector<HTMLButtonElement>('[data-action="send"]')?.disabled;
    expect(area.disabled).toBe(true);
    expect(send()).toBe(true);
    // A second send does nothing, and a reload keeps it disabled.
    ctrlEnter(area);
    fake.emit("review-changed", { path: A });
    await settle();
    expect(reviewOp).toHaveBeenCalledTimes(1);
    expect(card(panel, 1).querySelector("textarea")).toBe(area);
    expect(area.disabled).toBe(true);

    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalled();
    });
    await settle();
    expect(area.disabled).toBe(false);
    expect(send()).toBe(false);
    expect(area.value).toBe("Keep this text");
    // Sent again, it's the same text.
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(2);
    });
    expect(reviewOp).toHaveBeenLastCalledWith(A, { op: "reply", id: 1, text: "Keep this text" });
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelector("textarea")).toBeNull();
    });
  });

  it("a reload asked for during an operation applies after it", async () => {
    const { fake, panel, controller } = setup();
    await controller.load();
    const gate = deferred();
    const real = fake.reviewOp.bind(fake);
    vi.spyOn(fake, "reviewOp").mockImplementationOnce(async (path, op) => {
      // Answered as it stood before Claude's reply below.
      const payload = await real(path, op);
      await gate.promise;
      return payload;
    });
    action(panel, 2, "resolve").click();
    await settle();
    fake.agentReply(A, 1, "Claude", "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await settle();
    gate.resolve();
    await settle();
    expect(card(panel, 1).querySelectorAll(".comment-entry.agent")).toHaveLength(1);
    expect(card(panel, 1).dataset.status).toBe("replied");
    panel.commentsPane.querySelector<HTMLButtonElement>('[data-filter="all"]')?.click();
    expect(card(panel, 2).dataset.status).toBe("resolved");
  });

  it("a reply that couldn't be sent because the note changed waits as its draft", async () => {
    const { fake, panel, host, state, controller, rerender } = setup();
    fake.setReview(B, review(B));
    await controller.load();
    const gate = deferred();
    const real = fake.reviewOp.bind(fake);
    const reviewOp = vi.spyOn(fake, "reviewOp").mockImplementationOnce(async (path, op) => {
      await gate.promise;
      return real(path, op);
    });
    // A reply waiting behind an operation on its way, then another note.
    action(panel, 2, "resolve").click();
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(1);
    });
    action(panel, 1, "reply").click();
    ctrlEnter(typeReply(panel, 1, "Waiting reply"));
    state.path = B;
    rerender();
    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith(
        "Your reply wasn't sent: the note changed. It's kept as a draft.",
      );
    });
    expect(reviewOp).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(cards(panel)).toHaveLength(3);
    });
    expect(card(panel, 1).querySelector("textarea")).toBeNull();
    // Back on its note, it's in its box.
    state.path = A;
    rerender();
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelector("textarea")?.value).toBe("Waiting reply");
    });
    expect(card(panel, 1).querySelector("textarea")?.disabled).toBe(false);
  });

  it("copies a code block in a comment", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { fake, panel, host, controller } = setup();
    const p = review(A);
    p.comments[0]?.entries.push({
      author: "agent",
      name: "Claude",
      kind: null,
      text: "Like this:",
      html: '<p>Like this:</p>\n<div class="code-block" data-lang="toml" data-sourcepos="3:1-5:3"><div class="code-head"><span class="code-lang">toml</span><button type="button" class="code-copy" aria-label="Copy code">Copy</button></div><pre><code class="language-toml">[sync]\nbatch = 50\n</code></pre></div>',
    });
    fake.setReview(A, p);
    await controller.load();
    card(panel, 1).querySelector<HTMLButtonElement>(".comment-body .code-copy")?.click();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith("Copied the code");
    });
    expect(writeText).toHaveBeenCalledWith("[sync]\nbatch = 50\n");
    writeText.mockRejectedValueOnce(new Error("denied"));
    card(panel, 1).querySelector<HTMLButtonElement>(".comment-body .code-copy")?.click();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenLastCalledWith("Couldn't copy to the clipboard");
    });
    Reflect.deleteProperty(navigator, "clipboard");
  });

  it("operations apply in the order they were made", async () => {
    const { fake, panel, controller } = setup();
    await controller.load();
    const gate = deferred();
    const real = fake.reviewOp.bind(fake);
    const reviewOp = vi.spyOn(fake, "reviewOp").mockImplementationOnce(async (path, op) => {
      // Answered as it stood then, but only after the next one could have been.
      const payload = await real(path, op);
      await gate.promise;
      return payload;
    });
    action(panel, 1, "resolve").click();
    action(panel, 2, "resolve").click();
    await settle();
    // The second waits for the first.
    expect(reviewOp).toHaveBeenCalledTimes(1);
    gate.resolve();
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(2);
    });
    await settle();
    panel.commentsPane.querySelector<HTMLButtonElement>('[data-filter="all"]')?.click();
    expect(card(panel, 1).dataset.status).toBe("resolved");
    expect(card(panel, 2).dataset.status).toBe("resolved");
  });

  it("a reload while typing a reply keeps the box, its focus and its selection", async () => {
    const { fake, panel, controller } = setup();
    await controller.load();
    action(panel, 1, "reply").click();
    const area = typeReply(panel, 1, "Make it a setting");
    area.focus();
    area.setSelectionRange(5, 7, "backward");
    fake.agentReply(A, 1, "Claude", "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelectorAll(".comment-entry.agent")).toHaveLength(1);
    });
    expect(card(panel, 1).querySelector("textarea")).toBe(area);
    expect(document.activeElement).toBe(area);
    expect([area.selectionStart, area.selectionEnd, area.selectionDirection]).toEqual([
      5,
      7,
      "backward",
    ]);
    expect(area.value).toBe("Make it a setting");
  });

  it("does nothing for clicks on the note in focus mode", async () => {
    vi.stubGlobal("Highlight", FakeHighlight);
    vi.stubGlobal("CSS", { highlights: new Map<string, FakeHighlight>() });
    const { panel, host, controller, docEl } = setup({ focusMode: true, panelOpen: false });
    await controller.load();
    // The highlights stay.
    expect(CSS.highlights.has("comment")).toBe(true);
    docEl.querySelector<HTMLElement>('.lx-cdot[data-comments="2"]')?.click();
    expect(panel.tab).toBe("outline");
    expect(host.openPanel).not.toHaveBeenCalled();
    expect(panel.commentsPane.querySelector(".comment-card.selected")).toBeNull();
  });
  it("anchor from selection uses block lines and a chrome-free quote", async () => {
    const { fake, panel, host, controller, docEl } = setup({
      html: `<h1 id="tide-sync" data-sourcepos="1:1-1:11">Tide sync</h1>
<p data-sourcepos="3:1-3:70">The client sends at most fifty<sup data-sourcepos="3:31-3:34" class="footnote-ref"><a href="#fn-1" id="fnref-1" data-footnote-ref="">1</a></sup> changes per batch.</p>
<ul data-sourcepos="5:1-6:20">
<li data-sourcepos="5:1-5:30"><input type="checkbox" disabled=""> Retry with a short backoff</li>
<li data-sourcepos="6:1-6:20">Log every failure</li>
</ul>`,
    });
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    // From the paragraph, over its footnote mark, into the first item past its checkbox.
    selectBetween(docEl, "sends", "Retry");
    await selectionSettles();
    const button = host.scroller.querySelector<HTMLButtonElement>("button.lx-sel-comment");
    expect(button?.hidden).toBe(false);
    expect(button?.textContent).toBe("Comment");
    // In the pane, never in the note.
    expect(docEl.querySelector(".lx-sel-comment")).toBeNull();
    button?.click();
    const area = editorText();
    expect(document.activeElement).toBe(area);
    area.value = "Why fifty?";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledWith(A, {
        op: "add",
        anchor: {
          startLine: 3,
          endLine: 5,
          quote: "sends at most fifty changes per batch. Retry",
          // The text before it, which tells it apart from the same words elsewhere in its lines.
          prefix: "Tide sync The client ",
        },
        text: "Why fifty?",
      });
    });
    // Saved: the editor goes, and the new comment (the highest id) is the one on show.
    await vi.waitFor(() => {
      expect(card(panel, 5).classList.contains("selected")).toBe(true);
    });
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    expect(panel.tab).toBe("comments");

    // A selection reaching outside the note, or none, adds nothing.
    document.getSelection()?.removeAllRanges();
    await selectionSettles();
    expect(button?.hidden).toBe(true);
    expect(controller.addFromSelection()).toBe(false);
    const outside = document.createElement("p");
    outside.textContent = "Outside the note";
    document.body.append(outside);
    const range = document.createRange();
    range.setStart(docEl.querySelector("h1")?.firstChild ?? docEl, 0);
    range.setEnd(outside.firstChild ?? outside, 3);
    document.getSelection()?.removeAllRanges();
    document.getSelection()?.addRange(range);
    await selectionSettles();
    expect(button?.hidden).toBe(true);
    expect(controller.addFromSelection()).toBe(false);

    // A triple click ends at the very start of the next block: the comment is on the paragraph.
    const paragraph = docEl.querySelector("p");
    const opening = [...(paragraph?.childNodes ?? [])].find((n) => n instanceof Text);
    const item = docEl.querySelector("li");
    if (!opening || !item) throw new Error("no paragraph text or item");
    const triple = document.createRange();
    triple.setStart(opening, 0);
    triple.setEnd(item, 0);
    document.getSelection()?.removeAllRanges();
    document.getSelection()?.addRange(triple);
    expect(controller.addFromSelection()).toBe(true);
    const again = editorText();
    again.value = "Per batch, or per minute?";
    again.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(again);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenLastCalledWith(A, {
        op: "add",
        anchor: {
          startLine: 3,
          endLine: 3,
          quote: "The client sends at most fifty changes per batch.",
          prefix: "Tide sync ",
        },
        text: "Per batch, or per minute?",
      });
    });
  });

  it("add at top picks the first visible block", async () => {
    const { fake, host, controller, docEl } = setup();
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    // The pane's top is at 100: the heading starts above it, the paragraph is the first below.
    vi.spyOn(host.scroller, "getBoundingClientRect").mockReturnValue(box(100, 500));
    const tops: [string, number][] = [
      ["h1", 70],
      ["p", 130],
      ["ul", 200],
      ["li", 200],
      ["li:last-child", 240],
    ];
    for (const [selector, top] of tops) {
      for (const el of docEl.querySelectorAll<HTMLElement>(selector)) {
        vi.spyOn(el, "getBoundingClientRect").mockReturnValue(box(top, 30));
      }
    }
    controller.addAtTop();
    const area = editorText();
    area.value = "Is fifty enough?";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledWith(A, {
        op: "add",
        anchor: {
          startLine: 3,
          endLine: 3,
          quote: "The client sends at most fifty changes per batch to the hub.",
          // A whole block: nothing before it to tell apart.
          prefix: "",
        },
        text: "Is fifty enough?",
      });
    });

    // Scrolled partway into the paragraph: it crosses the top, so it's the one, not the list below.
    vi.spyOn(docEl.querySelector("p") ?? docEl, "getBoundingClientRect").mockReturnValue(
      box(90, 60),
    );
    controller.addAtTop();
    const again = editorText();
    again.value = "And per minute?";
    again.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(again);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenLastCalledWith(A, {
        op: "add",
        anchor: expect.objectContaining({ startLine: 3, endLine: 3 }) as unknown,
        text: "And per minute?",
      });
    });

    // The deepest block crossing it wins: an item rather than its list.
    for (const [selector, top, height] of [
      ["p", 40, 50],
      ["ul", 95, 80],
      ["li", 95, 40],
      ["li:last-child", 135, 40],
    ] as const) {
      for (const el of docEl.querySelectorAll<HTMLElement>(selector)) {
        vi.spyOn(el, "getBoundingClientRect").mockReturnValue(box(top, height));
      }
    }
    controller.addAtTop();
    const third = editorText();
    third.value = "How short?";
    third.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(third);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenLastCalledWith(A, {
        op: "add",
        anchor: { startLine: 5, endLine: 5, quote: "Retry with a short backoff", prefix: "" },
        text: "How short?",
      });
    });
  });

  it("asking for another comment while the editor holds text keeps it", async () => {
    const { controller, docEl } = setup();
    await controller.load();
    controller.addAtTop();
    const area = editorText();
    area.value = "Half a thought";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    // A press elsewhere leaves it open, and another comment asked for brings it back.
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    area.blur();
    selectBetween(docEl, "Log", "failure");
    expect(controller.addFromSelection()).toBe(true);
    expect(editorText()).toBe(area);
    expect(area.value).toBe("Half a thought");
    expect(document.activeElement).toBe(area);
    controller.addAtTop();
    expect(area.value).toBe("Half a thought");
  });

  it("a half-written comment waits for its note and comes back with it", async () => {
    const { fake, state, controller, rerender } = setup();
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    controller.addAtTop();
    const area = editorText();
    area.value = "Half a thought";
    area.dispatchEvent(new Event("input", { bubbles: true }));

    // Another note opens: the editor goes, its text kept for its own note only.
    state.path = B;
    rerender();
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    await frames(3);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);

    // Back on the note, a couple of frames on, it's open again with the text, by its block.
    state.path = A;
    rerender();
    await frames(3);
    expect(editorText().value).toBe("Half a thought");
    expect(document.activeElement).toBe(editorText());
    ctrlEnter(editorText());
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledWith(A, {
        op: "add",
        anchor: {
          startLine: 1,
          endLine: 1,
          quote: "Tide sync",
          prefix: "",
        },
        text: "Half a thought",
      });
    });
    await vi.waitFor(() => {
      expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    });
    // Saved, it's gone for good.
    rerender();
    await frames(3);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
  });

  it("a comment whose save found its note gone is kept for the note", async () => {
    const { fake, host, state, controller, rerender } = setup();
    await controller.load();
    controller.addAtTop();
    const area = editorText();
    area.value = "Lost in transit?";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    // The save waits its turn behind a load on its way; meanwhile another note opens.
    const gate = deferred();
    const real = fake.loadReview.bind(fake);
    vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path) => {
      await gate.promise;
      return real(path);
    });
    void controller.load();
    ctrlEnter(area);
    state.path = B;
    rerender();
    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith(
        "Your comment wasn't saved: the note changed. It's kept as a draft.",
      );
    });
    state.path = A;
    rerender();
    await frames(3);
    expect(editorText().value).toBe("Lost in transit?");
  });

  it("a comment saving while comments hide and show isn't brought back to save again", async () => {
    const { fake, panel, state, controller } = setup();
    await controller.load();
    const gate = deferred();
    const real = fake.reviewOp.bind(fake);
    const reviewOp = vi.spyOn(fake, "reviewOp").mockImplementationOnce(async (path, op) => {
      await gate.promise;
      return real(path, op);
    });
    controller.addAtTop();
    const area = editorText();
    area.value = "Saved once";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledTimes(1);
    });
    // Hidden and shown again while it saves: the editor waits for the save.
    state.visible = false;
    controller.setVisible(false);
    state.visible = true;
    controller.setVisible(true);
    await frames(3);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    gate.resolve();
    await vi.waitFor(() => {
      expect(card(panel, 5).textContent).toContain("Saved once");
    });
    await frames(3);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    expect(reviewOp).toHaveBeenCalledTimes(1);
  });

  it("a comment whose save fails while comments are hidden comes back", async () => {
    const { fake, host, state, controller } = setup();
    await controller.load();
    const gate = deferred();
    vi.spyOn(fake, "reviewOp").mockImplementationOnce(async () => {
      await gate.promise;
      throw new Error("Couldn't save the comment: the file kept changing.");
    });
    controller.addAtTop();
    const area = editorText();
    area.value = "Try again";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(area);
    state.visible = false;
    controller.setVisible(false);
    state.visible = true;
    controller.setVisible(true);
    await frames(3);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalled();
    });
    await frames(3);
    expect(editorText().value).toBe("Try again");
  });

  it("a failed save comes back first, and another draft kept meanwhile isn't lost", async () => {
    const { fake, host, state, controller } = setup();
    await controller.load();
    const gate = deferred();
    vi.spyOn(fake, "reviewOp").mockImplementationOnce(async () => {
      await gate.promise;
      throw new Error("Couldn't save the comment: the file kept changing.");
    });
    const type = (text: string): HTMLTextAreaElement => {
      const area = editorText();
      area.value = text;
      area.dispatchEvent(new Event("input", { bubbles: true }));
      return area;
    };
    const hideAndShow = async (show: boolean): Promise<void> => {
      state.visible = false;
      controller.setVisible(false);
      if (show) {
        state.visible = true;
        controller.setVisible(true);
      }
      await frames(3);
    };
    controller.addAtTop();
    ctrlEnter(type("First thought"));
    // Hidden and shown while it saves: a second comment, typed and hidden in turn.
    await hideAndShow(true);
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    controller.addAtTop();
    expect(editorText().value).toBe("");
    type("Second thought");
    await hideAndShow(false);

    // The first one's save fails: shown again, it's the one back in the editor.
    gate.resolve();
    await vi.waitFor(() => {
      expect(host.toast).toHaveBeenCalledWith(expect.stringContaining("the file kept changing"));
    });
    state.visible = true;
    controller.setVisible(true);
    await frames(3);
    expect(editorText().value).toBe("First thought");
    // Put away, the second is still kept, and comes back when a comment is asked for.
    editorText().dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
    controller.addAtTop();
    expect(editorText().value).toBe("Second thought");
  });

  it("takes the text before a selection from the note's index, built once per render", async () => {
    const paragraphs = Array.from(
      { length: 1200 },
      (_, i) => `Reading ${String(i)} came from station ${String(i % 7)}.`,
    );
    const html = paragraphs
      .map(
        (t, i) =>
          `<p data-sourcepos="${String(2 * i + 1)}:1-${String(2 * i + 1)}:${String(t.length)}">${t}</p>`,
      )
      .join("\n");
    const { fake, controller, docEl, rerender } = setup({ html });
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    const walk = vi.spyOn(document, "createTreeWalker");
    // The note's text indexed whole: a walk over its text and elements from its root.
    const builds = (): number =>
      walk.mock.calls.filter(
        ([root, show]) =>
          root === docEl && show === (NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT),
      ).length;
    const text = paragraphs.join(" ");
    /** Selects "station" in paragraph `i`, comments on it, and returns the prefix it was sent with. */
    const commentOn = async (i: number): Promise<string> => {
      const node = docEl.querySelectorAll("p")[i]?.firstChild;
      if (!(node instanceof Text)) throw new Error(`no paragraph ${String(i)}`);
      const range = document.createRange();
      const at = node.data.indexOf("station");
      range.setStart(node, at);
      range.setEnd(node, at + "station".length);
      document.getSelection()?.removeAllRanges();
      document.getSelection()?.addRange(range);
      expect(controller.addFromSelection()).toBe(true);
      const area = editorText();
      area.value = `About ${String(i)}`;
      area.dispatchEvent(new Event("input", { bubbles: true }));
      const calls = reviewOp.mock.calls.length;
      ctrlEnter(area);
      await vi.waitFor(() => {
        expect(reviewOp.mock.calls.length).toBe(calls + 1);
      });
      const op = reviewOp.mock.calls[calls]?.[1];
      return op?.op === "add" ? op.anchor.prefix : "";
    };
    /** The 32 characters of the note's text before "station" in paragraph `i`. */
    const expected = (i: number): string => {
      const start = paragraphs.slice(0, i).reduce((n, t) => n + t.length + 1, 0);
      const at = start + (paragraphs[i] ?? "").indexOf("station");
      return text.slice(at - 32, at);
    };

    expect(await commentOn(1000)).toBe(expected(1000));
    expect(expected(1000)).toBe("ation 5. Reading 1000 came from ");
    expect(await commentOn(10)).toBe(expected(10));
    expect(builds()).toBe(1);
    // A new render: indexed again.
    rerender();
    expect(await commentOn(999)).toBe(expected(999));
    expect(builds()).toBe(2);
    // jsdom's selector matching makes a big note slow to mark; the browser isn't.
  }, 20_000);

  it("reattach mode sends a reattach op", async () => {
    const { fake, panel, host, controller, docEl } = setup();
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    const banner = (): HTMLElement | null =>
      panel.commentsPane.querySelector<HTMLElement>(".comments-attach");
    const selButton = (): HTMLButtonElement | null =>
      host.scroller.querySelector<HTMLButtonElement>("button.lx-sel-comment");

    // Cancel, and Esc, leave attach mode.
    action(panel, 4, "reattach").click();
    expect(banner()?.hidden).toBe(false);
    expect(banner()?.textContent).toContain("Select the new text for C4, then press Attach here.");
    banner()?.querySelector<HTMLButtonElement>("button")?.click();
    expect(banner()?.hidden).toBe(true);
    action(panel, 4, "reattach").click();
    expect(banner()?.hidden).toBe(false);
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    expect(banner()?.hidden).toBe(true);
    selectBetween(docEl, "Log", "failure");
    await selectionSettles();
    expect(selButton()?.textContent).toBe("Comment");

    action(panel, 4, "reattach").click();
    await selectionSettles();
    expect(selButton()?.textContent).toBe("Attach C4 here");
    selButton()?.click();
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledWith(A, {
        op: "reattach",
        id: 4,
        anchor: {
          startLine: 6,
          endLine: 6,
          quote: "Log every failure",
          prefix: "hub. Retry with a short backoff ",
        },
      });
    });
    await vi.waitFor(() => {
      expect(card(panel, 4).dataset.state).toBe("anchored");
    });
    expect(banner()?.hidden).toBe(true);
    expect(card(panel, 4).classList.contains("selected")).toBe(true);
    // No editor: the selection is all a re-attach needs.
    expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden ?? true).toBe(true);
  });

  it("edit sends the entry index", async () => {
    const { fake, panel, controller } = setup();
    const p = review(A);
    p.comments[1]?.entries.push(you("Under a second?"));
    fake.setReview(A, p);
    await controller.load();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    const entries = (): HTMLElement[] => [
      ...card(panel, 2).querySelectorAll<HTMLElement>(".comment-entry"),
    ];
    // Only your own entries can be edited.
    expect(entries().map((e) => e.querySelector('[data-action="edit"]') !== null)).toEqual([
      true,
      false,
      true,
    ]);
    entries()[2]?.querySelector<HTMLButtonElement>('[data-action="edit"]')?.click();
    const area = card(panel, 2).querySelector<HTMLTextAreaElement>("textarea");
    if (!area) throw new Error("no edit box");
    // The raw text, in place of the entry's body.
    expect(area.value).toBe("Under a second?");
    expect(entries()[2]?.querySelector(".comment-body")).toBeNull();
    expect(document.activeElement).toBe(area);
    area.value = "Under two seconds?";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    ctrlEnter(area);
    await vi.waitFor(() => {
      expect(reviewOp).toHaveBeenCalledWith(A, {
        op: "edit",
        id: 2,
        entry: 2,
        text: "Under two seconds?",
      });
    });
    await vi.waitFor(() => {
      expect(entries()[2]?.querySelector(".comment-body")?.textContent).toBe("Under two seconds?");
    });
    expect(card(panel, 2).querySelector("textarea")).toBeNull();

    // Esc puts the entry back as it was.
    entries()[0]?.querySelector<HTMLButtonElement>('[data-action="edit"]')?.click();
    const again = card(panel, 2).querySelector<HTMLTextAreaElement>("textarea");
    again?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    expect(card(panel, 2).querySelector("textarea")).toBeNull();
    expect(entries()[0]?.querySelector(".comment-body")?.textContent).toBe("How short?");
    expect(reviewOp).toHaveBeenCalledTimes(1);
  });

  it("add-comment while hidden shows comments", async () => {
    const { host, controller, docEl } = setup({ visible: false });
    await controller.load();
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(0);
    controller.addAtTop();
    expect(host.showComments).toHaveBeenCalledTimes(1);
    expect(docEl.querySelectorAll(".lx-cdot")).toHaveLength(2);
    expect(editorText()).toBeDefined();
  });
});
