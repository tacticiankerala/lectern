// The parent's side of the page bridge (src/page-frame.ts): what it takes from a page's frame,
// checked field by field, and what it asks of it; the notice of what a page loads; and where a
// link clicked in a page leads.
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  MAX_TEXT,
  PageFrame,
  noticeText,
  pageLinkTarget,
  pageOpen,
  parseBridgeMessage,
  type PageHost,
} from "../src/page-frame";

const READY = {
  lx: 1,
  type: "ready",
  headings: [
    { level: 1, text: "Weekly report", id: "lx-heading-1", offset: 0 },
    { level: 2, text: "Results", id: "results", offset: 14 },
  ],
  blocks: [
    { text: "Weekly report", headingLevel: 1 },
    { text: "Results", headingLevel: 2 },
    { text: "Rendered by script" },
  ],
  scroll: { fraction: 0, top: 0 },
  dark: false,
};

describe("messages from a page's bridge", () => {
  it("are taken when every field is as the bridge sends it", () => {
    expect(parseBridgeMessage(READY)).toEqual({
      type: "ready",
      headings: READY.headings,
      blocks: READY.blocks,
      scroll: { fraction: 0, top: 0 },
      dark: false,
    });
    // A title, as an older bridge sent, is no part of it.
    expect(parseBridgeMessage({ ...READY, title: "Weekly report" })).not.toHaveProperty("title");
    const href = "http://lxpage.localhost/o/tok1/other.html";
    expect(parseBridgeMessage({ lx: 1, type: "link", href })).toEqual({ type: "link", href });
    expect(
      parseBridgeMessage({ lx: 1, type: "scroll", scroll: { fraction: 0.5, top: 3 } }),
    ).toEqual({ type: "scroll", scroll: { fraction: 0.5, top: 3 } });
    expect(parseBridgeMessage({ lx: 1, type: "selection", selection: null })).toEqual({
      type: "selection",
      selection: null,
    });
    const selection = {
      quote: "Rendered by",
      start: 22,
      end: 33,
      rect: { x: 10, y: 20, width: 80, height: 18 },
    };
    expect(parseBridgeMessage({ lx: 1, type: "selection", selection })).toEqual({
      type: "selection",
      selection,
    });
    expect(parseBridgeMessage({ lx: 1, type: "scheme", dark: true })).toEqual({
      type: "scheme",
      dark: true,
    });
    expect(parseBridgeMessage({ lx: 1, type: "comment-click", offset: 26 })).toEqual({
      type: "comment-click",
      offset: 26,
    });
  });

  it("are dropped, never thrown, when anything is off", () => {
    const bad: unknown[] = [
      null,
      undefined,
      "ready",
      42,
      [],
      { ...READY, lx: 2 },
      { ...READY, type: "paint" },
      { ...READY, headings: "none" },
      { ...READY, headings: [{ level: 9, text: "x", id: "x", offset: 0 }] },
      { ...READY, headings: [{ level: 1, text: "x", id: "x", offset: -1 }] },
      { ...READY, headings: [{ level: 1, text: "x", id: "x", offset: 1.5 }] },
      { ...READY, blocks: [{ text: 3 }] },
      { ...READY, blocks: [{ text: "x", headingLevel: "1" }] },
      { ...READY, scroll: { fraction: 2, top: 0 } },
      { ...READY, scroll: { fraction: Number.NaN, top: 0 } },
      { ...READY, dark: "yes" },
      { lx: 1, type: "link", href: 5 },
      { lx: 1, type: "link", href: "x".repeat(10_000) },
      { lx: 1, type: "selection", selection: { quote: "x", start: 5, end: 2, rect: {} } },
      { lx: 1, type: "scheme" },
      { lx: 1, type: "comment-click" },
      { lx: 1, type: "comment-click", offset: -1 },
      { lx: 1, type: "comment-click", offset: 2.5 },
      { lx: 1, type: "comment-click", offset: "26" },
      { lx: 1, type: "comment-click", offset: MAX_TEXT + 1 },
    ];
    bad.forEach((data, i) => {
      expect(() => parseBridgeMessage(data)).not.toThrow();
      expect(parseBridgeMessage(data), `case ${String(i)}`).toBeNull();
    });
  });

  it("over 2 MB of text are dropped", () => {
    const big = "x".repeat(1024 * 1024);
    const blocks = [{ text: big }, { text: big }, { text: "and more" }];
    expect(parseBridgeMessage({ lx: 1, type: "blocks", headings: [], blocks })).toBeNull();
    expect(
      parseBridgeMessage({ lx: 1, type: "blocks", headings: [], blocks: [{ text: big }] }),
    ).not.toBeNull();
    expect(MAX_TEXT).toBe(2 * 1024 * 1024);
  });
});

