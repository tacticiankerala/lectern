import { expect, test } from "@playwright/test";
import { expectLandedOn, fixturePath, openFixture } from "./util";

const BIG_PLAN = "work/alpha/plans/2026-01-01-big-plan.md";

test("opens big plan and shows outline + tasks counter", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  await expect(page).toHaveTitle("Big Plan — Lectern");
  expect(await page.locator("#lx-outline li").count()).toBeGreaterThan(10);
  await expect(page.locator("#lx-properties .props-tasks")).toHaveText(/^\d+ \/ \d+ tasks$/);
  await expect(page.locator("#lx-properties .badge.status-active")).toHaveText("active");
});

test("wikilink navigates", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  const h1 = page.locator("#lx-doc h1");
  await expect(h1).toHaveText("Memory index");
  await page.locator("#lx-doc a.wikilink:not(.broken)").first().click();
  await expect(h1).not.toHaveText("Memory index");
  // The switch is timed to the next paint.
  await expect
    .poll(() => page.evaluate(() => window.__fake.marks.map((m) => m.name)))
    .toEqual(["first-paint", "doc-switch"]);
});

test("wikilink to a heading lands on it through its slug", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  // [[big-plan#Section 2]]: no id is "Section 2", so the slug "section-2" is used.
  await page.locator('#lx-doc a.wikilink[data-anchor="Section 2"]').click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expectLandedOn(page, "section-2");
});

test("a wikilink to a far heading stays on it once lazy layout settles", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  const index = fixturePath("memory/index.md");
  const big = fixturePath(BIG_PLAN);
  await page.evaluate(
    ({ index, big }) => {
      window.__fake.setDoc(
        index,
        `<h1 id="memory-index">Memory index</h1><p><a href="#" class="wikilink" data-kind="doc" data-target="${big}" data-anchor="Section 15" data-slug="section-15">far</a></p>`,
      );
      window.__fake.emit("doc-changed", { path: index });
    },
    { index, big },
  );
  await page.locator('#lx-doc a.wikilink[data-anchor="Section 15"]').click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expectLandedOn(page, "section-15");
});

test("broken wikilink shows toast", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.locator("#lx-doc a.wikilink.broken").first().click();
  await expect(page.locator("#lx-toasts .toast")).toHaveText("No note named missing-note");
  await expect(page.locator("#lx-doc h1")).toHaveText("Memory index");
});

test("copy button copies", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, BIG_PLAN);
  const block = page.locator("#lx-doc .code-block").first();
  await block.locator(".code-copy").click();
  await expect(block.locator(".code-copy")).toHaveText("Copied");
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain(
    'RSpec.describe Alert11 do\n  let(:record) { described_class.new(name: "alert-1.1") }',
  );
  await expect(block.locator(".code-copy")).toHaveText("Copy");
});

test("script in fixture never executes", async ({ page }) => {
  let dialogs = 0;
  page.on("dialog", (dialog) => {
    dialogs += 1;
    void dialog.dismiss();
  });
  await openFixture(page, "prompts/writer.md");
  await expect(page.locator("#lx-doc h1")).toHaveText("Writer prompt");
  // `<img src=x onerror=…>` has failed to load and been swapped once this shows.
  await expect(page.locator("#lx-doc .img-placeholder")).toHaveCount(1);
  expect(dialogs).toBe(0);
  expect(await page.evaluate(() => "__xss" in window)).toBe(false);
  expect(await page.locator("#lx-doc script, #lx-doc [onerror]").count()).toBe(0);
});

test("a local image loads through the asset route", async ({ page }) => {
  await openFixture(page, "friends/readme-style.md");
  const logo = page.locator('#lx-doc img[alt="logo"]');
  await expect(logo).toHaveJSProperty("complete", true);
  expect(await logo.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
});

test("doc-changed reloads the document in place", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.evaluate((path) => {
    window.__fake.setDoc(path, "<h1 id='memory-index'>Memory index, revised</h1>");
    window.__fake.emit("doc-changed", { path });
  }, fixturePath("memory/index.md"));
  await expect(page.locator("#lx-doc h1")).toHaveText("Memory index, revised");
});

test("doc-removed shows the not-found state with Retry", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.evaluate((path) => {
    window.__fake.setDoc(path, null);
    window.__fake.emit("doc-removed", { path });
  }, fixturePath("memory/index.md"));
  await expect(page.locator("#lx-doc .state-error")).toContainText("memory\\index.md");
  await expect(page.locator("#lx-doc .state-error button", { hasText: "Retry" })).toBeVisible();
});
