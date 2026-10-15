// Lectern's page bridge (crates/lectern-core/assets/page-bridge.js), loaded as Lectern serves it
// into a page's frame: here a jsdom iframe whose parent, this test's window, plays Lectern. It
// reports the page's text, headings, selection, scroll and clicked links, and paints highlights
// and scrolls when its parent, and only its parent, asks.
import { afterEach, describe, expect, it } from "vitest";
import bridge from "../../crates/lectern-core/assets/page-bridge.js?raw";

interface Message {
  lx: 1;
  type: string;
  [key: string]: unknown;
}

/** A frame's window, with what jsdom lacks and the bridge uses faked in. */
type FrameWindow = Omit<Window & typeof globalThis, "CSS" | "Highlight"> & {
  eval(code: string): unknown;
  CSS: { highlights: Map<string, FakeHighlight> };
  Highlight: typeof FakeHighlight;
};

class FakeHighlight {
  readonly ranges: Range[];
  priority = 0;
  constructor(...ranges: Range[]) {
    this.ranges = ranges;
  }
}

/** A page in a frame, the bridge running in it, and what it has said to its parent. */
interface Page {
  win: FrameWindow;
  doc: Document;
  messages: Message[];
  /** Posts `data` to the bridge as its parent does. */
  send(data: unknown, source?: unknown): void;
  /** Waits for the bridge's next message of `type`. */
  next(type: string): Promise<Message>;
  dark: { matches: boolean; listeners: (() => void)[] };
}

const frames: HTMLIFrameElement[] = [];
const listeners: ((e: MessageEvent) => void)[] = [];

afterEach(() => {
  for (const frame of frames.splice(0)) frame.remove();
  for (const listener of listeners.splice(0)) window.removeEventListener("message", listener);
});

function page(html: string): Page {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  frames.push(frame);
  const win = frame.contentWindow as unknown as FrameWindow;
  const doc = frame.contentDocument as Document;
  doc.open();
  // eslint-disable-next-line @typescript-eslint/no-deprecated -- a whole page, doctype and all
  doc.write(html);
  doc.close();
  win.CSS = { highlights: new Map() };
  win.Highlight = FakeHighlight;
  const dark = { matches: false, listeners: [] as (() => void)[] };
  win.matchMedia = ((query: string) => ({
    get matches() {
      return query === "(prefers-color-scheme: dark)" && dark.matches;
    },
    addEventListener: (_: string, cb: () => void) => {
      dark.listeners.push(cb);
    },
  })) as unknown as typeof win.matchMedia;
  const messages: Message[] = [];
  const waiting: { type: string; resolve: (m: Message) => void }[] = [];
  const listener = (e: MessageEvent): void => {
    const data = e.data as Message | undefined;
    if (data?.lx !== 1) return;
    messages.push(data);
    const i = waiting.findIndex((w) => w.type === data.type);
    if (i >= 0) waiting.splice(i, 1)[0]?.resolve(data);
  };
  window.addEventListener("message", listener);
  listeners.push(listener);
  win.eval(bridge);
  return {
    win,
    doc,
    messages,
    dark,
    // Lectern's window as the frame sees it (vitest's `window` isn't that same object).
    send(data, source = win.parent) {
      const MessageEventOf = (win as unknown as { MessageEvent: typeof MessageEvent }).MessageEvent;
      win.dispatchEvent(new MessageEventOf("message", { data, source: source as Window }));
    },
    next(type) {
      return new Promise((resolve) => {
        waiting.push({ type, resolve });
      });
    },
  };
}

/** The page's text as the bridge reports it: its blocks joined by single spaces. */
function textOf(blocks: unknown): string {
  return (blocks as { text: string }[]).map((b) => b.text).join(" ");
}

const REPORT = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Weekly report</title>
<style>.gone { display: none }</style></head>
<body>
<h1>Weekly   report</h1>
<p>An invented
   report, <b>week</b> by <i>week</i>.</p>
