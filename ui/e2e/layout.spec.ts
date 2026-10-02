import { expect, test } from "@playwright/test";
import { VAULT, expectLandedOn, launch, openFixture } from "./util";

const BIG_PLAN = "work/alpha/plans/2026-01-01-big-plan.md";

test("the chrome carries the lx- layout", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  for (const id of [
    "lx-app",
    "lx-header",
    "lx-history-nav",
    "lx-breadcrumbs",
    "lx-header-actions",
    "lx-progress",
    "lx-library",
    "lx-doc-pane",
    "lx-banner",
    "lx-properties",
    "lx-doc",
    "lx-outline",
    "lx-overlay-root",
    "lx-toasts",
  ]) {
    await expect(page.locator(`#${id}`), id).toHaveCount(1);
  }
  await expect(page.locator(".resizer[data-for=library]")).toHaveCount(1);
  await expect(page.locator(".resizer[data-for=outline]")).toHaveCount(1);
});

test("no horizontal page overflow on wide table", async ({ page }) => {
  await openFixture(page, "stress/wide-table.md");
  const m = await page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    return {
      page: document.documentElement.scrollWidth,
      inner: window.innerWidth,
      paneScroll: pane?.scrollWidth ?? Infinity,
      paneClient: pane?.clientWidth ?? 0,
    };
  });
  expect(m.page).toBeLessThanOrEqual(m.inner);
  expect(m.paneScroll).toBeLessThanOrEqual(m.paneClient);
});

test("a wide table breaks out of the measure but stays inside the pane", async ({ page }) => {
  await openFixture(page, "stress/wide-table.md");
  // A 50-character measure, so the column is narrower than the pane whatever the font's metrics.
  await page.evaluate(() => {
    document.documentElement.style.setProperty("--measure", "50ch");
  });
  const box = await page.evaluate(() => {
    const rect = (sel: string) => {
      const r = document.querySelector(sel)?.getBoundingClientRect();
      return r ? { left: r.left, right: r.right } : null;
    };
    return {
      wrap: rect("#lx-doc .table-wrap"),
      para: rect("#lx-doc > p"),
      pane: rect("#lx-doc-pane"),
    };
  });
  const { wrap, para, pane } = box;
  if (!wrap || !para || !pane) throw new Error("missing the table, a paragraph or the pane");
  // Wider than the text column on both sides...
  expect(wrap.left).toBeLessThan(para.left - 10);
  expect(wrap.right).toBeGreaterThan(para.right + 10);
  // ...but within the pane, gutters included.
  expect(wrap.left).toBeGreaterThan(pane.left + 8);
  expect(wrap.right).toBeLessThan(pane.right - 8);
});

test("clicking a far outline entry lands on its heading and stays there", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const item = page.locator("#lx-outline li", { hasText: /^Section 12$/ });
  await item.locator("a").click();
  await expectLandedOn(page, "section-12");
  await expect(item).toHaveClass(/active/);
});

test("the progress bar follows the scroll", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const scale = () =>
    page.evaluate(() => {
      const bar = document.getElementById("lx-progress");
      return bar ? new DOMMatrix(getComputedStyle(bar).transform).a : -1;
    });
  expect(await scale()).toBeLessThan(0.05);
  await page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    if (pane) pane.scrollTop = pane.scrollHeight;
  });
  await expect.poll(scale).toBeGreaterThan(0.95);
});

test("the welcome screen shows without a document", async ({ page }) => {
  await launch(page);
  await expect(page).toHaveTitle("Lectern");
  const welcome = page.locator("#lx-doc .welcome");
  await expect(welcome.getByRole("button", { name: /Open file/ })).toBeVisible();
  await expect(welcome.getByRole("button", { name: /Add folder/ })).toBeVisible();
  await expect(welcome).toContainText("Ctrl+P");
  await expect(page.locator("#lx-properties")).toBeHidden();
});

test("a missing file shows the error state", async ({ page }) => {
  await launch(page, `${VAULT}\\gone.md`);
  const state = page.locator("#lx-doc .state-error");
  await expect(state).toContainText(`${VAULT}\\gone.md`);
  await expect(state.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(state.getByRole("button", { name: "Remove from recent" })).toBeVisible();
  await expect(state.getByRole("button", { name: "Open with default app" })).toHaveCount(0);
});
