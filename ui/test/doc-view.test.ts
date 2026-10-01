import { describe, expect, it, vi } from "vitest";
import { App } from "../src/app";
import { A, B, C, GatedBackend, appRoot, fixtures, rendered, settle } from "./helpers";

const HTML = `
<h1 id="title">Title</h1>
<p><a href="#Frag" data-kind="anchor" data-slug="frag" id="to-exact">exact</a>
<a href="#Missing%20Part" data-kind="anchor" data-slug="missing-part" id="to-slug">slug</a>
<a href="#fn-1" id="to-footnote">1</a>
<a href="#" class="wikilink" data-kind="doc" data-target="${B}" data-anchor="Part Two" data-slug="part-two" id="to-doc">B</a>
<a href="#" class="wikilink" data-kind="doc" data-target="${C}" id="to-doc-c">C</a>
<a href="#" class="wikilink broken" data-kind="broken" title="No note named nowhere" id="to-broken">nowhere</a>
<a href="#" class="code-link" data-kind="path" data-target="C:\\V\\src\\x.rs" data-line="12" id="to-path"><code>src/x.rs:12</code></a>
<a href="https://example.com/" id="to-raw">raw</a>
<span class="tag" data-tag="nvim">#nvim</span>
<img src="/missing.png" alt="A diagram" id="broken-img">
<img alt="Blocked logo" class="img-blocked" title="blocked">
</p>
<h2 id="Frag">Exact</h2>
<h2 id="missing-part">Slugged</h2>
<div class="code-block" data-lang="sh"><div class="code-head"><span class="code-lang">sh</span><button type="button" class="code-copy" aria-label="Copy code">Copy</button></div><pre><code>echo hi
</code></pre></div>
<ol><li id="fn-1">Note</li></ol>`;

async function started() {
  const fake = new GatedBackend(fixtures({ [A]: rendered("Aye", HTML) }), { initial: A });
  const app = new App(fake, appRoot());
  fake.startupGate.resolve();
  await app.start();
  const doc = document.getElementById("lx-doc");
  if (!doc) {
    throw new Error("no #lx-doc");
  }
  return { fake, app, doc };
}

