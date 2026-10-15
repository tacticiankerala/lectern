// HTML pages in the app, on the fake backend: the sandboxed frame and its colour scheme, and what
// the page's bridge reports (played here by messages from the frame's window) driving the outline,
// find, the reading position, links and the notice of what the page loads. Keys from inside the
// frame come from Rust as `frame-key`, which the fake sends on request.
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import { A, appRoot, deferred, fixtures, rendered, settle } from "./helpers";

const PAGE = "C:\\V\\artifacts\\report.html";
const OTHER = "C:\\V\\artifacts\\other.html";
const REMOTE = "C:\\V\\artifacts\\remote.html";
const PDF = "C:\\V\\artifacts\\brief.pdf";

const READY = {
  type: "ready",
  title: "Weekly report",
  headings: [
    { level: 1, text: "Weekly report", id: "lx-heading-1", offset: 0 },
    { level: 2, text: "Results", id: "results", offset: 14 },
    { level: 2, text: "Next steps", id: "next-steps", offset: 22 },
  ],
  blocks: [
    { text: "Weekly report", headingLevel: 1 },
    { text: "Results", headingLevel: 2 },
    { text: "Next steps", headingLevel: 2 },
    { text: "Rendered by script" },
  ],
  scroll: { fraction: 0, top: 0 },
  dark: false,
};

/** The report as its bridge reports it with the paragraph comments go on. */
const WITH_TEXT = {
  ...READY,
  headings: [
    { level: 1, text: "Weekly report", id: "lx-heading-1", offset: 0 },
    { level: 2, text: "Results", id: "results", offset: 14 },
    { level: 2, text: "Next steps", id: "next-steps", offset: 97 },
  ],
  blocks: [
    { text: "Weekly report", headingLevel: 1 },
    { text: "Results", headingLevel: 2 },
    { text: "Sign-ups rose steadily, but conversion dipped in week 3 before recovering." },
    { text: "Next steps", headingLevel: 2 },
    { text: "Rendered by script" },
  ],
};
const TEXT = WITH_TEXT.blocks.map((b) => b.text).join(" ");
/** Where the commented words start in the page's text. */
const DIP = TEXT.indexOf("conversion dipped in week 3");

/** The page's sidecar, with C1 on the commented words. */
function pageReview(): ReviewPayload {
  return {
    notePath: PAGE,
    sidecarPath: `${PAGE}.review.md`,
    noteWslPath: null,
    sidecarWslPath: null,
    exists: true,
    readOnly: null,
    comments: [
      {
        id: 1,
        status: "open",
        state: "anchored",
        startLine: 12,
        endLine: 12,
        headingPath: ["Weekly report", "Results"],
        jumpLine: 12,
        pinnedHeading: null,
        pinnedAt: null,
        quote: "conversion dipped in week 3",
        textStart: DIP,
        textEnd: DIP + 27,
        currentText: null,
        entries: [
          { author: "you", name: "You", kind: null, text: "weekly", html: "<p>weekly</p>" },
        ],
      },
    ],
    unreadable: [],
    openCount: 1,
  };
}

/** Waits for the comments module, its Comments tab shown with nothing read-only. */
async function commentsShown(): Promise<void> {
  document.querySelector<HTMLElement>("#lx-tab-comments")?.click();
  await vi.waitFor(() => {
    expect(document.querySelector("#lx-comments-pane .comments-empty")?.textContent).toBe(
      "No comments yet. Select text in the page to comment on it.",
    );
  });
  expect(document.querySelector<HTMLElement>("#lx-comments-pane .comments-banner")?.hidden).toBe(
    true,
  );
}

async function started(
  initial = PAGE,
  settings: { htmlPages?: "full" | "offline" | "static" } = {},
) {
  const fake = new FakeBackend(
    {
      ...fixtures({
        [PAGE]: rendered("report.html", ""),
        [OTHER]: rendered("other.html", ""),
        [REMOTE]: rendered("remote.html", ""),
        [PDF]: rendered("brief.pdf", ""),
      }),
      kinds: { [PAGE]: "html", [OTHER]: "html", [REMOTE]: "html", [PDF]: "pdf" },
      pageHosts: { [REMOTE]: ["cdn.example.test"] },
    },
    { initial: A },
  );
  if (settings.htmlPages) {
    await fake.setSettings({ htmlPages: settings.htmlPages });
  }
  const app = new App(fake, appRoot());
  await app.start();
  await app.open(initial, { push: true });
  return { fake, app };
}