<h2 id="results">Results</h2>
<p>Sign-ups rose, but conversion dipped<br>in week 3.</p>
<table><tr><th>Week</th><td>3</td></tr></table>
<ul><li>First <ul><li>Nested</li></ul></li></ul>
<div>Loose text <p>in a div</p></div>
<script>var secret = "code";</script>
<noscript>No scripts</noscript>
<template><p>Template</p></template>
<svg><text>Chart label</text></svg>
<p hidden>Hidden</p>
<p class="gone">Gone</p>
<h3><span>Next</span> steps</h3>
</body></html>`;

describe("the page bridge", () => {
  it("reports the page once it has loaded: title, headings, blocks, scroll and scheme", async () => {
    const p = page(REPORT);
    const ready = await p.next("ready");
    expect(ready.title).toBe("Weekly report");
    expect(ready.blocks).toEqual([
      { text: "Weekly report", headingLevel: 1 },
      { text: "An invented report, week by week." },
      { text: "Results", headingLevel: 2 },
      { text: "Sign-ups rose, but conversion dipped in week 3." },
      { text: "Week" },
      { text: "3" },
      { text: "First" },
      { text: "Nested" },
      { text: "Loose text" },
      { text: "in a div" },
      { text: "Next steps", headingLevel: 3 },
    ]);
    const text = textOf(ready.blocks);
    // Headings, with an id each (made up where it had none), at their block's start.
    expect(ready.headings).toEqual([
      { level: 1, text: "Weekly report", id: "lx-heading-1", offset: 0 },
      { level: 2, text: "Results", id: "results", offset: text.indexOf("Results") },
      { level: 3, text: "Next steps", id: "lx-heading-2", offset: text.indexOf("Next steps") },
    ]);
    expect(p.doc.querySelector("h1")?.id).toBe("lx-heading-1");
    expect(ready.scroll).toEqual({ fraction: 0, top: 0 });
    expect(ready.dark).toBe(false);
  });

  it("lists a long heading cut short, and one with an overlong id without it", async () => {
    const long = "Quarterly ".repeat(60);
    const id = "x".repeat(300);
    const p = page(
      `<head><title>${"T".repeat(3000)}</title></head><body><h1 id="${id}">${long}</h1></body>`,
    );
    const ready = await p.next("ready");
    expect((ready.title as string).length).toBe(1000);
    const [heading] = ready.headings as { text: string; id: string }[];
    expect(heading?.text).toBe(long.trim().slice(0, 300));
    expect(heading?.id).toBe("");
    expect(p.doc.querySelector("h1")?.id).toBe(id);
  });

  it("reports the blocks again once the page has changed, after a pause", async () => {
    const p = page("<body><h1>Report</h1><p>Before.</p></body>");
    await p.next("ready");
    const blocks = p.next("blocks");
    const added = p.doc.createElement("p");
    added.textContent = "Rendered by script";
    p.doc.body.append(added);
    const message = await blocks;
    expect(textOf(message.blocks)).toBe("Report Before. Rendered by script");
    expect((message.headings as unknown[]).length).toBe(1);
    // A change that leaves the text as it was says nothing.
    added.setAttribute("data-x", "1");
    p.doc.head.append(p.doc.createElement("style"));
    await new Promise((r) => setTimeout(r, 700));
    expect(p.messages.filter((m) => m.type === "blocks")).toHaveLength(1);
  });

  it("reports at most 2 MB of text, whole blocks at a time", async () => {
    const big = "x".repeat(800 * 1024);
    const p = page(`<body><p>${big}</p><p>${big}</p><p>${big}</p></body>`);
    const ready = await p.next("ready");
    expect((ready.blocks as unknown[]).length).toBe(2);
    expect(textOf(ready.blocks).length).toBeLessThanOrEqual(2 * 1024 * 1024);
  });

  it("hands a clicked link to Lectern instead of following it; in-page anchors stay the page's", async () => {
    const p = page(`<body>
      <a id="rel" href="other.html#part">Other</a>
      <a id="web" href="https://example.com/x"><span>Web</span></a>
      <a id="anchor" href="#results">Results</a>
      <a id="script" href="javascript:void 0">Script</a>
      <a id="own" href="mine.html">Own</a>
      <h2 id="results">Results</h2></body>`);
    await p.next("ready");
    p.doc.getElementById("own")?.addEventListener("click", (e) => {
      e.preventDefault();
    });
    const click = (id: string, child?: string): boolean => {
      const target = child ? p.doc.querySelector(`#${id} ${child}`) : p.doc.getElementById(id);
      const event = new (p.win as unknown as { MouseEvent: typeof MouseEvent }).MouseEvent(
        "click",
        { bubbles: true, cancelable: true },
      );
      target?.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(click("rel")).toBe(true);
    expect(click("web", "span")).toBe(true);
    expect(click("anchor")).toBe(false);
    expect(click("script")).toBe(false);
    expect(click("own")).toBe(true);
    await new Promise((r) => setTimeout(r, 20));
    expect(p.messages.filter((m) => m.type === "link").map((m) => m.href)).toEqual([
      "other.html#part",
      "https://example.com/x",
    ]);
  });

  it("reports the selection as a quote with its offsets into the text, and its clearing", async () => {
    const p = page("<body><h1>Report</h1><p>Conversion <b>dipped</b> in week 3.</p></body>");
    const ready = await p.next("ready");
    const text = textOf(ready.blocks);
    const range = p.doc.createRange();
    const para = p.doc.querySelector("p");
    if (!para?.firstChild || !para.lastChild) throw new Error("no paragraph");
    range.setStart(para.firstChild, 0);
    range.setEnd(para.lastChild, 3);
    const selection = p.win.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    const selected = p.next("selection");
    p.doc.dispatchEvent(new Event("selectionchange"));
    const message = (await selected).selection as Record<string, unknown>;
    expect(message.quote).toBe("Conversion dipped in");
    expect(text.slice(message.start as number, message.end as number)).toBe("Conversion dipped in");
    expect(message.rect).toEqual({ x: 0, y: 0, width: 0, height: 0 });
    // A whole paragraph, by its element's boundaries, as a triple click selects it.
    const whole = p.doc.createRange();
    whole.selectNodeContents(para);
    selection?.removeAllRanges();
    selection?.addRange(whole);
    const all = p.next("selection");
    p.doc.dispatchEvent(new Event("selectionchange"));
    expect(((await all).selection as Record<string, unknown>).quote).toBe(
      "Conversion dipped in week 3.",
    );
    const cleared = p.next("selection");
    selection?.removeAllRanges();
    p.doc.dispatchEvent(new Event("selectionchange"));
    expect((await cleared).selection).toBeNull();
  });

  it("places the selection by the box of its last line", async () => {
    const p = page("<body><p>Sign-ups rose, but conversion dipped in week 3.</p></body>");
    await p.next("ready");
    const box = (x: number, y: number, width: number, height: number) =>
      ({ x, y, left: x, top: y, width, height, right: x + width, bottom: y + height }) as DOMRect;
    const RangeOf = (p.win as unknown as { Range: typeof Range }).Range;
    RangeOf.prototype.getClientRects = () =>
      [box(40, 100, 300, 18), box(10, 118, 0, 0), box(10, 118, 120, 18)] as unknown as DOMRectList;
    RangeOf.prototype.getBoundingClientRect = () => box(10, 100, 330, 36);
    const para = p.doc.querySelector("p");
    if (!para?.firstChild) throw new Error("no paragraph");
    const range = p.doc.createRange();
    range.setStart(para.firstChild, 19);
    range.setEnd(para.firstChild, 47);
    p.win.getSelection()?.addRange(range);
    const selected = p.next("selection");
    p.doc.dispatchEvent(new Event("selectionchange"));
    const message = (await selected).selection as Record<string, unknown>;
    expect(message.quote).toBe("conversion dipped in week 3.");
    expect(message.rect).toEqual({ x: 10, y: 118, width: 120, height: 18 });
  });

  it("reports a plain click on text a comment marks, with where it fell", async () => {
    const p = page(
      '<body><h1>Report</h1><p>Sign-ups rose, but conversion dipped in week 3, <a href="other.html">see why</a>.</p></body>',
    );
    const ready = await p.next("ready");
    const text = textOf(ready.blocks);
    const para = p.doc.querySelector("p");
    const link = p.doc.querySelector("a");
    if (!para?.firstChild || !link?.firstChild) throw new Error("no paragraph");
    const start = text.indexOf("conversion dipped");
    // Where a click lands: here, a caret in the paragraph's text, `at` characters in.
    let caret: { offsetNode: Node; offset: number } = { offsetNode: para.firstChild, offset: 0 };
    (p.doc as unknown as { caretPositionFromPoint: () => unknown }).caretPositionFromPoint = () =>
      caret;
    // A click, and a moment for what it posts to arrive.
    const click = async (target: Element, at: number, node = para.firstChild): Promise<void> => {
      if (!node) throw new Error("no node");
      caret = { offsetNode: node, offset: at };
      const MouseEventOf = (p.win as unknown as { MouseEvent: typeof MouseEvent }).MouseEvent;
      target.dispatchEvent(
        new MouseEventOf("click", { bubbles: true, cancelable: true, button: 0, clientX: 5 }),
      );
      await new Promise((r) => setTimeout(r, 20));
    };
    const clicks = () => p.messages.filter((m) => m.type === "comment-click");
    // Nothing marked yet.
    await click(para, 25);
    expect(clicks()).toEqual([]);
    p.send({
      lx: 1,
      type: "paint",
      layer: "comment",
      ranges: [{ start, end: start + "conversion dipped".length }],
    });
    await click(para, 25);
    expect(clicks()).toEqual([
      { lx: 1, type: "comment-click", offset: text.indexOf("sion dipped") },
    ]);
    // Its last character, from its right half.
    await click(para, 36);
    expect(clicks().at(-1)?.offset).toBe(start + "conversion dipped".length);
    // Outside the marks, on a link, or ending a selection: nothing.
    await click(para, 5);
    await click(link, 2, link.firstChild);
    const range = p.doc.createRange();
    range.setStart(para.firstChild, 19);
    range.setEnd(para.firstChild, 30);
    p.win.getSelection()?.addRange(range);
    await click(para, 25);
    expect(clicks()).toHaveLength(2);
  });

  it("paints ranges of the text by layer, and clears them", async () => {
    const p = page(
      "<body><h1>Report</h1><p>Rendered by   <b>script</b>.</p><p>And rendered again.</p></body>",
    );
    const ready = await p.next("ready");
    const text = textOf(ready.blocks).toLowerCase();
    const first = text.indexOf("rendered by script");
    const second = text.indexOf("rendered again");
    p.send({
      lx: 1,
      type: "paint",
      layer: "find",
      ranges: [
        { start: first, end: first + "rendered by script".length },
        { start: second, end: second + "rendered again".length },
      ],
    });
    const found = p.win.CSS.highlights.get("lx-find");
    expect(found?.ranges.map((r) => r.toString())).toEqual([
      "Rendered by   script",
      "rendered again",
    ]);
    expect(found?.priority).toBe(2);
    p.send({
      lx: 1,
      type: "paint",
      layer: "find-current",
      ranges: [{ start: second, end: second + 8 }],
    });
    expect(p.win.CSS.highlights.get("lx-find-current")?.priority).toBe(3);
    // A range across two blocks covers both.
    const and = text.indexOf("and rendered");
    p.send({ lx: 1, type: "paint", layer: "comment", ranges: [{ start: first, end: and + 3 }] });
    expect(p.win.CSS.highlights.get("lx-comment")?.ranges[0]?.toString()).toBe(
      "Rendered by   script.And",
    );
    p.send({ lx: 1, type: "clear", layer: "find" });
    expect(p.win.CSS.highlights.has("lx-find")).toBe(false);
    expect(p.win.CSS.highlights.has("lx-find-current")).toBe(true);
  });

  it("paints in Lectern's colours", async () => {
    const p = page("<body><p>Text</p></body>");
    await p.next("ready");
    p.send({
      lx: 1,
      type: "theme",
      theme: {
        find: { background: "#e8d48a" },
        "find-current": { background: "#9a5b2e", color: "#f8f5ee" },
        comment: { background: "rgb(46 139 120 / 0.2)" },
        "comment-focus": { background: "red; } body { display: none" },
      },
    });
    const styles = [...p.doc.querySelectorAll("style")].map((s) => s.textContent).join("\n");
    expect(styles).toContain("::highlight(lx-find) { background-color: #e8d48a; }");
    expect(styles).toContain(
      "::highlight(lx-find-current) { background-color: #9a5b2e; color: #f8f5ee; }",
    );
    expect(styles).toContain(
      "::highlight(lx-comment) { background-color: rgb(46 139 120 / 0.2); }",
    );
    // Nothing that isn't a colour gets in.
    expect(styles).not.toContain("display: none");
  });

  it("scrolls to a fraction, or to an offset in the text", async () => {
    const p = page("<body><h1>Report</h1><p>Results here.</p></body>");
    await p.next("ready");
    const scroller = p.doc.scrollingElement ?? p.doc.documentElement;
    let top = 0;
    Object.defineProperties(scroller, {
      scrollHeight: { value: 3000, configurable: true },
      clientHeight: { value: 1000, configurable: true },
      scrollTop: {
        configurable: true,
        get: () => top,
        set: (v: number) => {
          top = v;
        },
      },
    });
    p.send({ lx: 1, type: "scroll-to", fraction: 0.25 });
    expect(top).toBe(500);
    // The text at an offset goes to the top, or, to reveal it, to the middle when out of sight.
    const RangeOf = (p.win as unknown as { Range: typeof Range }).Range;
    let at = { top: 600, bottom: 620, height: 20 };
    // jsdom lays nothing out.
    RangeOf.prototype.getBoundingClientRect = () =>
      ({ ...at, left: 0, right: 10, width: 10, x: 0, y: at.top }) as DOMRect;
    p.send({ lx: 1, type: "scroll-to", offset: 7 });
    expect(top).toBe(500 + 600 - 12);
    top = 0;
    p.send({ lx: 1, type: "scroll-to", offset: 7, reveal: true });
    expect(top).toBe(0);
    at = { top: 1500, bottom: 1520, height: 20 };
    p.send({ lx: 1, type: "scroll-to", offset: 7, reveal: true });
    expect(top).toBe(1500 - (p.win.innerHeight - 20) / 2);
  });

  it("keeps a restored place while the page grows, until the reader scrolls", async () => {
    const p = page("<body><h1>Report</h1><img src='chart.png'><p>Text.</p></body>");
    await p.next("ready");
    const scroller = p.doc.scrollingElement ?? p.doc.documentElement;
    let top = 0;
    let height = 2000;
    Object.defineProperties(scroller, {
      scrollHeight: { get: () => height, configurable: true },
      clientHeight: { value: 1000, configurable: true },
      scrollTop: {
        configurable: true,
        get: () => top,
        set: (v: number) => {
          top = v;
        },
      },
    });
    p.send({ lx: 1, type: "scroll-to", fraction: 0.5 });
    expect(top).toBe(500);
    // An image loads and the page grows: the place is put back.
    height = 4000;
    p.doc.querySelector("img")?.dispatchEvent(new Event("load"));
    expect(top).toBe(1500);
    // Once the reader scrolls, the page is theirs.
    p.win.dispatchEvent(new Event("wheel"));
    height = 6000;
    p.doc.querySelector("img")?.dispatchEvent(new Event("load"));
    expect(top).toBe(1500);
  });

  it("repaints its highlights when the page rebuilds nodes but keeps its text", async () => {
    const p = page("<body><h1>Report</h1><p id='p'>Rendered by script.</p></body>");
    const ready = await p.next("ready");
    const start = textOf(ready.blocks).indexOf("Rendered");
    p.send({ lx: 1, type: "paint", layer: "find", ranges: [{ start, end: start + 8 }] });
    const old = p.doc.getElementById("p");
    if (!old) throw new Error("no paragraph");
    old.replaceWith(old.cloneNode(true));
    await new Promise((r) => setTimeout(r, 700));
    const range = p.win.CSS.highlights.get("lx-find")?.ranges[0];
    expect(range?.toString()).toBe("Rendered");
    expect(range?.startContainer.isConnected).toBe(true);
    // The text is the same, so nothing is reported.
    expect(p.messages.filter((m) => m.type === "blocks")).toHaveLength(0);
  });

  it("reports the text of a page that never stops changing, every 2 seconds at least", async () => {
    const p = page("<body><p id='clock'>0</p></body>");
    await p.next("ready");
    const clock = p.doc.getElementById("clock");
    let n = 0;
    const ticking = setInterval(() => {
      n += 1;
      if (clock) clock.textContent = String(n);
    }, 100);
    try {
      const started = Date.now();
      const blocks = await p.next("blocks");
      expect(Date.now() - started).toBeLessThan(2600);
      expect(textOf(blocks.blocks)).not.toBe("0");
    } finally {
      clearInterval(ticking);
    }
  });

  it("maps text to the page once per reading of it, not once per range it paints", async () => {
    // 100 KB of words in one text node, and three thousand of them to paint.
    const p = page(`<body><pre>${"word ".repeat(20_000)}</pre></body>`);
    await p.next("ready");
    // Every read of a text node's characters, which mapping an offset to the page would need.
    const proto = (p.win as unknown as { CharacterData: typeof CharacterData }).CharacterData
      .prototype;
    const data = Object.getOwnPropertyDescriptor(proto, "data");
    if (!data) throw new Error("no data property");
    let reads = 0;
    Object.defineProperty(proto, "data", {
      ...data,
      get(this: CharacterData): string {
        reads += 1;
        // The same characters, by `Node`'s own getter.
        return this.nodeValue ?? "";
      },
    });
    const ranges = Array.from({ length: 3000 }, (_, i) => ({ start: i * 5, end: i * 5 + 4 }));
    p.send({ lx: 1, type: "paint", layer: "find", ranges });
    expect(p.win.CSS.highlights.get("lx-find")?.ranges).toHaveLength(3000);
    expect(p.win.CSS.highlights.get("lx-find")?.ranges[2999]?.toString()).toBe("word");
    expect(reads).toBeLessThan(10);
  });

  it("opens the closed <details> around a match it reveals, not one whose summary holds it", async () => {
    const p = page(`<body><p>Top</p>
      <details id="outer"><summary>More</summary>
        <details id="inner"><summary>Deeper</summary><p>Hidden match</p></details>
      </details>
      <details id="titled"><summary>Summary match</summary><p>Rest</p></details></body>`);
    const ready = await p.next("ready");
    const text = textOf(ready.blocks);
    const open = (id: string) => (p.doc.getElementById(id) as HTMLDetailsElement).open;
    p.send({ lx: 1, type: "scroll-to", offset: text.indexOf("Hidden match"), reveal: true });
    expect([open("outer"), open("inner")]).toEqual([true, true]);
    p.send({ lx: 1, type: "scroll-to", offset: text.indexOf("Summary match"), reveal: true });
    expect(open("titled")).toBe(false);
  });

  it("reports the blocks again when they split or join, though the text reads the same", async () => {
    const p = page("<body><div id='box'><p>alpha beta</p></div></body>");
    const ready = await p.next("ready");
    expect(ready.blocks).toEqual([{ text: "alpha beta" }]);
    const blocks = p.next("blocks");
    const box = p.doc.getElementById("box");
    if (box) box.innerHTML = "<p>alpha</p><p>beta</p>";
    expect((await blocks).blocks).toEqual([{ text: "alpha" }, { text: "beta" }]);
  });

  it("reports the colour scheme as it changes", async () => {
    const p = page("<body><p>Text</p></body>");
    await p.next("ready");
    p.dark.matches = true;
    const scheme = p.next("scheme");
    for (const cb of p.dark.listeners) cb();
    expect((await scheme).dark).toBe(true);
  });

  it("listens to its parent alone, and drops what it can't read", async () => {
    const p = page("<body><p>Text to paint</p></body>");
    await p.next("ready");
    const paint = { lx: 1, type: "paint", layer: "find", ranges: [{ start: 0, end: 4 }] };
    // From another window, or from the page itself.
    p.send(paint, p.win);
    p.send(paint, null);
    expect(p.win.CSS.highlights.has("lx-find")).toBe(false);
    for (const bad of [
      null,
      "paint",
      { type: "paint", layer: "find", ranges: [{ start: 0, end: 4 }] },
      { lx: 1, type: "paint", layer: "page", ranges: [{ start: 0, end: 4 }] },
      { lx: 1, type: "paint", layer: "find", ranges: "0-4" },
      { lx: 1, type: "paint", layer: "find", ranges: [{ start: 4, end: 0 }] },
      { lx: 1, type: "paint", layer: "find", ranges: [{ start: 0, end: 9999 }] },
      { lx: 1, type: "scroll-to", fraction: "half" },
      { lx: 1, type: "theme", theme: null },
      { lx: 1, type: "unknown" },
    ]) {
      expect(() => {
        p.send(bad);
      }).not.toThrow();
    }
    expect(p.win.CSS.highlights.has("lx-find")).toBe(false);
    // Its parent is heard.
    p.send(paint);
    expect(p.win.CSS.highlights.get("lx-find")?.ranges[0]?.toString()).toBe("Text");
  });
});
