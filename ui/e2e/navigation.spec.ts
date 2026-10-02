import { expect, test, type Page } from "@playwright/test";
import { VAULT, fixturePath, headingOffset, launch, openFixture } from "./util";

const BIG_PLAN = "work/alpha/plans/2026-01-01-big-plan.md";
/** The fake library's unreachable root. */
const OFFLINE = "\\\\offline-nas\\share\\notes";

/** A sidebar row by its fixture path ("" for the root itself). */
function row(page: Page, rel: string) {
  const path = rel === "" ? VAULT : fixturePath(rel);
  return page.locator(`#lx-library .tree-row[data-path="${path.replaceAll("\\", "\\\\")}"]`);
}

test("opens a document through the sidebar", async ({ page }) => {
  await launch(page);
  const memory = row(page, "memory");
  await expect(memory).toBeVisible();
  await expect(row(page, "memory/index.md")).toHaveCount(0);
  // A folder without a README toggles from its name.
  await memory.locator(".tree-name").click();
  await expect(memory).toHaveAttribute("aria-expanded", "true");
  await row(page, "memory/index.md").locator(".tree-name").click();
  await expect(page).toHaveTitle("Memory index — Lectern");
  await expect(row(page, "memory/index.md")).toHaveClass(/active/);
  // The chevron collapses it again.
  await memory.locator(".tree-chevron").click();
  await expect(memory).toHaveAttribute("aria-expanded", "false");
  await expect(row(page, "memory/index.md")).toHaveCount(0);
});

test("a folder click opens its README and shows the status badge", async ({ page }) => {
  await launch(page);
  await row(page, "work").locator(".tree-name").click();
  const alpha = row(page, "work/alpha");
  await expect(alpha.locator(".badge.status-blocked")).toHaveText("blocked");
  await alpha.locator(".tree-name").click();
  await expect(page).toHaveTitle("Alpha — Lectern");
  await expect(alpha).toHaveAttribute("aria-expanded", "true");
  await expect(row(page, "work/alpha/README.md")).toHaveClass(/active/);
  // The root's README folder carries its badge too.
  await expect(page.locator("#lx-library .lib-root-head .badge.status-active")).toHaveText(
    "active",
  );
});

test("the open document is revealed in the sidebar, and folders stay expanded", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const active = row(page, BIG_PLAN);
  await expect(active).toHaveClass(/active/);
  await expect(active).toBeVisible();
  await expect(row(page, "work/alpha/plans")).toHaveAttribute("aria-expanded", "true");
  // Expanded folders persist across launches.
  await launch(page);
  await expect(row(page, "work/alpha/plans")).toHaveAttribute("aria-expanded", "true");
});

test("quick open finds a file by fuzzy name", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.keyboard.press("Control+p");
  const input = page.locator(".quick-open input");
  await expect(input).toBeFocused();
  await page.keyboard.type("big");
  const first = page.locator(".quick-open .qo-item").first();
  await expect(first).toHaveAttribute("aria-selected", "true");
  await expect(first.locator(".qo-name")).toHaveText("2026-01-01-big-plan.md");
  await expect(first.locator("mark")).toHaveText(["big"]);
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expect(page.locator(".quick-open")).toBeHidden();
});

test("quick open lists recent files first and closes on Esc", async ({ page }) => {
  await openFixture(page, "prompts/writer.md");
  await row(page, "stress").locator(".tree-name").click();
  await row(page, "stress/wide-table.md").locator(".tree-name").click();
  await expect(page).toHaveTitle("Wide table — Lectern");
  await page.keyboard.press("Control+p");
  const names = page.locator(".quick-open .qo-item .qo-name");
  await expect(names.nth(0)).toHaveText("wide-table.md");
  await expect(names.nth(1)).toHaveText("writer.md");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".quick-open .qo-item").nth(1)).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.keyboard.press("Escape");
  await expect(page.locator(".quick-open")).toBeHidden();
  await expect(page).toHaveTitle("Wide table — Lectern");
});

test("breadcrumbs open README folders and reveal the others", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const crumbs = page.locator("#lx-breadcrumbs .crumb");
  await expect(crumbs).toHaveText(["vault", "work", "alpha", "plans", "2026-01-01-big-plan.md"]);
  // A folder without a README: revealed in the sidebar.
  await row(page, "work/alpha/plans").locator(".tree-chevron").click();
  await expect(row(page, BIG_PLAN)).toHaveCount(0);
  await crumbs.nth(3).click();
  await expect(row(page, "work/alpha/plans")).toBeFocused();
  // A folder with a README: opened.
  await crumbs.nth(2).click();
  await expect(page).toHaveTitle("Alpha — Lectern");
  await expect(crumbs).toHaveText(["vault", "work", "alpha", "README.md"]);
});