function frame(): HTMLIFrameElement {
  const found = document.querySelector<HTMLIFrameElement>("#lx-doc-pane > iframe.lx-frame");
  if (!found) throw new Error("no frame");
  return found;
}

/** What the app has posted to the frame, from now on. */
function posts(): Record<string, unknown>[] {
  const sent: Record<string, unknown>[] = [];
  const win = frame().contentWindow;
  if (!win) throw new Error("no frame window");
  vi.spyOn(win, "postMessage").mockImplementation((message: unknown) => {
    sent.push(message as Record<string, unknown>);
  });
  return sent;
}

/** A message from the page's bridge: posted by the frame's own window, unless `source` says. */
async function bridge(data: Record<string, unknown>, source?: Window | null): Promise<void> {
  // The page's module loads on first use.
  await vi.waitFor(() => {
    expect(document.querySelector(".page-notice")).not.toBeNull();
  });
  await import("../src/page-frame");
  await settle(5);
  window.dispatchEvent(
    new MessageEvent("message", {
      data: { lx: 1, ...data },
      source: source === undefined ? frame().contentWindow : source,
    }),
  );
}

function press(init: KeyboardEventInit): void {
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
  );
}

function outlineLinks(): HTMLAnchorElement[] {
  return [...document.querySelectorAll<HTMLAnchorElement>("#lx-outline-pane a")];
}

afterEach(() => {
  Reflect.deleteProperty(navigator, "userActivation");
});

