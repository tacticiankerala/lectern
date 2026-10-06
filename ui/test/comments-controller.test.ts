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
  return { author: "you", kind: null, text, html: `<p>${text}</p>` };
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
          { author: "claude", kind: "question", text: "Seconds?", html: "<p>Seconds?</p>" },
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
  } satisfies CommentsHost;
  const controller = new CommentsController(host);
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

/** Stands in for the CSS Custom Highlight API, which jsdom lacks. */
class FakeHighlight extends Set<Range> {
  priority = 0;
  constructor(...ranges: Range[]) {
    super(ranges);
  }
}

afterEach(() => {
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
    expect(second.querySelector(".comment-entry.claude .comment-author")?.textContent).toBe(
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
    // Re-attach is for moved and detached comments only, and comes with adding comments.
    expect(card(panel, 3).querySelector('[data-action="reattach"]')).toBeNull();
    expect(action(panel, 4, "reattach").disabled).toBe(true);
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

    fake.claudeReply(A, 1, "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelectorAll(".comment-entry.claude")).toHaveLength(1);
    });
    expect(card(panel, 1).dataset.status).toBe("replied");
    expect(panel.commentsPane.ownerDocument.querySelector("[role=tab].flash")).not.toBeNull();
    // The panel is closed, so the header badge pulses too.
    expect(host.pulseBadge).toHaveBeenCalledTimes(1);
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
      author: "claude",
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
    // A detached comment with no surviving heading has nowhere to go.
    expect(card(panel, 4).querySelector<HTMLButtonElement>("button.comment-place")?.disabled).toBe(
      true,
    );
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
    fake.claudeReply(A, 1, "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await settle();
    gate.resolve();
    await settle();
    expect(card(panel, 1).querySelectorAll(".comment-entry.claude")).toHaveLength(1);
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
      author: "claude",
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
    fake.claudeReply(A, 1, "reply", "Fifty fits one request.");
    fake.emit("review-changed", { path: A });
    await vi.waitFor(() => {
      expect(card(panel, 1).querySelectorAll(".comment-entry.claude")).toHaveLength(1);
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
});