test("back and forward restore the reading position", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const back = page.getByRole("button", { name: "Back", exact: true });
  const forward = page.getByRole("button", { name: "Forward", exact: true });
  await expect(back).toBeDisabled();
  await page
    .locator("#lx-outline li", { hasText: /^Section 12$/ })
    .locator("a")
    .click();
  await page.waitForTimeout(1100);
  const before = await headingOffset(page, "section-12");
  await page.keyboard.press("Control+p");
  await page.keyboard.type("memory index");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Memory index — Lectern");
  await expect(back).toBeEnabled();

  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1100);
  const after = await headingOffset(page, "section-12");
  expect(Math.abs((after ?? Infinity) - (before ?? 0))).toBeLessThanOrEqual(8);
  await expect(forward).toBeEnabled();

  await page.keyboard.press("Alt+ArrowRight");
  await expect(page).toHaveTitle("Memory index — Lectern");
  await expect(forward).toBeDisabled();

  // Mouse button 3 (back) and the header buttons work too.
  await page.locator("#lx-doc").dispatchEvent("mouseup", { button: 3 });
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await forward.click();
  await expect(page).toHaveTitle("Memory index — Lectern");
});

test("an unavailable root shows its reason and Retry", async ({ page }) => {
  await launch(page);
  const root = page.locator(
    `#lx-library .lib-root[data-root="${OFFLINE.replaceAll("\\", "\\\\")}"]`,
  );
  await expect(root.locator(".lib-root-state")).toContainText("Couldn't reach");
  await root.getByRole("button", { name: "Retry" }).click();
  await expect.poll(() => page.evaluate(() => window.__fake.retried)).toEqual([OFFLINE]);
});

test("the context menu copies a path and removes a root", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await launch(page);
  await row(page, "memory").locator(".tree-name").click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveText(["Reveal in Explorer", "Copy path"]);
  await menu.getByRole("menuitem", { name: "Copy path" }).click();
  await expect(menu).toHaveCount(0);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(fixturePath("memory"));

  const offline = page.locator(".lib-root-head", { hasText: "notes" });
  await offline.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Remove from library" }).click();
  await expect(page.locator("#lx-library .lib-root")).toHaveCount(1);
});

test("preferences: a path mapping persists across reload", async ({ page }) => {
  await launch(page);
  await page.keyboard.press("Control+,");
  const prefs = page.getByRole("dialog", { name: "Preferences" });
  await expect(prefs).toBeVisible();
  await expect(prefs).toContainText("0.0.0-fake");
  await prefs.getByRole("button", { name: "Add mapping" }).click();
  await prefs.getByLabel("Map from").last().fill("/home/me/shared");
  await prefs.getByLabel("Map to").last().fill("S:\\Shared");
  // Saved on blur.
  await prefs.getByLabel("Map to").last().blur();
  await page.keyboard.press("Escape");
  await expect(prefs).toHaveCount(0);

  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  // The ⋯ menu opens it too.
  await page.getByRole("button", { name: "More", exact: true }).click();
  await page.getByRole("menuitem", { name: "Preferences" }).click();
  await expect(prefs.getByLabel("Map from")).toHaveValue("/home/me/shared");
  await expect(prefs.getByLabel("Map to")).toHaveValue("S:\\Shared");
});

test("dropping a file opens it; dropping a folder adds it to the library", async ({ page }) => {
  await launch(page);
  await page.evaluate((path) => {
    window.__fake.drop([path]);
  }, fixturePath("friends/readme-style.md"));
  await expect(page).toHaveTitle("Readme style — Lectern");
  await page.evaluate(() => {
    window.__fake.drop(["D:\\Elsewhere\\Notes"]);
  });
  await expect(
    page.locator('#lx-library .lib-root[data-root="D:\\\\Elsewhere\\\\Notes"]'),
  ).toHaveCount(1);
  await expect(page.locator("#lx-toasts .toast")).toContainText("Added D:\\Elsewhere\\Notes");
});

test("Ctrl+B and the header button toggle the sidebars", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  await expect(page.locator("#lx-library")).toBeVisible();
  await page.keyboard.press("Control+b");
  await expect(page.locator("#lx-library")).toBeHidden();
  await page.keyboard.press("Control+b");
  await expect(page.locator("#lx-library")).toBeVisible();
  await page.getByRole("button", { name: "Outline", exact: true }).click();
  await expect(page.locator("#lx-outline")).toBeHidden();
  await page.keyboard.press("Control+Shift+O");
  await expect(page.locator("#lx-outline")).toBeVisible();
});

test("dragging the library's edge resizes it, and the width is saved", async ({ page }) => {
  await launch(page);
  const width = () =>
    page.locator("#lx-library").evaluate((el) => Math.round(el.getBoundingClientRect().width));
  expect(await width()).toBe(280);
  const box = await page.locator(".resizer[data-for=library]").boundingBox();
  if (!box) throw new Error("no resizer");
  await page.mouse.move(box.x + 1, box.y + 300);
  await page.mouse.down();
  await page.mouse.move(box.x + 61, box.y + 300, { steps: 6 });
  await page.mouse.up();
  expect(await width()).toBe(340);
  // Saved after a pause, so a reload keeps it.
  await page.waitForTimeout(400);
  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  expect(await width()).toBe(340);
});
