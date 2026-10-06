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

test("folder status badges show by default and hide once turned off", async ({ page }) => {
  await launch(page);
  await row(page, "work").locator(".tree-name").click();
  const alpha = row(page, "work/alpha");
  await expect(alpha.locator(".badge.status-blocked")).toHaveText("blocked");

  await page.keyboard.press("Control+,");
  const prefs = page.getByRole("dialog", { name: "Preferences" });
  const toggle = prefs.getByRole("checkbox", { name: "Show folder status badges" });
  await expect(toggle).toBeChecked();
  await expect(prefs).toContainText(
    "Shows a badge on folders whose README.md has a status: field in its frontmatter, e.g. status: active. Green for active, red for blocked, grey for parked, blue for done; other values appear neutral.",
  );
  await toggle.uncheck();
  await expect(page.locator("#lx-library .badge")).toHaveCount(0);
  await expect(alpha).toBeVisible();
  // Saved: a reload keeps them hidden, and turning it on brings them back.
  await page.keyboard.press("Escape");
  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await expect(row(page, "work/alpha")).toBeVisible();
  await expect(page.locator("#lx-library .badge")).toHaveCount(0);
  await page.keyboard.press("Control+,");
  await prefs.getByRole("checkbox", { name: "Show folder status badges" }).check();
  await expect(row(page, "work/alpha").locator(".badge.status-blocked")).toHaveText("blocked");
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

/** The breadcrumb chooser's parts. */
function chooser(page: Page) {
  const panel = page.locator(".crumb-chooser");
  return {
    panel,
    where: panel.locator(".cc-path"),
    filter: panel.getByRole("combobox", { name: "Filter" }),
    names: panel.locator(".cc-item .cc-name"),
    chosen: panel.locator('.cc-item[aria-selected="true"] .cc-name'),
  };
}

test("the breadcrumb chooser lists the folder of each kind of crumb", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const crumbs = page.locator("#lx-breadcrumbs .crumb");
  await expect(crumbs).toHaveText(["vault", "work", "alpha", "plans", "2026-01-01-big-plan.md"]);
  const cc = chooser(page);
  // The root's crumb: the root's children, folders first.
  await crumbs.nth(0).click();
  await expect(cc.panel).toBeVisible();
  await expect(cc.filter).toBeFocused();
  await expect(cc.where).toHaveText("vault");
  await expect(cc.names).toHaveText([
    "archive",
    "friends",
    "memory",
    "notes",
    "prompts",
    "stress",
    "work",
    "README.md",
  ]);
  await expect(crumbs.nth(0)).toHaveAttribute("aria-expanded", "true");
  // A folder's crumb: that folder, its README folders badged.
  await crumbs.nth(1).click();
  await expect(cc.where).toHaveText("vault / work");
  await expect(cc.names).toHaveText(["alpha"]);
  await expect(cc.panel.locator(".cc-item .badge.status-blocked")).toHaveText("blocked");
  await expect(crumbs.nth(0)).toHaveAttribute("aria-expanded", "false");
  // The file's crumb: its folder, the file marked and chosen.
  await crumbs.nth(4).click();
  await expect(cc.where).toHaveText("vault / work / alpha / plans");
  await expect(cc.names).toHaveText(["2026-01-01-big-plan.md"]);
  await expect(cc.chosen).toHaveText("2026-01-01-big-plan.md");
  await expect(cc.panel.locator(".cc-item.current")).toHaveCount(1);
  // A second click on the crumb closes it.
  await crumbs.nth(4).click();
  await expect(cc.panel).toBeHidden();
  // Ctrl+Shift+. opens it on the file's crumb.
  await page.keyboard.press("Control+Shift+Period");
  await expect(cc.panel).toBeVisible();
  await expect(cc.chosen).toHaveText("2026-01-01-big-plan.md");
});

test("the breadcrumb chooser goes into folders, staying open, and opens a file", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const cc = chooser(page);
  const up = cc.panel.getByRole("button", { name: "Up" });
  await page.locator("#lx-breadcrumbs .crumb").first().click();
  await expect(cc.filter).toBeFocused();
  // Filter, then Enter on a folder: in it, still open.
  await page.keyboard.type("WOR");
  await expect(cc.names).toHaveText(["work"]);
  await page.keyboard.press("Enter");
  await expect(cc.panel).toBeVisible();
  await expect(cc.where).toHaveText("vault / work");
  await expect(cc.filter).toHaveValue("");
  // → goes in; ← and Backspace go up, choosing the folder just left.
  await page.keyboard.press("ArrowRight");
  await expect(cc.where).toHaveText("vault / work / alpha");
  await expect(cc.names).toHaveText(["notes", "plans", "README.md"]);
  await page.keyboard.press("ArrowLeft");
  await expect(cc.where).toHaveText("vault / work");
  await expect(cc.chosen).toHaveText("alpha");
  await page.keyboard.press("Backspace");
  await expect(cc.where).toHaveText("vault");
  await expect(cc.chosen).toHaveText("work");
  await expect(up).toBeDisabled();
  // The mouse: a folder goes in, Up goes up, and the filter keeps the keyboard.
  await cc.names.filter({ hasText: "work" }).click();
  await cc.names.filter({ hasText: "alpha" }).click();
  await expect(cc.where).toHaveText("vault / work / alpha");
  await up.click();
  await expect(cc.where).toHaveText("vault / work");
  await expect(cc.filter).toBeFocused();
  await cc.names.filter({ hasText: "alpha" }).click();
  // ↓ to the README and Enter: it opens, the chooser closes, and Back returns.
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(cc.chosen).toHaveText("README.md");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Alpha — Lectern");
  await expect(cc.panel).toBeHidden();
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Big Plan — Lectern");
});

