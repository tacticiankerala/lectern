import { expect, test, type Page } from "@playwright/test";
import { openFixture } from "./util";

/** Opens a fixture through quick open, by `query`, and waits for its title. */
async function quickOpen(page: Page, query: string, title: string): Promise<void> {
  await page.keyboard.press("Control+p");
  await page.locator(".qo-input").fill(query);
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle(`${title} — Lectern`);
}

test("Ctrl+H lists the other recent files; a number opens one, and Back comes back", async ({
  page,
}) => {
  await openFixture(page, "prompts/writer.md");
  await quickOpen(page, "wide-table", "Wide table");
  await quickOpen(page, "long-lines", "Long lines");
  await quickOpen(page, "big-plan", "Big Plan");
  await page.keyboard.press("Control+h");
  const menu = page.getByRole("menu");
  await expect(menu.locator(".menu-row .menu-label")).toHaveText([
    "Long lines",
    "Wide table",
    "Writer prompt",
  ]);
  await expect(menu.locator(".menu-row .menu-digit")).toHaveText(["1", "2", "3"]);
  await expect(menu.locator(".menu-row .menu-detail")).toHaveText(["stress", "stress", "prompts"]);
  await expect(page.locator("#lx-recent-btn")).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("2");
  await expect(page).toHaveTitle("Wide table — Lectern");
  await expect(menu).toBeHidden();
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  // ↓ and Enter, from the header's clock.
  await page.locator("#lx-recent-btn").click();
  await expect(menu.locator(".menu-row .menu-label").first()).toHaveText("Wide table");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Long lines — Lectern");
});

test("a full menu fits the window: its rows scroll under All files, and the keys reach each", async ({
  page,
}) => {
  await page.goto("/?recent=20");
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await page.evaluate(() => {
    window.__fake.settingsElsewhere({ recentInMenu: 20 });
  });
  await page.keyboard.press("Control+h");
  const menu = page.getByRole("menu");
  const rows = menu.locator(".menu-row .menu-item");
  await expect(rows).toHaveCount(20);
  const box = await menu.boundingBox();
  const height = page.viewportSize()?.height ?? 0;
  expect((box?.y ?? 0) + (box?.height ?? Infinity)).toBeLessThanOrEqual(height - 8);
  const allFiles = menu.getByRole("menuitem", { name: /All files/ });
  await expect(allFiles).toBeInViewport({ ratio: 1 });
  await page.keyboard.press("End");
  await expect(allFiles).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(rows.last()).toBeFocused();
  // Scrolled to the end; a list scrolls in whole pixels, so a sliver of a pixel may stay hidden.
  await expect(rows.last()).toBeInViewport({ ratio: 0.99 });
  await page.keyboard.press("Home");
  await expect(rows.first()).toBeFocused();
  await expect(rows.first()).toBeInViewport({ ratio: 1 });
});