function click(id: string): MouseEvent {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`no #${id}`);
  }
  const event = new MouseEvent("click", { bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  return event;
}

describe("DocView", () => {
  it("inserts the document and sets the window title", async () => {
    await started();
    expect(document.querySelector("#lx-doc h1")?.textContent).toBe("Title");
    expect(document.title).toBe("Aye — Lectern");
  });

  it("ignores a slow link resolution once another navigation started", async () => {
    const { fake, app } = await started();
    fake.slowFollow = true;
    const open = vi.spyOn(app, "open");
    click("to-doc");
    await app.open(C);
    fake.followGate.resolve();
    await settle();
    expect(app.state.doc?.path).toBe(C);
    expect(open.mock.calls.map(([path]) => path)).toEqual([C]);
  });

  it("follows only the latest of two clicked links", async () => {
    const { fake, app } = await started();
    fake.slowFollow = true;
    click("to-doc");
    fake.slowFollow = false;
    click("to-doc-c");
    await vi.waitFor(() => {
      expect(app.state.doc?.path).toBe(C);
    });
    fake.followGate.resolve();
    await settle();
    expect(app.state.doc?.path).toBe(C);
  });

  it("finds anchors by exact id first, then by slug, inside the document only", async () => {
    const { app, doc } = await started();
    // A chrome element outside the document with a colliding id is never the target.
    document.body.insertAdjacentHTML("afterbegin", "<div id='missing-part'>chrome</div>");
    expect(app.view.findAnchor("Frag", "frag")?.textContent).toBe("Exact");
    expect(app.view.findAnchor("Missing Part", "missing-part")?.textContent).toBe("Slugged");
    expect(doc.contains(app.view.findAnchor("missing-part"))).toBe(true);
    expect(app.view.findAnchor("nothing", "nothing")).toBeNull();
  });

  it("scrolls in-page links to their anchor, decoding the fragment", async () => {
    const { app } = await started();
    const scroll = vi.spyOn(app.view, "scrollToAnchor");
    expect(click("to-exact").defaultPrevented).toBe(true);
    expect(scroll).toHaveBeenLastCalledWith("Frag", "frag");
    click("to-slug");
    expect(scroll).toHaveBeenLastCalledWith("Missing Part", "missing-part");
    // comrak's footnote links carry no data-kind.
    expect(click("to-footnote").defaultPrevented).toBe(true);
    expect(scroll).toHaveBeenLastCalledWith("fn-1", undefined);
  });

  it("follows doc links and opens the result with its anchor", async () => {
    const { fake, app } = await started();
    const follow = vi.spyOn(fake, "follow");
    const open = vi.spyOn(app, "open");
    click("to-doc");
    expect(follow).toHaveBeenCalledWith({
      kind: "doc",
      target: B,
      line: null,
      anchor: "Part Two",
    });
    await vi.waitFor(() => {
      expect(open).toHaveBeenCalledWith(B, {
        anchor: "Part Two",
        slug: "part-two",
        push: true,
      });
    });
  });

  it("toasts a link the backend can't follow", async () => {
    const { fake } = await started();
    const follow = vi.spyOn(fake, "follow");
    click("to-path");
    expect(follow).toHaveBeenCalledWith({
      kind: "path",
      target: "C:\\V\\src\\x.rs",
      line: 12,
      anchor: null,
    });
    await vi.waitFor(() => {
      expect(document.querySelector("#lx-toasts .toast")?.textContent).toBe(
        "Couldn't find C:\\V\\src\\x.rs",
      );
    });
  });

  it("toasts a broken link without asking the backend", async () => {
    const { fake } = await started();
    const follow = vi.spyOn(fake, "follow");
    expect(click("to-broken").defaultPrevented).toBe(true);
    expect(follow).not.toHaveBeenCalled();
    expect(document.querySelector("#lx-toasts .toast")?.textContent).toBe("No note named nowhere");
  });

  it("never lets a raw link navigate the window, following it as external", async () => {
    const { fake } = await started();
    const follow = vi.spyOn(fake, "follow");
    expect(click("to-raw").defaultPrevented).toBe(true);
    expect(follow).toHaveBeenCalledWith({
      kind: "external",
      target: "https://example.com/",
      line: null,
      anchor: null,
    });
  });

  it("opens search for a clicked tag", async () => {
    const { app, doc } = await started();
    const search = vi.fn();
    app.openSearch = search;
    doc.querySelector<HTMLElement>(".tag")?.click();
    expect(search).toHaveBeenCalledWith("#nvim");
  });

  it("copies a code block and says so", async () => {
    const { doc } = await started();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const code = doc.querySelector<HTMLElement>(".code-block code");
    // jsdom has no layout, so no innerText of its own.
    if (code) {
      code.innerText = "echo hi\n";
    }
    const button = doc.querySelector<HTMLButtonElement>(".code-copy");
    button?.click();
    expect(writeText).toHaveBeenCalledWith("echo hi\n");
    await vi.waitFor(() => {
      expect(button?.textContent).toBe("Copied");
    });
  });

  it("swaps a broken or blocked image for its alt text", async () => {
    const { doc } = await started();
    const img = doc.querySelector("#broken-img");
    img?.dispatchEvent(new Event("error"));
    const placeholders = [...doc.querySelectorAll(".img-placeholder")];
    expect(placeholders.map((p) => p.textContent)).toEqual(["A diagram", "Blocked logo"]);
    expect(doc.querySelector("img")).toBeNull();
  });
});

describe("DocView anchoring", () => {
  /** Puts the heading `y` px down the document, laid out against the pane's scrollTop. */
  async function anchorable() {
    const { app, doc } = await started();
    const heading = doc.querySelector<HTMLElement>('[id="Frag"]');
    if (!heading) throw new Error("no heading");
    const pane = app.scroller;
    const layout = { y: 1000 };
    pane.scrollTop = 0;
    pane.getBoundingClientRect = () => ({ top: 0 }) as DOMRect;
    heading.getBoundingClientRect = () => ({ top: layout.y - pane.scrollTop }) as DOMRect;
    return { app, pane, layout };
  }

  function afterFrames(n: number, f: () => void): void {
    if (n === 0) {
      f();
      return;
    }
    requestAnimationFrame(() => {
      afterFrames(n - 1, f);
    });
  }

  it("keeps the target anchored while lazy layout settles", async () => {
    const { app, pane, layout } = await anchorable();
    app.view.scrollToAnchor("Frag");
    expect(layout.y - pane.scrollTop).toBe(12);
    // Two frames on, the blocks above render at their real height and push the heading down.
    afterFrames(2, () => {
      layout.y += 3567;
    });
    await settle(400);
    expect(layout.y - pane.scrollTop).toBe(12);
  });

  it("stops anchoring when the reader scrolls", async () => {
    const { app, pane, layout } = await anchorable();
    app.view.scrollToAnchor("Frag");
    pane.dispatchEvent(new WheelEvent("wheel", { bubbles: true }));
    afterFrames(2, () => {
      layout.y += 3567;
    });
    await settle(300);
    expect(pane.scrollTop).toBe(988);
  });

  it("gives up after about a second of layout that never settles", async () => {
    const { app, pane, layout } = await anchorable();
    const grow = (): void => {
      layout.y += 40;
      requestAnimationFrame(grow);
    };
    requestAnimationFrame(grow);
    app.view.scrollToAnchor("Frag");
    await settle(1400);
    const frozen = pane.scrollTop;
    await settle(200);
    expect(pane.scrollTop).toBe(frozen);
  });
});
