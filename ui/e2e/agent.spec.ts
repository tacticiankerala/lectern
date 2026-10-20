// An agent's requests (src/agent.ts), through the fake backend: `status` names the heading the
// outline marks as being read, which a navigation lands a little below the pane's top; `show` asks
// with a bar over the document, and an agent's switch keeps a half-written comment. A target lands
// at the top, with Back to where the reader was, and a line target inside a long paragraph or code
// block lands that line, not the block's top; one folded in a closed details opens it, and a line
// in a note's frontmatter lands at the document's top; an agent's highlights paint on their own
// layer in a note, and through its bridge in an HTML page, whose text `read` takes from the page.
// An agent's comment on a page on screen is its own card, painted in the page, and `show` at a
// thread opens the Comments tab on it.
import { expect, test, type Page } from "@playwright/test";
import type { Backend } from "../src/backend";
import {
  expectLandedOn,
  fixturePath,
  headingOffset,
  LANDING_PX,
  LANDING_SLACK_PX,
  openFixture,
} from "./util";

const BIG_PLAN = "work/alpha/plans/2026-01-01-big-plan.md";

test("status names the heading navigated to, as the outline marks it", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const item = page.locator("#lx-outline li", { hasText: /^Section 12$/ });
  await item.locator("a").click();
  await expectLandedOn(page, "section-12");
  await expect(item).toHaveClass(/active/);
  const reply = await page.evaluate(() => window.__fake.agentRequest({ tool: "status" }));
  // The line is the landed heading's own, read from its source position.
  const line = await page.evaluate(() => {
    const heading = document.querySelector('#lx-doc [id="section-12"]');
    return Number(/^(\d+):/.exec(heading?.getAttribute("data-sourcepos") ?? "")?.[1]);
  });
  expect(line).toBeGreaterThan(1);
  expect(reply).toMatchObject({
    kind: "view",
    path: fixturePath(BIG_PLAN),
    heading: ["Big Plan", "Section 12"],
    line,
  });
});

const ALPHA = "work/alpha/README.md";

/** The bar over the document that asks the reader about an agent's request. */
const bar = (page: Page) => page.locator("#lx-agent-ask");

test("show asks with a bar over the document, which keeps its place; Show opens it, Back returns", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const top = await headingOffset(page, "alpha");
  const asked = await page.evaluate(
    (path) =>
      window.__fake.agentRequest({
        tool: "show",
        path,
        target: { heading: "Big Plan" },
        newWindow: false,
        wait: 0,
      }),
    fixturePath(BIG_PLAN),
  );
  expect(asked).toEqual({ kind: "asked" });
  await expect(bar(page)).toBeVisible();
  await expect(bar(page)).toContainText("Claude wants to show 2026-01-01-big-plan.md › Big Plan");
  await expect(bar(page).locator(".agent-ask-file")).toHaveAttribute(
    "title",
    fixturePath(BIG_PLAN),
  );
  // Over the top of the document pane, which it doesn't move.
  const [barBox, paneBox] = await Promise.all([
    bar(page).boundingBox(),
    page.locator("#lx-doc-pane").boundingBox(),
  ]);
  expect(barBox && paneBox && barBox.y - paneBox.y).toBeGreaterThanOrEqual(0);
  expect(barBox && paneBox && barBox.y - paneBox.y).toBeLessThan(40);
  expect(await headingOffset(page, "alpha")).toBe(top);
  await expect(page).toHaveTitle("Alpha — Lectern");

  await bar(page).getByRole("button", { name: "Show" }).click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expect(bar(page)).toBeHidden();
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Alpha — Lectern");
});

test("a half-written comment survives an agent's switch to another note", async ({ page }) => {
  await openFixture(page, ALPHA);
  await page.keyboard.press("Control+Alt+KeyM");
  const editor = page.locator("#lx-doc-pane .comment-editor");
  await expect(editor).toBeVisible();
  await page.keyboard.type("Half a thought");
  await page.evaluate(
    (path) => window.__fake.agentRequest({ tool: "show", path, newWindow: false, wait: 0 }),
    fixturePath(BIG_PLAN),
  );
  await bar(page).getByRole("button", { name: "Show" }).click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expect(editor).toBeHidden();
  // Back on its note, it's open again with the text.
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Alpha — Lectern");
  await expect(editor).toBeVisible();
  await expect(editor.locator("textarea")).toHaveValue("Half a thought");
});