test("Esc or a click outside closes the breadcrumb chooser, and the library is left alone", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const plans = row(page, "work/alpha/plans");
  await plans.locator(".tree-chevron").click();
  await expect(plans).toHaveAttribute("aria-expanded", "false");
  const crumb = page.locator("#lx-breadcrumbs .crumb").nth(3);
  const cc = chooser(page);
  await crumb.click();
  await expect(cc.names).toHaveText(["2026-01-01-big-plan.md"]);
  await page.keyboard.press("ArrowLeft");
  await expect(cc.where).toHaveText("vault / work / alpha");
  await page.keyboard.press("Escape");
  await expect(cc.panel).toBeHidden();
  await expect(crumb).toBeFocused();
  await expect(crumb).toHaveAttribute("aria-expanded", "false");
  // Nothing was revealed or expanded in the library.
  await expect(plans).toHaveAttribute("aria-expanded", "false");
  await expect(row(page, BIG_PLAN)).toHaveCount(0);
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await crumb.click();
  await expect(cc.panel).toBeVisible();
  await page.locator("#lx-doc h1").first().click();
  await expect(cc.panel).toBeHidden();
  await expect(plans).toHaveAttribute("aria-expanded", "false");
});

test("the breadcrumb chooser and the other overlays close each other", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const cc = chooser(page);
  await page.locator("#lx-breadcrumbs .crumb").first().click();
  await expect(cc.panel).toBeVisible();
  // Ctrl+F gets through the filter, and the find bar closes the chooser.
  await page.keyboard.press("Control+f");
  await expect(page.locator(".find-bar")).toBeVisible();
  await expect(cc.panel).toBeHidden();
  await page.keyboard.press("Escape");
  // Opening the chooser closes quick open; opening quick open closes the chooser.
  await page.keyboard.press("Control+p");
  await expect(page.locator(".quick-open")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.locator("#lx-breadcrumbs .crumb").first().focus();
  await page.keyboard.press("Control+Shift+Period");
  await expect(cc.panel).toBeVisible();
  await page.locator("#lx-breadcrumbs .crumb").first().focus();
  await page.keyboard.press("Control+p");
  await expect(page.locator(".quick-open")).toBeVisible();
  await expect(cc.panel).toBeHidden();
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

test("the library button collapses the library, giving its width to the document, and a reload keeps it", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const button = page.getByRole("button", { name: "Library", exact: true });
  // At the far left of the header, before back and forward.
  await expect(page.locator("#lx-header > :first-child")).toHaveId("lx-library-btn");
  await expect(button).toHaveAttribute("aria-pressed", "true");
  await expect(button).toHaveAttribute("title", "Hide library (Ctrl+B)");
  const paneWidth = () =>
    page.locator("#lx-doc-pane").evaluate((el) => Math.round(el.getBoundingClientRect().width));
  const before = await paneWidth();
  await button.click();
  await expect(page.locator("#lx-library")).toBeHidden();
  await expect(page.locator(".resizer[data-for=library]")).toBeHidden();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await expect(button).toHaveAttribute("title", "Show library (Ctrl+B)");
  await expect.poll(paneWidth).toBe(before + 280);
  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await expect(page.locator("#lx-library")).toBeHidden();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await button.click();
  await expect(page.locator("#lx-library")).toBeVisible();
  await expect(button).toHaveAttribute("aria-pressed", "true");
  await expect.poll(paneWidth).toBe(before);
  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await expect(page.locator("#lx-library")).toBeVisible();
});

test("in a narrow window the library button shows the library on purpose", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  const button = page.getByRole("button", { name: "Library", exact: true });
  await page.setViewportSize({ width: 700, height: 800 });
  await expect(page.locator("#lx-library")).toBeHidden();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await button.click();
  await expect(page.locator("#lx-library")).toBeVisible();
  await expect(button).toHaveAttribute("aria-pressed", "true");
  await button.click();
  await expect(page.locator("#lx-library")).toBeHidden();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  // Wide again, the setting still shows it.
  await page.setViewportSize({ width: 1400, height: 900 });
  await expect(page.locator("#lx-library")).toBeVisible();
  await expect(button).toHaveAttribute("aria-pressed", "true");
});

test("with no library roots, the library offers to add a folder", async ({ page }) => {
  await launch(page);
  const hint = page.locator("#lx-library .lib-empty");
  await expect(hint).toHaveCount(0);
  for (const name of ["vault", "notes"]) {
    await page.locator(".lib-root-head", { hasText: name }).click({ button: "right" });
    await page.getByRole("menuitem", { name: "Remove from library" }).click();
  }
  await expect(page.locator("#lx-library .lib-root")).toHaveCount(0);
  await expect(hint).toContainText("Add a folder to build your library");
  await expect(hint).toContainText("Lectern indexes the Markdown files in folders you add.");
  await expect(hint.getByRole("button", { name: "Add folder…" })).toBeVisible();
  // It goes as soon as there is a root.
  await page.evaluate(() => {
    window.__fake.drop(["D:\\Elsewhere\\Notes"]);
  });
  await expect(page.locator("#lx-library .lib-root")).toHaveCount(1);
  await expect(hint).toHaveCount(0);
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