describe("an HTML page", () => {
  it("shows in a frame that allows scripts and never the same origin, in Lectern's colour scheme", async () => {
    const { app } = await started();
    expect(frame().getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame().style.colorScheme).toBe("light");
    expect(frame().getAttribute("src")).toMatch(
      /^\/__page\/d\/c\/V\/artifacts\/report\.html\?t=[^&]+&lxmode=full&v=\d+$/,
    );
    const sent = posts();
    await bridge(READY);
    expect(sent[0]).toMatchObject({ lx: 1, type: "theme" });
    app.updateSettings({ themeMode: "dark" });
    expect(frame().style.colorScheme).toBe("dark");
    // The page paints in the new theme's colours.
    expect(sent.filter((m) => m.type === "theme")).toHaveLength(2);
  });

  it("lists its headings in the outline, and a click scrolls the page to one", async () => {
    await started();
    const sent = posts();
    await bridge(READY);
    expect(outlineLinks().map((a) => a.textContent)).toEqual([
      "Weekly report",
      "Results",
      "Next steps",
    ]);
    outlineLinks()[1]?.click();
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", offset: 14 });
  });

  it("is searched by find, which paints its matches there, and F3 from inside it moves on", async () => {
    const { fake, app } = await started();
    const sent = posts();
    await bridge(READY);
    press({ key: "f", ctrlKey: true });
    const input = await vi.waitFor(() => {
      const found = document.querySelector<HTMLInputElement>(".find-bar:not([hidden]) input");
      if (!found) throw new Error("no find bar");
      return found;
    });
    input.value = "rendered by";
    input.dispatchEvent(new Event("input"));
    await vi.waitFor(() => {
      expect(document.querySelector(".find-count")?.textContent).toBe("1 / 1");
    });
    expect(sent).toContainEqual({
      lx: 1,
      type: "paint",
      layer: "find",
      ranges: [{ start: 33, end: 44 }],
    });
    expect(sent).toContainEqual({
      lx: 1,
      type: "paint",
      layer: "find-current",
      ranges: [{ start: 33, end: 44 }],
    });
    // The page changes: found again.
    await bridge({
      type: "blocks",
      headings: READY.headings,
      blocks: [...READY.blocks, { text: "Rendered by script again" }],
    });
    await vi.waitFor(() => {
      expect(document.querySelector(".find-count")?.textContent).toBe("1 / 2");
    });
    // F3 inside the frame comes from Rust.
    sent.length = 0;
    fake.frameKey({ key: "F3", code: "F3" });
    expect(document.querySelector(".find-count")?.textContent).toBe("2 / 2");
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", offset: 52, reveal: true });
    // Closed, the page's highlights go.
    expect(app.actions.run("escape")).toBe(true);
    expect(sent).toContainEqual({ lx: 1, type: "clear", layer: "find" });
    expect(sent).toContainEqual({ lx: 1, type: "clear", layer: "find-current" });
  });

  it("hands Lectern's shortcuts pressed inside it to Lectern", async () => {
    const { fake, app } = await started();
    fake.frameKey({ key: "F11", code: "F11" });
    expect(app.actions.inFocusMode).toBe(true);
    fake.frameKey({ key: "Escape", code: "Escape" });
    expect(app.actions.inFocusMode).toBe(false);
    expect(fake.fullscreen).toEqual([true, false]);
  });

  it("keeps its reading position: saved as it scrolls, and back once it is ready again", async () => {
    const { fake, app } = await started();
    await bridge(READY);
    const save = vi.spyOn(fake, "savePosition");
    await bridge({ type: "scroll", scroll: { fraction: 0.6, top: 2 } });
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledWith(PAGE, {
        headingId: null,
        offset: 0,
        line: null,
        fraction: 0.6,
      });
    });
    await app.open(A, { push: true });
    await app.nav.travel("back");
    const sent = posts();
    await bridge(READY);
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.6 });
    // A change on disk loads it again, at the same place.
    await bridge({ type: "scroll", scroll: { fraction: 0.3, top: 1 } });
    const old = frame();
    fake.emit("doc-changed", { path: PAGE });
    await vi.waitFor(() => {
      expect(frame()).not.toBe(old);
    });
    const again = posts();
    await bridge(READY);
    expect(again).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.3 });
  });

  it("lands on an anchor once the page is ready, over its saved place", async () => {
    const { fake, app } = await started(A);
    await fake.savePosition(PAGE, { headingId: null, offset: 0, line: null, fraction: 0.6 });
    // As a note's link to `report.html#results` opens it.
    await app.open(PAGE, { anchor: "results", push: true });
    const sent = posts();
    await bridge(READY);
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", offset: 14 });
    expect(sent.filter((m) => "fraction" in m)).toEqual([]);
    // An anchor that names no heading leaves the saved place (saved again: leaving saved where
    // the anchor had taken it).
    await app.open(A);
    await fake.savePosition(PAGE, { headingId: null, offset: 0, line: null, fraction: 0.6 });
    await app.open(PAGE, { anchor: "nowhere" });
    const again = posts();
    await bridge(READY);
    expect(again).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.6 });
  });

  it("opens a page a link in it leads to, with Back returning; a web link goes out", async () => {
    // The reader has just clicked in the page.
    Object.defineProperty(navigator, "userActivation", {
      value: { isActive: true },
      configurable: true,
    });
    const { fake, app } = await started();
    await bridge(READY);
    await bridge({ type: "link", href: "other.html" });
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(OTHER);
    });
    await app.nav.travel("back");
    expect(app.state.doc?.path).toBe(PAGE);
    const before = frame();
    await bridge(READY);
    await bridge({ type: "link", href: "https://example.com" });
    await vi.waitFor(() => {
      expect(fake.followed.map((t) => t.target)).toContain("https://example.com");
    });
    expect(fake.followed.at(-1)?.kind).toBe("external");
    expect(app.state.doc?.path).toBe(PAGE);
    expect(frame()).toBe(before);
  });

  it("hears nothing from another window, and drops a message it can't take", async () => {
    const { app } = await started();
    await bridge(READY, window);
    expect(outlineLinks()).toEqual([]);
    const errors = vi.spyOn(console, "error");
    await bridge({
      ...READY,
      blocks: [{ text: "x".repeat(3 * 1024 * 1024) }],
    });
    expect(outlineLinks()).toEqual([]);
    expect(errors).not.toHaveBeenCalled();
    expect(app.state.doc?.path).toBe(PAGE);
  });

  it("names what it loads from the web, or what was blocked, with Preferences a click away", async () => {
    const { app } = await started(REMOTE);
    const notice = () => document.querySelector<HTMLElement>(".page-notice");
    await vi.waitFor(() => {
      expect(notice()?.hidden).toBe(false);
    });
    expect(notice()?.querySelector(".page-notice-text")?.textContent).toBe(
      "This page loads from cdn.example.test",
    );
    // A page with nothing from the web has no notice.
    await app.open(PAGE);
    await settle(80);
    expect(notice()?.hidden).toBe(true);
    // Offline, the load is blocked, and the notice says where to change that.
    app.updateSettings({ htmlPages: "offline" });
    await settle(5);
    await app.open(REMOTE);
    await vi.waitFor(() => {
      expect(notice()?.querySelector(".page-notice-text")?.textContent).toBe(
        "Blocked loads from cdn.example.test",
      );
    });
    notice()?.querySelector<HTMLButtonElement>(".page-notice-prefs")?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".prefs-backdrop")?.hasAttribute("hidden")).toBe(false);
    });
  });

  it("takes a comment on text selected in it, and paints it there", async () => {
    const { fake } = await started();
    const sent = posts();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    await bridge(WITH_TEXT);
    await commentsShown();
    await bridge({
      type: "selection",
      selection: {
        quote: "conversion dipped in week 3",
        start: DIP,
        end: DIP + 27,
        rect: { x: 120, y: 200, width: 90, height: 18 },
      },
    });
    const button = document.querySelector<HTMLButtonElement>(".lx-sel-comment");
    await vi.waitFor(() => {
      expect(button?.hidden).toBe(false);
    });
    // By the selection's last line, inside the frame (here at the pane's corner).
    expect(button?.style.top).toBe("209px");
    button?.click();
    const area = document.querySelector<HTMLTextAreaElement>(".comment-editor textarea");
    if (!area) throw new Error("no editor");
    area.value = "weekly, not daily";
    area.dispatchEvent(new Event("input"));
    document.querySelector<HTMLButtonElement>(".comment-editor .btn.primary")?.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".comment-card.selected")?.getAttribute("data-id")).toBe("1");
    });
    expect(reviewOp).toHaveBeenCalledWith(
      PAGE,
      {
        op: "add",
        anchor: {
          startLine: 0,
          endLine: 0,
          quote: "conversion dipped in week 3",
          prefix: TEXT.slice(DIP - 32, DIP),
        },
        text: "weekly, not daily",
      },
      WITH_TEXT.blocks,
    );
    const marked = [{ start: DIP, end: DIP + 27 }];
    expect(sent).toContainEqual({ lx: 1, type: "paint", layer: "comment", ranges: marked });
    expect(sent).toContainEqual({ lx: 1, type: "paint", layer: "comment-focus", ranges: marked });
    // No dots and no "+" for a page.
    expect(document.querySelector(".lx-cdot")).toBeNull();
    expect(document.querySelector<HTMLElement>(".lx-block-plus")?.hidden).toBe(true);
  });

  it("shows a comment's card when its text is clicked in the page, and the card goes to it", async () => {
    const { fake } = await started();
    fake.setReview(PAGE, pageReview());
    const sent = posts();
    await bridge(WITH_TEXT);
    const marked = [{ start: DIP, end: DIP + 27 }];
    await vi.waitFor(() => {
      expect(sent).toContainEqual({ lx: 1, type: "paint", layer: "comment", ranges: marked });
    });
    await bridge({ type: "comment-click", offset: DIP + 5 });
    await vi.waitFor(() => {
      expect(document.querySelector(".comment-card.selected")?.getAttribute("data-id")).toBe("1");
    });
    expect(document.querySelector("#lx-tab-comments")?.getAttribute("aria-selected")).toBe("true");
    expect(sent).toContainEqual({ lx: 1, type: "paint", layer: "comment-focus", ranges: marked });
    // Text no comment marks shows nothing.
    await bridge({ type: "comment-click", offset: 2 });
    document.querySelector<HTMLButtonElement>(".comment-card[data-id='1'] .comment-place")?.click();
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", offset: DIP });
  });

  it("takes a detached comment to the heading core pinned it to, of several with its text", async () => {
    const { fake } = await started();
    const blocks = [
      { text: "First", headingLevel: 1 },
      { text: "Results", headingLevel: 2 },
      { text: "Early figures." },
      { text: "Second", headingLevel: 1 },
      { text: "Results", headingLevel: 2 },
    ];
    const text = blocks.map((b) => b.text).join(" ");
    const second = text.lastIndexOf("Results");
    const review = pageReview();
    const [c1] = review.comments;
    if (!c1) throw new Error("no comment");
    fake.setReview(PAGE, {
      ...review,
      comments: [
        {
          ...c1,
          state: "detached",
          quote: "Late figures came in",
          headingPath: ["Second", "Results"],
          pinnedHeading: "Results",
          pinnedAt: second,
          textStart: null,
          textEnd: null,
        },
      ],
    });
    const sent = posts();
    await bridge({ ...READY, headings: [], blocks });
    const place = await vi.waitFor(() => {
      const button = document.querySelector<HTMLButtonElement>(
        ".comment-card[data-id='1'] .comment-place",
      );
      if (!button) throw new Error("no card");
      return button;
    });
    place.click();
    expect(sent).toContainEqual({ lx: 1, type: "scroll-to", offset: second });
    expect(sent).not.toContainEqual({ lx: 1, type: "scroll-to", offset: text.indexOf("Results") });
  });

  it("loads its comments once it has reported its text, and again as the text changes", async () => {
    const { fake } = await started();
    fake.setReview(PAGE, pageReview());
    const load = vi.spyOn(fake, "loadReview");
    await settle();
    expect(load.mock.calls.filter(([path]) => path === PAGE)).toEqual([]);
    await bridge(WITH_TEXT);
    await vi.waitFor(() => {
      expect(load).toHaveBeenLastCalledWith(PAGE, WITH_TEXT.blocks);
    });
    const changed = [{ text: "A line the page added." }, ...WITH_TEXT.blocks];
    await bridge({ type: "blocks", headings: WITH_TEXT.headings, blocks: changed });
    await vi.waitFor(() => {
      expect(load).toHaveBeenLastCalledWith(PAGE, changed);
    });
    // The agent writing to the page's sidecar loads it again, with the page's text.
    fake.emit("review-changed", { path: PAGE });
    await vi.waitFor(() => {
      expect(load).toHaveBeenCalledTimes(3);
    });
    expect(load).toHaveBeenLastCalledWith(PAGE, changed);
  });

  it("loads its comments again when its text changes while they first load", async () => {
    const { fake, app } = await started();
    fake.setReview(PAGE, pageReview());
    const real = fake.loadReview.bind(fake);
    const gate = deferred();
    const load = vi.spyOn(fake, "loadReview").mockImplementationOnce(async (path, page) => {
      await gate.promise;
      return real(path, page);
    });
    await bridge(WITH_TEXT);
    await vi.waitFor(() => {
      expect(load).toHaveBeenCalledTimes(1);
    });
    const changed = [{ text: "A line the page added." }, ...WITH_TEXT.blocks];
    await bridge({ type: "blocks", headings: WITH_TEXT.headings, blocks: changed });
    await vi.waitFor(() => {
      expect(app.view.page?.text).toContain("A line the page added.");
    });
    gate.resolve();
    await vi.waitFor(() => {
      expect(load).toHaveBeenCalledTimes(2);
    });
    expect(load).toHaveBeenLastCalledWith(PAGE, changed);
  });

  it("leaves re-attach mode, or hides the Comment button, on Esc pressed inside it", async () => {
    const { fake } = await started();
    const review = pageReview();
    const [c1] = review.comments;
    if (!c1) throw new Error("no comment");
    fake.setReview(PAGE, { ...review, comments: [{ ...c1, state: "detached", quote: "gone" }] });
    await bridge(WITH_TEXT);
    const reattach = await vi.waitFor(() => {
      const button = document.querySelector<HTMLButtonElement>(
        ".comment-card[data-id='1'] [data-action='reattach']",
      );
      if (!button) throw new Error("no re-attach button");
      return button;
    });
    reattach.click();
    const banner = document.querySelector<HTMLElement>(".comments-attach");
    expect(banner?.hidden).toBe(false);
    fake.frameKey({ key: "Escape", code: "Escape" });
    expect(banner?.hidden).toBe(true);
    await bridge({
      type: "selection",
      selection: {
        quote: "Results",
        start: 14,
        end: 21,
        rect: { x: 0, y: 0, width: 0, height: 0 },
      },
    });
    const button = document.querySelector<HTMLButtonElement>(".lx-sel-comment");
    await vi.waitFor(() => {
      expect(button?.hidden).toBe(false);
    });
    fake.frameKey({ key: "Escape", code: "Escape" });
    expect(button?.hidden).toBe(true);
  });

  it("puts its Comment button away when another document opens", async () => {
    const { app } = await started();
    await bridge(WITH_TEXT);
    await commentsShown();
    await bridge({
      type: "selection",
      selection: {
        quote: "Results",
        start: 14,
        end: 21,
        rect: { x: 0, y: 0, width: 0, height: 0 },
      },
    });
    const button = document.querySelector<HTMLButtonElement>(".lx-sel-comment");
    await vi.waitFor(() => {
      expect(button?.hidden).toBe(false);
    });
    await app.open(OTHER);
    expect(button?.hidden).toBe(true);
  });

  it("takes Ctrl+Alt+M from inside it: a comment on its selection, else on the block at the top", async () => {
    const { fake } = await started();
    const reviewOp = vi.spyOn(fake, "reviewOp");
    await bridge(WITH_TEXT);
    await commentsShown();
    const save = async (text: string): Promise<void> => {
      fake.frameKey({ key: "m", code: "KeyM", ctrl: true, alt: true });
      const area = document.querySelector<HTMLTextAreaElement>(".comment-editor textarea");
      await vi.waitFor(() => {
        expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(false);
      });
      if (!area) throw new Error("no editor");
      area.value = text;
      area.dispatchEvent(new Event("input"));
      document.querySelector<HTMLButtonElement>(".comment-editor .btn.primary")?.click();
      await vi.waitFor(() => {
        expect(document.querySelector<HTMLElement>(".comment-editor")?.hidden).toBe(true);
      });
    };
    await save("Which week?");
    expect(reviewOp.mock.calls[0]?.[1]).toEqual({
      op: "add",
      anchor: { startLine: 0, endLine: 0, quote: "Weekly report", prefix: "" },
      text: "Which week?",
    });
    await bridge({
      type: "selection",
      selection: {
        quote: "Rendered by",
        start: TEXT.indexOf("Rendered by"),
        end: TEXT.indexOf("Rendered by") + 11,
        rect: { x: 0, y: 0, width: 0, height: 0 },
      },
    });
    await save("Which script?");
    expect(reviewOp.mock.calls[1]?.[1]).toMatchObject({
      anchor: { quote: "Rendered by" },
      text: "Which script?",
    });
  });

  it("tells Rust whether the focus is inside a page's or a PDF's frame", async () => {
    const { fake, app } = await started();
    frame().focus();
    window.dispatchEvent(new Event("blur"));
    await vi.waitFor(() => {
      expect(fake.frameFocus.at(-1)).toBe("page");
    });
    await app.open(PDF);
    frame().focus();
    window.dispatchEvent(new Event("blur"));
    await vi.waitFor(() => {
      expect(fake.frameFocus.at(-1)).toBe("pdf");
    });
    await app.open(A);
    await vi.waitFor(() => {
      expect(fake.frameFocus.at(-1)).toBe("none");
    });
  });
});