/** Where `quote` is in the note on screen's text, as Rust's core sends it: lines and offsets. */
function placeOf(page: Page, quote: string) {
  return page.evaluate((quote) => {
    const text = window.__lxDocText();
    const start = text.indexOf(quote);
    const heading = [...document.querySelectorAll("#lx-doc [data-sourcepos]")].find((el) =>
      el.textContent.includes(quote),
    );
    const line = Number(/^(\d+):/.exec(heading?.getAttribute("data-sourcepos") ?? "")?.[1]);
    return { startLine: line, endLine: line, textStart: start, textEnd: start + quote.length };
  }, quote);
}

test("a target lands its heading at the top, and Back returns to where the reader was", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const top = await headingOffset(page, "big-plan");
  const place = await placeOf(page, "Section 12");
  const shown = await page.evaluate(
    ({ path, place }) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { heading: "Section 12" }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      ),
    { path: fixturePath(BIG_PLAN), place },
  );
  expect(shown).toEqual({ kind: "shown" });
  await expectLandedOn(page, "section-12");
  await expect(bar(page)).toBeHidden();
  await page.keyboard.press("Alt+ArrowLeft");
  await expect.poll(() => headingOffset(page, "big-plan")).toBe(top);
});

/** The text of each range of the agent's layer in the document. */
function agentPainted(page: Page) {
  return page.evaluate(() => {
    const highlight = CSS.highlights.get("agent");
    return highlight ? [...highlight].map((range) => (range as Range).toString()) : [];
  });
}

test("an agent's highlights paint a note's passages on their layer, and go with the file", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const places = [await placeOf(page, "Section 12"), await placeOf(page, "Big Plan")];
  const reply = await page.evaluate(
    ({ path, places }) =>
      window.__fake.agentRequest(
        {
          tool: "highlight",
          path,
          targets: [{ heading: "Section 12" }, { heading: "Big Plan" }],
          clear: false,
        },
        "Claude",
        places,
      ),
    { path: fixturePath(BIG_PLAN), places },
  );
  expect(reply).toEqual({ kind: "highlighted", count: 2 });
  expect(await agentPainted(page)).toEqual(["Section 12", "Big Plan"]);
  // Painted in the agent's colour, over the comments' and under find's.
  const layer = await page.evaluate(() => {
    const highlight = CSS.highlights.get("agent");
    const rule = [...document.styleSheets]
      .flatMap((sheet) => [...sheet.cssRules])
      .find((r) => r.cssText.startsWith("::highlight(agent)"));
    return { priority: highlight?.priority, rule: rule?.cssText ?? "" };
  });
  expect(layer.priority).toBe(2);
  expect(layer.rule).toContain("var(--agent-highlight)");
  // The reader opens another note: they go.
  await page.evaluate((path) => {
    window.__fake.emit("open-request", { path, t0Ms: null, folder: false });
  }, fixturePath(ALPHA));
  await expect(page).toHaveTitle("Alpha — Lectern");
  expect(await agentPainted(page)).toEqual([]);
});

const README_TXT = "notes/readme.txt";

/** The survey's 300 lines, which `surveyFile` makes the text file of. */
const SURVEY = Array.from(
  { length: 300 },
  (_, i) => `Survey entry ${String(i + 1).padStart(3, "0")}: the lanternfish count.`,
);

/** Opens the text file and makes it one paragraph of the survey's 300 lines, as core renders it. */
async function surveyFile(page: Page): Promise<string> {
  await openFixture(page, README_TXT);
  const path = fixturePath(README_TXT);
  await page.evaluate(
    ({ path, lines }) => {
      const last = lines[lines.length - 1] ?? "";
      const html = `<p class="lx-plain" data-sourcepos="1:1-${String(lines.length)}:${String(last.length)}">${lines.join("\n")}</p>`;
      window.__fake.setDoc(path, html);
      // Its text as core has it, the lines kept apart, so a line target finds its own.
      window.__fake.setText(path, [[1, lines.length, lines.join("\n")]]);
      window.__fake.emit("doc-changed", { path });
    },
    { path, lines: SURVEY },
  );
  await expect(page.locator("#lx-doc p.lx-plain")).toContainText("Survey entry 300");
  return path;
}

/**
 * Where core finds `text` on line `line` of the survey: its lines, its place in the text, and
 * where it starts in its paragraph's own.
 */
function surveyPlace(page: Page, line: number, text: string) {
  return page.evaluate(
    ({ line, text }) => {
      const start = window.__lxDocText().indexOf(text);
      const blockOffset = window.__lxDocText("p.lx-plain").indexOf(text);
      return {
        startLine: line,
        endLine: line,
        textStart: start,
        textEnd: start + text.length,
        blockOffset,
      };
    },
    { line, text },
  );
}

