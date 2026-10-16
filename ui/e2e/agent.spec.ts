// An agent's `status` request (src/agent.ts), through the fake backend: it names the heading the
// outline marks as being read, which a navigation lands a little below the pane's top.
import { expect, test } from "@playwright/test";
import { expectLandedOn, fixturePath, openFixture } from "./util";

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