describe("the notice of what a page loads", () => {
  it("names up to three hosts, then how many more", () => {
    expect(noticeText(["cdn.example.test"], 1, false)).toBe(
      "This page loads from cdn.example.test",
    );
    expect(noticeText(["a.example.test", "b.example.test"], 2, false)).toBe(
      "This page loads from a.example.test and b.example.test",
    );
    expect(noticeText(["a.test", "b.test", "c.test"], 3, false)).toBe(
      "This page loads from a.test, b.test and c.test",
    );
    expect(noticeText(["a.test", "b.test", "c.test", "d.test", "e.test"], 5, false)).toBe(
      "This page loads from a.test, b.test, c.test and 2 more",
    );
    expect(noticeText(["cdn.example.test"], 1, true)).toBe("Blocked loads from cdn.example.test");
  });

  it("counts the hosts past those Rust names", () => {
    const named = Array.from({ length: 50 }, (_, i) => `h${String(i)}.test`);
    expect(noticeText(named, 60, false)).toBe(
      "This page loads from h0.test, h1.test, h2.test and 57 more",
    );
  });
});

describe("a link clicked in a page", () => {
  const PAGE = "C:\\Fixtures\\vault\\artifacts\\report.html";
  const OPEN = pageOpen("http://lxpage.localhost/o/tok1/report.html", PAGE);
  const IN = "http://lxpage.localhost/o/tok1/";

  it("knows its open from its frame URL", () => {
    expect(OPEN).toEqual({
      token: "tok1",
      base: IN,
      folder: "C:\\Fixtures\\vault\\artifacts",
      separator: "\\",
    });
    // The fake's, on the dev server, beside the app.
    expect(pageOpen("/__page/o/t3-4321/report.html?lxmode=full", PAGE)).toEqual({
      token: "t3-4321",
      base: `${location.origin}/__page/o/t3-4321/`,
      folder: "C:\\Fixtures\\vault\\artifacts",
      separator: "\\",
    });
    // A path written with forward slashes.
    const slashed = pageOpen(
      "http://lxpage.localhost/o/tok1/report.html",
      "S:/Notes/My Vault/report.html",
    );
    expect(slashed).toEqual({
      token: "tok1",
      base: IN,
      folder: "S:/Notes/My Vault",
      separator: "/",
    });
    expect(pageLinkTarget(slashed, `${IN}other.html`)?.target).toBe("S:/Notes/My Vault/other.html");
    expect(pageOpen(null, PAGE)).toBeNull();
    expect(pageOpen("http://lxpage.localhost/d/c/report.html", PAGE)).toBeNull();
    expect(pageOpen("http://lxpage.localhost/o/tok1/report.html", "report.html")).toBeNull();
  });

  it("to the web or mail goes out as an external link", () => {
    expect(pageLinkTarget(OPEN, "https://example.com/a?b#c")).toEqual({
      kind: "external",
      target: "https://example.com/a?b#c",
      line: null,
      anchor: null,
    });
    expect(pageLinkTarget(OPEN, "MAILTO:someone@example.com")?.kind).toBe("external");
    expect(pageLinkTarget(null, "https://example.com/")?.kind).toBe("external");
  });

  it("within the page's open goes to the file it names in the page's folder, with its anchor", () => {
    expect(pageLinkTarget(OPEN, `${IN}other.html#part-2`)).toEqual({
      kind: "path",
      target: "C:\\Fixtures\\vault\\artifacts\\other.html",
      line: null,
      anchor: "part-2",
    });
    expect(pageLinkTarget(OPEN, `${IN}sub/r%C3%A9sum%C3%A9%20notes.md`)?.target).toBe(
      "C:\\Fixtures\\vault\\artifacts\\sub\\résumé notes.md",
    );
    expect(pageLinkTarget(OPEN, `${IN}data.json?x=1`)?.target).toBe(
      "C:\\Fixtures\\vault\\artifacts\\data.json",
    );
    // Where a `<base href="docs/">` sent it: the browser resolved it.
    expect(pageLinkTarget(OPEN, `${IN}docs/guide.html`)?.target).toBe(
      "C:\\Fixtures\\vault\\artifacts\\docs\\guide.html",
    );
  });

  it("anywhere else is ignored", () => {
    for (const href of [
      // Other URLs of the page's origin: another open, the reports, the old forms.
      "http://lxpage.localhost/o/tok2/other.html",
      "http://lxpage.localhost/o/tok12/other.html",
      "http://lxpage.localhost/__csp-report?t=tok1",
      "http://lxpage.localhost/d/c/Windows/win.ini",
      // Its open's folder itself, and what decodes to a climb, a separator or a drive.
      IN,
      `${IN}..%2Fsecret.html`,
      `${IN}%2E%2E/secret.html`,
      `${IN}a%5Cb.html`,
      `${IN}C%3A%5Ca.html`,
      `${IN}a%00b.html`,
      `${IN}%ZZ.html`,
      `${IN}a//b.html`,
      // Lectern's own and the machine's.
      "http://localhost:1420/index.html",
      "http://ipc.localhost/open",
      "http://lxasset.localhost/C%3A%5Cx.png",
      "javascript:alert(1)",
      "file:///C:/Windows/win.ini",
      "data:text/html,x",
      // Not as the browser resolves a link.
      "other.html",
      "C:\\Windows\\win.ini",
      "\\\\attacker\\share\\x.html",
      "",
      "   ",
      "#top",
    ]) {
      expect(pageLinkTarget(OPEN, href), href).toBeNull();
    }
  });
});