/** How far `text` of the survey sits below the pane's top, once lazy layout has settled. */
async function surveyTop(page: Page, text: string): Promise<number | null> {
  await page.waitForTimeout(1000);
  return page.evaluate((text) => {
    const pane = document.getElementById("lx-doc-pane");
    const node = document.querySelector("#lx-doc p.lx-plain")?.firstChild;
    if (!pane || !(node instanceof Text)) return null;
    const at = node.data.indexOf(text);
    const range = document.createRange();
    range.setStart(node, at);
    range.setEnd(node, at + text.length);
    return Math.round(range.getBoundingClientRect().top - pane.getBoundingClientRect().top);
  }, text);
}

test("a line target in a long paragraph of a text file lands that line at the top, and paints it alone", async ({
  page,
}) => {
  const path = await surveyFile(page);
  const line = SURVEY[249] ?? "";
  // Where core finds line 250 of it: that line's own text.
  const place = await surveyPlace(page, 250, line);
  expect(place.textStart).toBeGreaterThan(0);
  const shown = await page.evaluate(
    ({ path, place }) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { line: 250 }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      ),
    { path, place },
  );
  expect(shown).toEqual({ kind: "shown" });
  // Line 250 at the top of the pane, where a heading lands, not the paragraph's first line.
  expect(Math.abs(((await surveyTop(page, line)) ?? Infinity) - LANDING_PX)).toBeLessThanOrEqual(
    LANDING_SLACK_PX,
  );
  // A highlight of the line paints that line alone.
  const highlighted = await page.evaluate(
    ({ path, place }) =>
      window.__fake.agentRequest(
        { tool: "highlight", path, targets: [{ line: 250 }], clear: false },
        "Claude",
        [place],
      ),
    { path, place },
  );
  expect(highlighted).toEqual({ kind: "highlighted", count: 1 });
  expect(await agentPainted(page)).toEqual([line]);
});

test("a quote near the end of a long paragraph of a text file lands at the quote", async ({
  page,
}) => {
  const path = await surveyFile(page);
  // Far enough from the end that the text after it lets it reach the top.
  const quote = "Survey entry 260: the lanternfish";
  // Where core finds it: on line 260, at its text.
  const place = await surveyPlace(page, 260, quote);
  expect(place.textStart).toBeGreaterThan(0);
  const shown = await page.evaluate(
    ({ path, place, quote }) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { quote }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      ),
    { path, place, quote },
  );
  expect(shown).toEqual({ kind: "shown" });
  expect(Math.abs(((await surveyTop(page, quote)) ?? Infinity) - LANDING_PX)).toBeLessThanOrEqual(
    LANDING_SLACK_PX,
  );
  // One the paragraph's own landing shows already lands the paragraph, as before.
  const near = "Survey entry 003: the lanternfish";
  const nearPlace = await surveyPlace(page, 3, near);
  expect(
    await page.evaluate(
      ({ path, place, quote }) =>
        window.__fake.agentRequest(
          { tool: "show", path, target: { quote }, newWindow: false, wait: 0 },
          "Claude",
          [place],
        ),
      { path, place: nearPlace, quote: near },
    ),
  ).toEqual({ kind: "shown" });
  const first = SURVEY[0] ?? "";
  expect(Math.abs(((await surveyTop(page, first)) ?? Infinity) - LANDING_PX)).toBeLessThanOrEqual(
    LANDING_SLACK_PX,
  );
});

