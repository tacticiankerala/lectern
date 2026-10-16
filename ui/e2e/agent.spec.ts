// An agent's requests (src/agent.ts), through the fake backend: `status` names the heading the
// outline marks as being read, which a navigation lands a little below the pane's top; `show` asks
// with a bar over the document, and an agent's switch keeps a half-written comment. A target lands
// at the top, with Back to where the reader was; an agent's highlights paint on their own layer in
// a note, and through its bridge in an HTML page, whose text `read` takes from the page. An agent's
// comment on a page on screen is its own card, painted in the page, and `show` at a thread opens
// the Comments tab on it.
import { expect, test, type Page } from "@playwright/test";
import { expectLandedOn, fixturePath, headingOffset, openFixture } from "./util";

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