/** A page's frame, its window a fake that records what the session posts to it. */
function session(restore: number | null = null, anchor: string | null = null) {
  const pane = document.createElement("div");
  document.body.append(pane);
  const notice = document.createElement("div");
  notice.hidden = true;
  const frame = document.createElement("iframe");
  pane.append(notice, frame);
  const posted: Record<string, unknown>[] = [];
  const win = frame.contentWindow;
  if (!win) throw new Error("no frame window");
  vi.spyOn(win, "postMessage").mockImplementation((message: unknown) => {
    posted.push(message as Record<string, unknown>);
  });
  const host = {
    headings: vi.fn<PageHost["headings"]>(),
    text: vi.fn<PageHost["text"]>(),
    scrolled: vi.fn<PageHost["scrolled"]>(),
    follow: vi.fn<PageHost["follow"]>(),
    preferences: vi.fn<PageHost["preferences"]>(),
    selection: vi.fn<PageHost["selection"]>(),
    commentClicked: vi.fn<PageHost["commentClicked"]>(),
  };
  const page = new PageFrame(
    frame,
    notice,
    {
      path: "C:\\Fixtures\\vault\\artifacts\\report.html",
      frameUrl: "http://lxpage.localhost/o/tok1/report.html",
    },
    host,
    { fraction: restore, anchor },
  );
  return { pane, notice, frame, page, posted, host };
}

/** Whether the reader has just done something, as the browser tells it. */
function readerActed(isActive: boolean): void {
  Object.defineProperty(navigator, "userActivation", { value: { isActive }, configurable: true });
}