test("a passage's landing ends when another document takes its place, which keeps its own position", async ({
  page,
}) => {
  const path = await surveyFile(page);
  const plan = fixturePath(BIG_PLAN);
  await page.evaluate((plan) => {
    window.__fake.settingsElsewhere({ agentOpens: "switch" });
    // The plan was left halfway down, its place saved only as a fraction.
    const backend = window.__fake as unknown as Pick<Backend, "savePosition">;
    void backend.savePosition(plan, { headingId: null, offset: 0, line: null, fraction: 0.5 });
  }, plan);
  const quote = "Survey entry 260: the lanternfish";
  const place = await surveyPlace(page, 260, quote);
  // Two shows in a row: the passage in the file on screen, then the plan, before the passage's
  // landing has settled. What the plan shows at the top of the pane is noted as it opens.
  const opened = await page.evaluate(
    async ({ path, place, quote, plan }) => {
      const fake = window.__fake;
      const first = await fake.agentRequest(
        { tool: "show", path, target: { quote }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      );
      const second = await fake.agentRequest({
        tool: "show",
        path: plan,
        newWindow: false,
        wait: 0,
      });
      return { replies: [first, second], top: window.__lxViewTop() };
    },
    { path, place, quote, plan },
  );
  expect(opened.replies).toEqual([{ kind: "shown" }, { kind: "shown" }]);
  expect(opened.top?.line).toBeGreaterThan(1);
  await page.waitForTimeout(1500);
  // The same block at the top, where it was: the passage's landing never moved the plan.
  const settled = await page.evaluate(() => window.__lxViewTop());
  expect(settled?.line).toBe(opened.top?.line);
  expect(Math.abs((settled?.offset ?? Infinity) - (opened.top?.offset ?? 0))).toBeLessThanOrEqual(
    LANDING_SLACK_PX,
  );
});

test("a line target in a long code block of a note lands that line at the top", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const path = fixturePath(ALPHA);
  // A fenced block of 60 lines of code, on lines 4 to 63, between its fences on 3 and 64.
  const lines = Array.from(
    { length: 60 },
    (_, i) => `let tally_${String(i + 1).padStart(2, "0")} = count("lanternfish");`,
  );
  await page.evaluate(
    ({ path, lines }) => {
      const code = lines.join("\n");
      // As core renders a fenced block without a language.
      const html =
        `<h1 id="alpha" data-sourcepos="1:1-1:7">Alpha</h1>` +
        `<div class="code-block" data-sourcepos="3:1-64:3"><div class="code-head">` +
        `<span class="code-lang"></span><button type="button" class="code-copy">Copy</button>` +
        `</div><pre><code>${code}\n</code></pre></div>` +
        // Enough after it for its last lines to reach the top.
        Array.from(
          { length: 40 },
          (_, i) =>
            `<p data-sourcepos="${String(66 + 2 * i)}:1-${String(66 + 2 * i)}:9">After.</p>`,
        ).join("");
      window.__fake.setDoc(path, html);
      // Its text as core has it: the code's lines, kept apart, between the fences.
      window.__fake.setText(path, [
        [1, 1, "Alpha"],
        [4, 63, code],
        ...Array.from({ length: 40 }, (_, i): [number, number, string] => [
          66 + 2 * i,
          66 + 2 * i,
          "After.",
        ]),
      ]);
      window.__fake.emit("doc-changed", { path });
    },
    { path, lines },
  );
  await expect(page.locator("#lx-doc pre")).toContainText("tally_60");
  // Line 54 is the code's 51st line.
  const wanted = lines[50] ?? "";
  const place = await page.evaluate((line) => {
    const start = window.__lxDocText().indexOf(line);
    const blockOffset = window.__lxDocText(".code-block").indexOf(line);
    return {
      startLine: 54,
      endLine: 54,
      textStart: start,
      textEnd: start + line.length,
      blockOffset,
    };
  }, wanted);
  expect(place.textStart).toBeGreaterThan(0);
  const shown = await page.evaluate(
    ({ path, place }) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { line: 54 }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      ),
    { path, place },
  );
  expect(shown).toEqual({ kind: "shown" });
  await page.waitForTimeout(1000);
  const top = await page.evaluate((line) => {
    const pane = document.getElementById("lx-doc-pane");
    const code = document.querySelector("#lx-doc pre code");
    if (!pane || !code) return null;
    const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node as Text;
      const at = text.data.indexOf(line);
      if (at === -1) continue;
      const range = document.createRange();
      range.setStart(text, at);
      range.setEnd(text, at + line.length);
      return Math.round(range.getBoundingClientRect().top - pane.getBoundingClientRect().top);
    }
    return null;
  }, wanted);
  expect(Math.abs((top ?? Infinity) - LANDING_PX)).toBeLessThanOrEqual(LANDING_SLACK_PX);
});

