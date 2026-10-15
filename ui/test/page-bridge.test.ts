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
  /** Ends the page's parsing (`loading`): the browser's own DOMContentLoaded and load follow. */
  finish(): void;
}

const frames: HTMLIFrameElement[] = [];
const listeners: ((e: MessageEvent) => void)[] = [];
/** The open's key, as Lectern serves it into the bridge's slot for it. */
const KEY = "k-test";

afterEach(() => {
  for (const frame of frames.splice(0)) frame.remove();
  for (const listener of listeners.splice(0)) window.removeEventListener("message", listener);
});

/**
 * `html` in a frame with the bridge running, after `before` has set the frame up. With `loading`,
 * the bridge runs while the page is still being parsed, as Lectern serves it, and the page's
 * DOMContentLoaded and load come once `finish` is called.
 */
function page(
  html: string,
  before?: (win: FrameWindow, doc: Document) => void,
  loading = false,
): Page {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  frames.push(frame);
  const win = frame.contentWindow as unknown as FrameWindow;
  const doc = frame.contentDocument as Document;
  doc.open();
  // eslint-disable-next-line @typescript-eslint/no-deprecated -- a whole page, doctype and all
  doc.write(html);
  if (loading) {
    // jsdom has it complete already; the browser has it loading until the page is parsed.
    Object.defineProperty(doc, "readyState", { value: "loading", configurable: true });
  } else {
    doc.close();
  }
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
  before?.(win, doc);
  win.eval(bridge.replace('"LECTERN_BRIDGE_KEY"', JSON.stringify(KEY)));
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
    finish() {
      Reflect.deleteProperty(doc, "readyState");
      doc.close();
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
  it("reports the page once it has loaded: headings, blocks, scroll and scheme", async () => {
    const p = page(REPORT);
    const ready = await p.next("ready");
    expect(ready).not.toHaveProperty("title");
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

  it("signs every message with its open's key, which no page script sees", async () => {
    const stolen: unknown[] = [];
    const script = { removed: false };
    const p = page("<body><h1>Report</h1><p>Text.</p></body>", (win, doc) => {
      // The bridge's own element, which it takes out of the page before any page script runs.
      const element = doc.createElement("script");
      doc.head.append(element);
      element.remove = () => {
        script.removed = true;
        Element.prototype.remove.call(element);
      };
      Object.defineProperty(doc, "currentScript", { value: element, configurable: true });
      // A page that would catch the key as it is set on a message.
      const ObjectOf = (win as unknown as { Object: typeof Object }).Object;
      ObjectOf.defineProperty(ObjectOf.prototype, "key", {
        set(value: unknown) {
          stolen.push(value);
        },
        configurable: true,
      });
    });
    const ready = await p.next("ready");
    expect(ready.key).toBe(KEY);
    expect(script.removed).toBe(true);
    expect(p.doc.querySelector("script")).toBeNull();
    p.doc.dispatchEvent(new Event("scroll"));
    p.win.dispatchEvent(new Event("scroll"));
    await p.next("scroll");
    expect(p.messages.every((m) => m.key === KEY)).toBe(true);
    expect(stolen).toEqual([]);
  });

  it("says it is ready without an animation frame, as in a window in the background", async () => {
    const p = page("<body><p>Text.</p></body>", (win) => {
      win.requestAnimationFrame = () => 0;
    });
    const ready = await p.next("ready");
    expect(ready.blocks).toEqual([{ text: "Text." }]);
    // Loaded already, it said so too, as it started.
    expect(p.messages.filter((m) => m.type === "loaded")).toEqual([
      { lx: 1, key: KEY, type: "loaded" },
    ]);
  });

  it("says its page has loaded, signed, once its window's load comes, whatever the page does", async () => {
    const p = page("<body><p>Text.</p></body>", undefined, true);
    // A page script, after the bridge as every page script is, that stops the load event going
    // any further, as soon as it can.
    p.win.addEventListener(
      "load",
      (e) => {
        e.stopImmediatePropagation();
      },
      true,
    );
    const ready = p.next("ready");
    const loaded = p.next("loaded");
    p.finish();
    await ready;
    expect(await loaded).toEqual({ lx: 1, key: KEY, type: "loaded" });
  });

  it("answers the browser's own DOMContentLoaded and load once, never ones a page fakes", async () => {
    const p = page("<body><p>Text.</p></body>", undefined, true);
    const ready = p.next("ready");
    const loaded = p.next("loaded");
    p.finish();
    await ready;
    await loaded;
    // A page faking them again, to have the bridge say so again, then leave.
    const EventOf = (p.win as unknown as { Event: typeof Event }).Event;
    p.doc.dispatchEvent(new EventOf("DOMContentLoaded"));
    p.win.dispatchEvent(new EventOf("load"));
    await new Promise((r) => setTimeout(r, 250));
    expect(p.messages.filter((m) => m.type === "ready")).toHaveLength(1);
    expect(p.messages.filter((m) => m.type === "loaded")).toHaveLength(1);
    // Its text changing still goes out, as blocks.
    const blocks = p.next("blocks");
    p.doc.body.append(p.doc.createTextNode(" More."));
    expect(textOf((await blocks).blocks)).toBe("Text. More.");
  });

  it("takes no faked DOMContentLoaded or load for the browser's own", async () => {
    const p = page("<body><p>Text.</p></body>", undefined, true);
    const EventOf = (p.win as unknown as { Event: typeof Event }).Event;
    p.doc.dispatchEvent(new EventOf("DOMContentLoaded"));
    p.win.dispatchEvent(new EventOf("load"));
    await new Promise((r) => setTimeout(r, 250));
    expect(p.messages.filter((m) => m.type === "ready" || m.type === "loaded")).toEqual([]);
    const ready = p.next("ready");
    p.finish();
    await ready;
  });

  it("lists a long heading cut short, and one with an overlong id without it", async () => {
    const long = "Quarterly ".repeat(60);
    const id = "x".repeat(300);
    const p = page(
      `<head><title>${"T".repeat(3000)}</title></head><body><h1 id="${id}">${long}</h1></body>`,
    );
    const ready = await p.next("ready");
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

  it("hands a clicked link to Lectern as the browser resolves it; in-page anchors stay the page's", async () => {
    const p = page(`<head><base href="http://lxpage.localhost/o/tok1/docs/"></head><body>
      <a id="rel" href="other.html#part">Other</a>
      <a id="up" href="../report.html">Up</a>
      <a id="web" href="https://example.com/x"><span>Web</span></a>
      <a id="anchor" href="#results">Results</a>
      <a id="script" href="javascript:void 0">Script</a>
      <a id="own" href="mine.html">Own</a>
      <a id="kept" href="kept.html">Kept</a>
      <a id="bare">No href</a>
      <svg><a id="chart" xlink:href="chart.html"><text>Chart</text></a></svg>
      <h2 id="results">Results</h2></body>`);
    await p.next("ready");
    // The page's own handlers come after the bridge's: one that takes the click, or stops it
    // going any further, hides nothing from Lectern.
    p.doc.getElementById("own")?.addEventListener("click", (e) => {
      e.preventDefault();
    });
    p.doc.getElementById("kept")?.addEventListener("click", (e) => {
      e.stopPropagation();
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
    expect(click("up")).toBe(true);
    expect(click("web", "span")).toBe(true);
    expect(click("anchor")).toBe(false);
    expect(click("script")).toBe(false);
    expect(click("own")).toBe(true);
    expect(click("kept")).toBe(true);
    expect(click("bare")).toBe(false);
    expect(click("chart", "text")).toBe(true);
    await new Promise((r) => setTimeout(r, 20));
    expect(p.messages.filter((m) => m.type === "link").map((m) => m.href)).toEqual([
      "http://lxpage.localhost/o/tok1/docs/other.html#part",
      "http://lxpage.localhost/o/tok1/report.html",
      "https://example.com/x",
      "http://lxpage.localhost/o/tok1/docs/mine.html",
      "http://lxpage.localhost/o/tok1/docs/kept.html",
      "http://lxpage.localhost/o/tok1/docs/chart.html",
    ]);
  });

  it("holds the selection while a pointer drags it, and reports it once the pointer is up", async () => {
    const p = page("<body><p>Conversion dipped in week 3.</p></body>");
    await p.next("ready");
    const para = p.doc.querySelector("p");
    if (!para?.firstChild) throw new Error("no paragraph");
    const Pointer = (p.win as unknown as { Event: typeof Event }).Event;
    const pointer = (type: string): void => {
      p.win.dispatchEvent(new Pointer(type, { bubbles: true }));
    };
    pointer("pointerdown");
    const range = p.doc.createRange();
    for (const end of [4, 10, 17]) {
      range.setStart(para.firstChild, 0);
      range.setEnd(para.firstChild, end);
      p.win.getSelection()?.removeAllRanges();
      p.win.getSelection()?.addRange(range);
      p.doc.dispatchEvent(new Event("selectionchange"));
    }
    await new Promise((r) => setTimeout(r, 250));
    expect(p.messages.filter((m) => m.type === "selection")).toEqual([]);
    const selected = p.next("selection");
    pointer("pointerup");
    expect(((await selected).selection as Record<string, unknown>).quote).toBe("Conversion dipped");
    await new Promise((r) => setTimeout(r, 250));
    expect(p.messages.filter((m) => m.type === "selection")).toHaveLength(1);
  });

  it("lets a held selection go when the window loses the focus, or after 3 seconds", async () => {
    const p = page("<body><p>Conversion dipped in week 3.</p></body>");
    await p.next("ready");
    const para = p.doc.querySelector("p");
    if (!para?.firstChild) throw new Error("no paragraph");
    const Pointer = (p.win as unknown as { Event: typeof Event }).Event;
    const select = (end: number): void => {
      const range = p.doc.createRange();
      range.setStart(para.firstChild as Node, 0);
      range.setEnd(para.firstChild as Node, end);
      p.win.getSelection()?.removeAllRanges();
      p.win.getSelection()?.addRange(range);
      p.doc.dispatchEvent(new Event("selectionchange"));
    };
    p.win.dispatchEvent(new Pointer("pointerdown", { bubbles: true }));
    select(10);
    const blurred = p.next("selection");
    p.win.dispatchEvent(new Pointer("blur"));
    expect(((await blurred).selection as Record<string, unknown>).quote).toBe("Conversion");
    // A pointer that never comes up: let go after 3 seconds.
    p.win.dispatchEvent(new Pointer("pointerdown", { bubbles: true }));
    select(17);
    await new Promise((r) => setTimeout(r, 1000));
    expect(p.messages.filter((m) => m.type === "selection")).toHaveLength(1);
    const late = p.next("selection");
    expect(((await late).selection as Record<string, unknown>).quote).toBe("Conversion dipped");
  }, 10_000);

  it("makes up a heading's id by its place among those without, so a rebuild changes nothing", async () => {
    const markup = "<h2>Alpha</h2><p>One.</p><h2 id='beta'>Beta</h2><h2>Gamma</h2>";
    const p = page(`<body><main id="app">${markup}</main><p id="lx-heading-2">Taken</p></body>`);
    const ready = await p.next("ready");
    const ids = (headings: unknown) => (headings as { id: string }[]).map((h) => h.id);
    // The page's own `lx-heading-2` is skipped.
    expect(ids(ready.headings)).toEqual(["lx-heading-1", "beta", "lx-heading-3"]);
    // The page renders its headings again, as new elements without ids.
    const app = p.doc.getElementById("app");
    if (app) app.innerHTML = markup;
    await new Promise((r) => setTimeout(r, 700));
    expect(p.messages.filter((m) => m.type === "blocks")).toEqual([]);
    expect([...p.doc.querySelectorAll("#app h2")].map((h) => h.id)).toEqual([
      "lx-heading-1",
      "beta",
      "lx-heading-3",
    ]);
    // An id the page gives a heading later is its own, kept.
    const first = p.doc.querySelector("#app h2");
    if (first) first.id = "alpha";
    const blocks = p.next("blocks");
    first?.append(" again");
    expect(ids((await blocks).headings)).toEqual(["alpha", "beta", "lx-heading-1"]);
    expect(first?.id).toBe("alpha");
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
      { lx: 1, key: KEY, type: "comment-click", offset: text.indexOf("sion dipped") },
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
    // jsdom has no `scrollIntoView`, and lays nothing out: the block stays where it is.
    const ElementOf = (p.win as unknown as { Element: typeof Element }).Element;
    ElementOf.prototype.scrollIntoView = () => undefined;
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

  it("reveals a match inside a scrolling box by scrolling every box around it", async () => {
    const p = page(`<body><p>Top</p>
      <div id="box" style="overflow-y: auto"><p>Filler</p><p id="deep">Deep match</p></div></body>`);
    const ready = await p.next("ready");
    const text = textOf(ready.blocks);
    const scroller = p.doc.scrollingElement ?? p.doc.documentElement;
    const before = scroller.scrollTop;
    const box = p.doc.getElementById("box");
    if (!box) throw new Error("no box");
    const rect = (top: number, bottom: number) =>
      ({
        top,
        bottom,
        left: 0,
        right: 100,
        width: 100,
        height: bottom - top,
        x: 0,
        y: top,
      }) as DOMRect;
    // The box shows 100 to 400 of the window; the match is in the window, but scrolled out of
    // the box, below it.
    box.getBoundingClientRect = () => rect(100, 400);
    let at = rect(500, 520);
    const RangeOf = (p.win as unknown as { Range: typeof Range }).Range;
    RangeOf.prototype.getBoundingClientRect = () => at;
    const scrolled: [Element, unknown][] = [];
    const ElementOf = (p.win as unknown as { Element: typeof Element }).Element;
    ElementOf.prototype.scrollIntoView = function (this: Element, options?: unknown) {
      scrolled.push([this, options]);
      at = rect(240, 260);
    };
    p.send({ lx: 1, type: "scroll-to", offset: text.indexOf("Deep match"), reveal: true });
    expect(scrolled).toEqual([
      [p.doc.getElementById("deep"), { block: "center", inline: "nearest" }],
    ]);
    expect(scroller.scrollTop).toBe(before);
    // In sight in the box and the window: left where it is.
    p.send({ lx: 1, type: "scroll-to", offset: text.indexOf("Deep match"), reveal: true });
    expect(scrolled).toHaveLength(1);
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