afterEach(() => {
  document.body.replaceChildren();
  Reflect.deleteProperty(navigator, "userActivation");
});

describe("a page's session", () => {
  it("takes the page's text, lists its headings and answers with the theme", () => {
    const { page, posted, host } = session();
    page.receive(READY);
    expect(page.text).toBe("Weekly report Results Rendered by script");
    expect(page.starts).toEqual([0, 14, 22]);
    expect(host.headings).toHaveBeenCalledWith([
      { level: 1, text: "Weekly report", id: "lx-heading-1" },
      { level: 2, text: "Results", id: "results" },
    ]);
    expect(host.text).toHaveBeenCalled();
    expect(posted[0]).toMatchObject({ lx: 1, type: "theme" });
    expect(Object.keys(posted[0]?.theme as object)).toEqual([
      "find",
      "find-current",
      "comment",
      "comment-focus",
    ]);
  });

  it("lists the outline again only when the headings change", () => {
    vi.useFakeTimers();
    try {
      const { page, host } = session();
      page.receive(READY);
      page.receive({ ...READY, type: "blocks", blocks: [...READY.blocks, { text: "More" }] });
      vi.advanceTimersByTime(500);
      expect(host.headings).toHaveBeenCalledTimes(1);
      expect(host.text).toHaveBeenCalledTimes(2);
      expect(page.text).toBe("Weekly report Results Rendered by script More");
      page.receive({ ...READY, type: "blocks", headings: READY.headings.slice(0, 1) });
      vi.advanceTimersByTime(500);
      expect(host.headings).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("scrolls to a heading's block, and to a saved place once the page is ready", () => {
    const { page, posted } = session(0.4);
    expect(page.scrollToHeading("results")).toBe(false);
    page.receive(READY);
    expect(posted).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.4 });
    expect(page.scrollToHeading("results")).toBe(true);
    expect(posted).toContainEqual({ lx: 1, type: "scroll-to", offset: 14 });
    expect(page.scrollToHeading("nowhere")).toBe(false);
  });

  it("paints and reveals for find, and keeps track of where the reader is", () => {
    const { page, posted, host } = session();
    page.receive(READY);
    page.paint("find", [{ start: 22, end: 33 }]);
    page.reveal(22);
    page.clear("find");
    expect(posted.slice(1)).toEqual([
      { lx: 1, type: "paint", layer: "find", ranges: [{ start: 22, end: 33 }] },
      { lx: 1, type: "scroll-to", offset: 22, reveal: true },
      { lx: 1, type: "clear", layer: "find" },
    ]);
    page.receive({ lx: 1, type: "scroll", scroll: { fraction: 0.75, top: 2 } });
    expect(page.fraction).toBe(0.75);
    expect(page.top).toBe(22);
    expect(host.scrolled).toHaveBeenCalled();
  });

  it("tells the heading the view's top is under, after the headings it sits under", () => {
    const { page } = session();
    expect(page.headingPath()).toEqual([]);
    page.receive({
      ...READY,
      headings: [
        ...READY.headings,
        { level: 2, text: "Costs", id: "costs", offset: 41 },
        { level: 3, text: "Travel", id: "travel", offset: 47 },
      ],
      blocks: [
        ...READY.blocks,
        { text: "Costs", headingLevel: 2 },
        { text: "Travel", headingLevel: 3 },
      ],
    });
    expect(page.headingPath()).toEqual(["Weekly report"]);
    page.receive({ lx: 1, type: "scroll", scroll: { fraction: 0.4, top: 2 } });
    expect(page.headingPath()).toEqual(["Weekly report", "Results"]);
    page.receive({ lx: 1, type: "scroll", scroll: { fraction: 0.9, top: 4 } });
    expect(page.headingPath()).toEqual(["Weekly report", "Costs", "Travel"]);
  });

  it("keeps the page's blocks, as core takes them, and its selection, and says when they change", () => {
    const { page, host } = session();
    page.receive(READY);
    expect(page.blocks).toEqual(READY.blocks);
    const selection = {
      quote: "Rendered by",
      start: 22,
      end: 33,
      rect: { x: 10, y: 20, width: 80, height: 18 },
    };
    page.receive({ lx: 1, type: "selection", selection });
    expect(page.selection).toEqual(selection);
    expect(host.selection).toHaveBeenCalledTimes(1);
    page.receive({ lx: 1, type: "selection", selection: null });
    expect(page.selection).toBeNull();
    expect(host.selection).toHaveBeenCalledTimes(2);
  });

  it("hands on a click on text a comment marks, and scrolls an offset to the top", () => {
    const { page, posted, host, frame } = session();
    page.receive(READY);
    page.receive({ lx: 1, type: "comment-click", offset: 25 });
    expect(host.commentClicked).toHaveBeenCalledWith(25);
    page.scrollTo(22);
    page.paint("comment", [{ start: 22, end: 30 }]);
    page.clear("comment-focus");
    expect(posted.slice(1)).toEqual([
      { lx: 1, type: "scroll-to", offset: 22 },
      { lx: 1, type: "paint", layer: "comment", ranges: [{ start: 22, end: 30 }] },
      { lx: 1, type: "clear", layer: "comment-focus" },
    ]);
    expect(page.frameBox()).toEqual(frame.getBoundingClientRect());
  });

  it("follows a link the reader clicked, at most one a second, and none a page sends by itself", () => {
    vi.useFakeTimers();
    try {
      const { page, host } = session();
      const link = (href: string) => {
        page.receive({ lx: 1, type: "link", href });
      };
      readerActed(true);
      link("http://lxpage.localhost/o/tok1/other.html");
      expect(host.follow).toHaveBeenCalledWith({
        kind: "path",
        target: "C:\\Fixtures\\vault\\artifacts\\other.html",
        line: null,
        anchor: null,
      });
      // 200 ms on: within the second, dropped, not kept for later.
      vi.advanceTimersByTime(200);
      link("https://example.com/again");
      expect(host.follow).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(2000);
      expect(host.follow).toHaveBeenCalledTimes(1);
      // 1100 ms after a follow: followed.
      link("https://example.com/next");
      vi.advanceTimersByTime(1100);
      link("https://example.com/later");
      expect(host.follow).toHaveBeenCalledTimes(3);
      expect(host.follow).toHaveBeenLastCalledWith({
        kind: "external",
        target: "https://example.com/later",
        line: null,
        anchor: null,
      });
      // Without the reader having just done something, none.
      readerActed(false);
      vi.advanceTimersByTime(5000);
      link("https://example.com");
      link("javascript:alert(1)");
      expect(host.follow).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("scrolls to an anchor once the page is ready, over a saved place", () => {
    const anchored = session(0.4, "results");
    anchored.page.receive(READY);
    expect(anchored.posted).toContainEqual({ lx: 1, type: "scroll-to", offset: 14 });
    expect(anchored.posted.filter((m) => "fraction" in m)).toEqual([]);
    // An anchor the page has no heading for leaves the saved place.
    const missing = session(0.4, "nowhere");
    missing.page.receive(READY);
    expect(missing.posted).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.4 });
    // Asked for after the session started, before the page is ready.
    const later = session();
    expect(later.page.scrollToAnchor("results")).toBe(true);
    later.page.receive(READY);
    expect(later.posted).toContainEqual({ lx: 1, type: "scroll-to", offset: 14 });
  });

  it("takes the page's text at most every half second, the latest of it", () => {
    vi.useFakeTimers();
    try {
      const { page, host } = session();
      page.receive(READY);
      const more = (text: string) => ({ ...READY, type: "blocks", blocks: [{ text }] });
      page.receive(more("First"));
      page.receive(more("Second"));
      expect(host.text).toHaveBeenCalledTimes(1);
      expect(page.text).toBe("Weekly report Results Rendered by script");
      vi.advanceTimersByTime(500);
      expect(host.text).toHaveBeenCalledTimes(2);
      expect(page.text).toBe("Second");
      // Gone, it takes nothing more.
      page.receive(more("Third"));
      page.dispose();
      vi.advanceTimersByTime(500);
      expect(page.text).toBe("Second");
    } finally {
      vi.useRealTimers();
    }
  });

  it("reads a page that loads itself again, and takes it back where the reader was", () => {
    const { page, host, posted } = session();
    page.receive(READY);
    page.receive({ lx: 1, type: "scroll", scroll: { fraction: 0.4, top: 1 } });
    page.receive({ ...READY, blocks: [{ text: "Refreshed" }] });
    expect(page.text).toBe("Refreshed");
    expect(host.text).toHaveBeenCalledTimes(2);
    expect(posted.filter((m) => m.type === "theme")).toHaveLength(2);
    expect(posted).toContainEqual({ lx: 1, type: "scroll-to", fraction: 0.4 });
  });

  it("scrolls to the heading the outline lists by its place, where two share an id", () => {
    const { page, posted } = session();
    page.receive({
      ...READY,
      headings: [
        { level: 2, text: "Notes", id: "notes", offset: 0 },
        { level: 2, text: "Notes", id: "notes", offset: 6 },
      ],
      blocks: [
        { text: "Notes", headingLevel: 2 },
        { text: "Notes", headingLevel: 2 },
      ],
    });
    expect(page.scrollToHeadingAt(1)).toBe(true);
    expect(posted).toContainEqual({ lx: 1, type: "scroll-to", offset: 6 });
    expect(page.scrollToHeadingAt(2)).toBe(false);
  });

  it("drops what isn't a message from the bridge", () => {
    const { page, host } = session();
    page.receive({ ...READY, blocks: [{ text: "x".repeat(3 * 1024 * 1024) }] });
    page.receive("not a message");
    expect(page.text).toBe("");
    expect(host.headings).not.toHaveBeenCalled();
  });

  it("shows what the page loads from the web, for this open only, until dismissed", () => {
    const { page, notice, host } = session();
    const loads = (token: string, hosts: string[], blocked: string[], offline = false) => {
      page.loads({
        token,
        hosts,
        count: hosts.length,
        blockedHosts: blocked,
        blockedCount: blocked.length,
        offline,
      });
    };
    loads("other", ["cdn.example.test"], []);
    expect(notice.hidden).toBe(true);
    loads("tok1", ["cdn.example.test"], []);
    expect(notice.hidden).toBe(false);
    expect(notice.textContent).toBe("This page loads from cdn.example.test×");
    expect(notice.querySelector(".page-notice-prefs")).toBeNull();
    // What was blocked, beside what loaded: Preferences can't help with it.
    loads("tok1", ["cdn.example.test"], ["plain.example.test"]);
    expect(notice.querySelector(".page-notice-text")?.textContent).toBe(
      "This page loads from cdn.example.test",
    );
    expect(notice.querySelector(".page-notice-blocked")?.textContent).toBe(
      "Blocked: plain.example.test",
    );
    expect(notice.querySelector(".page-notice-prefs")).toBeNull();
    // Served without the web: all of it blocked, and Preferences a click away.
    loads("tok1", [], ["cdn.example.test", "fonts.example.test"], true);
    expect(notice.querySelector(".page-notice-text")?.textContent).toBe(
      "Blocked loads from cdn.example.test and fonts.example.test",
    );
    expect(notice.querySelector(".page-notice-blocked")).toBeNull();
    const change = notice.querySelector<HTMLButtonElement>(".page-notice-prefs");
    expect(change?.textContent).toBe("Change in Preferences");
    change?.click();
    expect(host.preferences).toHaveBeenCalled();
    notice.querySelector<HTMLButtonElement>(".page-notice-dismiss")?.click();
    expect(notice.hidden).toBe(true);
    loads("tok1", [], ["late.example.test"], true);
    expect(notice.hidden).toBe(true);
  });
});