test("a quote folded in a closed details opens it, and lands with the quote in view", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const path = fixturePath(ALPHA);
  const quote = "surface at dusk";
  await page.evaluate((path) => {
    const paragraphs = (from: number, text: string) =>
      Array.from({ length: 40 }, (_, i) => {
        const line = String(from + 2 * i);
        return `<p data-sourcepos="${line}:1-${line}:${String(text.length)}">${text}</p>`;
      }).join("");
    // As core renders Markdown between raw HTML: the details has no lines, its paragraph does.
    // Far enough down to need a scroll, with enough after it to reach the top.
    const html =
      `<h1 id="alpha" data-sourcepos="1:1-1:7">Alpha</h1>${paragraphs(3, "Before.")}` +
      `<details><summary>Field notes</summary>` +
      `<p data-sourcepos="86:1-86:41">The lanternfish surface at dusk in spring.</p></details>` +
      paragraphs(90, "After.");
    window.__fake.setDoc(path, html);
    const lines = (from: number, text: string) =>
      Array.from({ length: 40 }, (_, i): [number, number, string] => [
        from + 2 * i,
        from + 2 * i,
        text,
      ]);
    window.__fake.setText(path, [
      [1, 1, "Alpha"],
      ...lines(3, "Before."),
      [86, 86, "The lanternfish surface at dusk in spring."],
      ...lines(90, "After."),
    ]);
    window.__fake.emit("doc-changed", { path });
  }, path);
  const details = page.locator("#lx-doc details");
  await expect(details).toHaveCount(1);
  await expect(details).not.toHaveAttribute("open");
  // Where core finds the quote: on line 86, at its text.
  const place = await page.evaluate((quote) => {
    const start = window.__lxDocText().indexOf(quote);
    const blockOffset = window.__lxDocText("details p").indexOf(quote);
    return {
      startLine: 86,
      endLine: 86,
      textStart: start,
      textEnd: start + quote.length,
      blockOffset,
    };
  }, quote);
  expect(place.textStart).toBeGreaterThan(0);
  const shown = await page.evaluate(
    ({ path, place, quote }) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { quote }, newWindow: false, wait: 0 },
        "Claude",
        [place],
      ),
    { path, place, quote },
  );
  expect(shown).toEqual({ kind: "shown" });
  await expect(details).toHaveAttribute("open", "");
  await page.waitForTimeout(1000);
  // The quote is laid out, inside the pane.
  const box = await page.evaluate((quote) => {
    const pane = document.getElementById("lx-doc-pane")?.getBoundingClientRect();
    const node = document.querySelector("#lx-doc details p")?.firstChild;
    if (!pane || !(node instanceof Text)) return null;
    const range = document.createRange();
    const at = node.data.indexOf(quote);
    range.setStart(node, at);
    range.setEnd(node, at + quote.length);
    const r = range.getBoundingClientRect();
    return {
      height: r.height,
      top: r.top - pane.top,
      bottom: r.bottom - pane.top,
      pane: pane.height,
    };
  }, quote);
  expect(box?.height).toBeGreaterThan(0);
  expect(box?.top).toBeGreaterThanOrEqual(0);
  expect(box?.bottom).toBeLessThanOrEqual(box?.pane ?? 0);
});

test("a line before the first block, in a note's frontmatter, lands at the top", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  // Read well down the plan first.
  const item = page.locator("#lx-outline li", { hasText: /^Section 12$/ });
  await item.locator("a").click();
  await expectLandedOn(page, "section-12");
  const pane = page.locator("#lx-doc-pane");
  expect(await pane.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  // Line 1 is the frontmatter's opening fence: core finds the line, with no text on it.
  const shown = await page.evaluate(
    (path) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { line: 1 }, newWindow: false, wait: 0 },
        "Claude",
        [{ startLine: 1, endLine: 1 }],
      ),
    fixturePath(BIG_PLAN),
  );
  expect(shown).toEqual({ kind: "shown" });
  await expect.poll(() => pane.evaluate((el) => el.scrollTop)).toBe(0);
  // Back returns to where the reader was.
  await page.keyboard.press("Alt+ArrowLeft");
  await expectLandedOn(page, "section-12");
});

test("an open at a line in a note's frontmatter lands at the top, not where the reader left it", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const plan = fixturePath(BIG_PLAN);
  await page.evaluate((plan) => {
    window.__fake.settingsElsewhere({ agentOpens: "switch" });
    // The reader left the plan at Section 12.
    const backend = window.__fake as unknown as Pick<Backend, "savePosition">;
    void backend.savePosition(plan, {
      headingId: "section-12",
      offset: 0,
      line: null,
      fraction: 0,
    });
  }, plan);
  const shown = await page.evaluate(
    (path) =>
      window.__fake.agentRequest(
        { tool: "show", path, target: { line: 1 }, newWindow: false, wait: 0 },
        "Claude",
        [{ startLine: 1, endLine: 1 }],
      ),
    plan,
  );
  expect(shown).toEqual({ kind: "shown" });
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1000);
  expect(await page.locator("#lx-doc-pane").evaluate((el) => el.scrollTop)).toBe(0);
});

