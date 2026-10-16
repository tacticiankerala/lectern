// An agent's requests (src/agent.ts), through the fake backend: `status` names the heading the
// outline marks as being read, which a navigation lands a little below the pane's top; `show` asks
// with a bar over the document, and an agent's switch keeps a half-written comment.
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