describe("Preferences", () => {
  it("has HTML pages, three choices, saved for every window", async () => {
    const { fake, app } = await started(A);
    expect(app.actions.run("preferences")).toBe(true);
    const section = await vi.waitFor(() => {
      const found = [...document.querySelectorAll<HTMLElement>(".prefs-section")].find(
        (s) => s.querySelector("h4")?.textContent === "HTML pages",
      );
      if (!found) throw new Error("no HTML pages section");
      return found;
    });
    const choices = [...section.querySelectorAll<HTMLInputElement>('input[name="lx-html-pages"]')];
    expect(choices.map((c) => c.closest("label")?.textContent)).toEqual([
      "Run scripts and allow the internet",
      "Run scripts offline",
      "No page scripts",
    ]);
    expect(choices.map((c) => c.checked)).toEqual([true, false, false]);
    expect(section.textContent).toContain(
      "Pages always run in a sandbox, away from your files and from Lectern. Lectern tells you when a page loads anything from the internet.",
    );
    choices[2]?.click();
    await settle();
    expect(fake.sharedSettings().htmlPages).toBe("static");
    // Changed elsewhere, it shows here.
    fake.settingsElsewhere({ htmlPages: "offline" });
    expect(choices.map((c) => c.checked)).toEqual([false, true, false]);
  });
});