test("Show finds a held target in the revision it renders, though the file changes as it opens", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const plan = fixturePath(BIG_PLAN);
  // Where core finds Section 12 as the bar goes up: its heading's line.
  const line = await page.evaluate(
    (plan) => window.__fake.blocksOf(plan).find(([, , text]) => text === "Section 12")?.[0] ?? 0,
    plan,
  );
  expect(line).toBeGreaterThan(1);
  const asked = await page.evaluate(
    ({ plan, line }) =>
      window.__fake.agentRequest(
        { tool: "show", path: plan, target: { heading: "Section 12" }, newWindow: false, wait: 0 },
        "Claude",
        [{ startLine: line, endLine: line }],
      ),
    { plan, line },
  );
  expect(asked).toEqual({ kind: "asked" });
  // The plan changes on disk just after the open has read it, before the target is found again:
  // a paragraph lands before Section 12, which moves two lines down; the watcher tells the window.
  // The refresh that follows is held until the test lets it go (`letRefresh`).
  await page.evaluate(
    ({ plan, line }) => {
      const fake = window.__fake as unknown as Backend & typeof window.__fake;
      const open = fake.openDocument.bind(fake);
      const held = new Promise<void>((resolve) => {
        (window as unknown as { letRefresh: () => void }).letRefresh = resolve;
      });
      let changed = false;
      fake.openDocument = async (path, generation) => {
        if (changed && path === plan) {
          await held;
          return open(path, generation);
        }
        const result = await open(path, generation);
        if (!changed && path === plan) {
          changed = true;
          const html = (fake.html(plan) ?? "").replace(
            /data-sourcepos="(\d+):(\d+)-(\d+):(\d+)"/g,
            (_, s: string, a: string, e: string, b: string) => {
              const shift = (n: string) => String(Number(n) >= line ? Number(n) + 2 : Number(n));
              return `data-sourcepos="${shift(s)}:${a}-${shift(e)}:${b}"`;
            },
          );
          fake.setDoc(
            plan,
            html.replace(
              '<h2 id="section-12"',
              `<p data-sourcepos="${String(line)}:1-${String(line)}:9">Inserted.</p><h2 id="section-12"`,
            ),
          );
          const blocks = fake
            .blocksOf(plan)
            .map(([s, e, text]): [number, number, string] => [
              s >= line ? s + 2 : s,
              e >= line ? e + 2 : e,
              text,
            ]);
          const at = blocks.findIndex(([s]) => s > line);
          blocks.splice(at, 0, [line, line, "Inserted."]);
          fake.setText(plan, blocks);
          fake.emit("doc-changed", { path: plan });
        }
        return result;
      };
    },
    { plan, line },
  );
  await bar(page).getByRole("button", { name: "Show" }).click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expect
    .poll(() => page.evaluate(() => window.__fake.agentAnswers))
    .toEqual([{ id: 1, answer: "shown" }]);
  // Landed in the revision on screen, the change not yet rendered: Section 12 at the top.
  const inserted = page.locator("#lx-doc p", { hasText: /^Inserted\.$/ });
  await expect(inserted).toHaveCount(0);
  await expectLandedOn(page, "section-12");
  // The change renders after the landing, keeping the place as for any edit: Section 12 in view.
  await page.evaluate(() => {
    (window as unknown as { letRefresh: () => void }).letRefresh();
  });
  await expect(inserted).toHaveCount(1);
  await page.waitForTimeout(1000);
  const offset = await headingOffset(page, "section-12");
  const pane = await page.locator("#lx-doc-pane").evaluate((el) => el.clientHeight);
  expect(offset).toBeGreaterThanOrEqual(0);
  expect(offset).toBeLessThan(pane);
});

test("a show within the file on screen lands in the revision a change is being shown as", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const plan = fixturePath(BIG_PLAN);
  // Where core finds Section 12 as the show comes: its heading's line, in the revision on screen.
  const line = await page.evaluate(
    (plan) => window.__fake.blocksOf(plan).find(([, , text]) => text === "Section 12")?.[0] ?? 0,
    plan,
  );
  expect(line).toBeGreaterThan(1);
  // The plan changes on disk: a paragraph lands before Section 12, which moves two lines down; the
  // watcher tells the window. The refresh that follows is held until the test lets it go.
  await page.evaluate(
    ({ plan, line }) => {
      const fake = window.__fake as unknown as Backend & typeof window.__fake;
      const open = fake.openDocument.bind(fake);
      const held = new Promise<void>((resolve) => {
        (window as unknown as { letRefresh: () => void }).letRefresh = resolve;
      });
      fake.openDocument = async (path, generation) => {
        if (path === plan) {
          await held;
        }
        return open(path, generation);
      };
      const html = (fake.html(plan) ?? "").replace(
        /data-sourcepos="(\d+):(\d+)-(\d+):(\d+)"/g,
        (_, s: string, a: string, e: string, b: string) => {
          const shift = (n: string) => String(Number(n) >= line ? Number(n) + 2 : Number(n));
          return `data-sourcepos="${shift(s)}:${a}-${shift(e)}:${b}"`;
        },
      );
      fake.setDoc(
        plan,
        html.replace(
          '<h2 id="section-12"',
          `<p data-sourcepos="${String(line)}:1-${String(line)}:9">Inserted.</p><h2 id="section-12"`,
        ),
      );
      const blocks = fake
        .blocksOf(plan)
        .map(([s, e, text]): [number, number, string] => [
          s >= line ? s + 2 : s,
          e >= line ? e + 2 : e,
          text,
        ]);
      const at = blocks.findIndex(([s]) => s > line);
      blocks.splice(at, 0, [line, line, "Inserted."]);
      fake.setText(plan, blocks);
      fake.emit("doc-changed", { path: plan });
    },
    { plan, line },
  );
  const scrollTop = () => page.locator("#lx-doc-pane").evaluate((el) => el.scrollTop);
  const top = await scrollTop();
  // The show comes with where core found Section 12 as it came, while the refresh is on its way.
  await page.evaluate(
    ({ plan, line }) => {
      void window.__fake
        .agentRequest(
          {
            tool: "show",
            path: plan,
            target: { heading: "Section 12" },
            newWindow: false,
            wait: 0,
          },
          "Claude",
          [{ startLine: line, endLine: line }],
        )
        .then((reply) => {
          (window as unknown as { shown: unknown }).shown = reply;
        });
    },
    { plan, line },
  );
  const answer = () => page.evaluate(() => (window as unknown as { shown?: unknown }).shown);
  // It waits for the refresh: unanswered, the view not moved.
  await page.waitForTimeout(300);
  expect(await answer()).toBeUndefined();
  expect(await scrollTop()).toBe(top);
  await page.evaluate(() => {
    (window as unknown as { letRefresh: () => void }).letRefresh();
  });
  await expect.poll(answer).toEqual({ kind: "shown" });
  // Landed in the revision the reader sees, the change rendered: Section 12 at the top.
  await expect(page.locator("#lx-doc p", { hasText: /^Inserted\.$/ })).toHaveCount(1);
  await expectLandedOn(page, "section-12");
});

const REPORT = "artifacts/report.html";

/** The text of each range the page's bridge paints in the agent's layer, in the page. */
function pagePainted(page: Page) {
  return page
    .frameLocator("#lx-doc-pane > iframe.lx-frame")
    .locator("body")
    .evaluate(() => {
      const highlight = CSS.highlights.get("lx-agent");
      return highlight ? [...highlight].map((range) => (range as Range).toString()) : [];
    });
}

test("in an HTML page, the window finds a passage in what the page reported, and paints it", async ({
  page,
}) => {
  await openFixture(page, REPORT);
  await expect.poll(() => page.evaluate(() => window.__lxPage()?.ready ?? false)).toBe(true);
  const path = fixturePath(REPORT);
  const request = (targets: unknown[]) =>
    page.evaluate(
      ({ path, targets }) =>
        window.__fake.agentRequest({
          tool: "highlight",
          path,
          targets: targets as { quote: string }[],
          clear: false,
        }),
      { path, targets },
    );
  expect(await request([{ quote: "conversion dipped\n in week 3" }])).toEqual({
    kind: "highlighted",
    count: 1,
  });
  await expect.poll(() => pagePainted(page)).toEqual(["conversion dipped in week 3"]);
  expect(await request([{ quote: "rolled back" }])).toEqual({
    kind: "failed",
    message: "That passage isn't in report.html.",
  });
  // Its text, scripts and all, for `read`.
  const read = await page.evaluate(
    (path) => window.__fake.agentRequest({ tool: "read", path }),
    path,
  );
  expect(read.kind).toBe("pageText");
  expect(JSON.stringify(read)).toContain("Rendered by script");
});

test("a page an agent opens is waited for: a passage that isn't there fails, one that is is shown", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  await page.evaluate(() => {
    window.__fake.settingsElsewhere({ agentOpens: "switch" });
  });
  const show = (quote: string) =>
    page.evaluate(
      ({ path, quote }) =>
        window.__fake.agentRequest({
          tool: "show",
          path,
          target: { quote },
          newWindow: false,
          wait: 0,
        }),
      { path: fixturePath(REPORT), quote },
    );
  expect(await show("rolled back")).toEqual({
    kind: "failed",
    message: "That passage isn't in report.html.",
  });
  await expect(page).toHaveTitle("report.html — Lectern");
  // Opened afresh, and on screen already.
  await page.evaluate((path) => {
    window.__fake.emit("open-request", { path, t0Ms: null, folder: false });
  }, fixturePath(ALPHA));
  await expect(page).toHaveTitle("Alpha — Lectern");
  expect(await show("conversion dipped in week 3")).toEqual({ kind: "shown" });
  expect(await show("Rendered by script")).toEqual({ kind: "shown" });
});

test("an agent's highlights in a note are found again as it changes, and go when their text does", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  const path = fixturePath(ALPHA);
  const places = [await placeOf(page, "platform team"), await placeOf(page, "Blocked on")];
  const reply = await page.evaluate(
    ({ path, places }) =>
      window.__fake.agentRequest(
        {
          tool: "highlight",
          path,
          targets: [{ quote: "platform team" }, { quote: "Blocked on" }],
          clear: false,
        },
        "Claude",
        places,
      ),
    { path, places },
  );
  expect(reply).toEqual({ kind: "highlighted", count: 2 });
  expect(await agentPainted(page)).toEqual(["platform team", "Blocked on"]);
  // The paragraph is rewritten on disk: "Blocked on" goes, "platform team" moves along.
  await page.evaluate((path) => {
    const before = "Blocked on a review from the platform team.";
    const after = "Waiting, still, for a review from the platform team.";
    const fake = window.__fake;
    fake.setText(
      path,
      fake
        .blocksOf(path)
        .map(([s, e, t]): [number, number, string] => [s, e, t === before ? after : t]),
    );
    fake.setDoc(path, (fake.html(path) ?? "").replace(before, after));
    fake.emit("doc-changed", { path });
  }, path);
  await expect(page.locator("#lx-doc p", { hasText: "Waiting, still" })).toBeVisible();
  await expect.poll(() => agentPainted(page)).toEqual(["platform team"]);
});

/** The text of each range the page's bridge paints in the comments' layer, in the page. */
function pageComments(page: Page) {
  return page
    .frameLocator("#lx-doc-pane > iframe.lx-frame")
    .locator("body")
    .evaluate(() => {
      const highlight = CSS.highlights.get("lx-comment");
      return highlight ? [...highlight].map((range) => (range as Range).toString()) : [];
    });
}

test("an agent's comment on a page on screen is its own card, painted there; your reply is yours", async ({
  page,
}) => {
  await openFixture(page, REPORT);
  await expect.poll(() => page.evaluate(() => window.__lxPage()?.ready ?? false)).toBe(true);
  const path = fixturePath(REPORT);
  const commented = await page.evaluate(
    (path) =>
      window.__fake.agentRequest({
        tool: "comment",
        path,
        target: { quote: "conversion dipped\n in week 3" },
        text: "Weekly, not daily?",
      }),
    path,
  );
  expect(commented).toEqual({ kind: "commented", id: 1, status: "question" });
  const card = page.locator(".comment-card[data-id='1']");
  await expect(card.locator(".comment-quote")).toHaveText("conversion dipped in week 3");
  await expect(card.locator(".comment-entry.agent .comment-author")).toHaveText("Claude question");
  await expect(card.locator("[data-action='edit']")).toHaveCount(0);
  await expect.poll(() => pageComments(page)).toEqual(["conversion dipped in week 3"]);

  await page.locator("#lx-tab-comments").click();
  await card.locator("[data-action='reply']").click();
  await card.locator("textarea").fill("Daily.");
  await page.keyboard.press("Control+Enter");
  await expect(card.locator(".comment-author")).toHaveText(["Claude question", "You"]);
  await expect(card).toHaveAttribute("data-status", "open");
  const resolved = await page.evaluate(
    (path) => window.__fake.agentRequest({ tool: "resolve", path, id: "C1" }),
    path,
  );
  expect(resolved).toEqual({ kind: "commented", id: 1, status: "resolved" });
  await expect(card).toHaveCount(0);
});

test("show at a comment thread opens the Comments tab on its card", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const path = fixturePath(BIG_PLAN);
  await page.evaluate((path) => {
    window.__fake.agentComment(path, {
      startLine: 1,
      endLine: 1,
      quote: "Big Plan",
      headingPath: ["Big Plan"],
      name: "Claude",
      kind: "question",
      text: "Is the title right?",
    });
  }, path);
  const shown = await page.evaluate(
    (path) =>
      window.__fake.agentRequest({
        tool: "show",
        path,
        target: { comment: "C1" },
        newWindow: false,
        wait: 0,
      }),
    path,
  );
  expect(shown).toEqual({ kind: "shown" });
  await expect(page.locator("#lx-tab-comments")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".comment-card[data-id='1']")).toHaveClass(/selected/);
});
