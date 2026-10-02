import { expect, test, type Page } from "@playwright/test";
import { launch } from "./util";

async function chooseFromMore(page: Page, label: string): Promise<void> {
  await page.locator("#lx-more-btn").click();
  await page.locator(".menu-item", { hasText: label }).click();
}

test("Check for updates shows an update as a pill in the header that installs it", async ({
  page,
}) => {
  await launch(page);
  await page.evaluate(() => {
    window.__fake.update = { version: "0.2.0", notes: null, portable: false };
  });
  await chooseFromMore(page, "Check for updates");
  const pill = page.locator("#lx-header-actions .update-pill");
  await expect(pill).toHaveText("Update to v0.2.0");
  await expect(page.locator(".toast", { hasText: "Lectern 0.2.0 is available." })).toBeVisible();
  // It sits before the header's buttons, on one line.
  const [pillBox, readingBox] = await Promise.all([
    pill.boundingBox(),
    page.locator("#lx-reading-btn").boundingBox(),
  ]);
  expect(pillBox && readingBox && pillBox.x + pillBox.width <= readingBox.x).toBe(true);
  expect(pillBox?.height).toBeLessThanOrEqual(26);
  await pill.click();
  expect(await page.evaluate(() => window.__fake.updateCalls)).toEqual(["check", "install"]);
});

test("Check for updates says when Lectern is up to date", async ({ page }) => {
  await launch(page);
  await chooseFromMore(page, "Check for updates");
  await expect(page.locator(".toast", { hasText: "You're up to date." })).toBeVisible();
  await expect(page.locator(".update-pill")).toHaveCount(0);
});

test("About shows the version and licences, and closes with Escape", async ({ page }) => {
  await launch(page);
  await chooseFromMore(page, "About Lectern");
  const about = page.getByRole("dialog", { name: "Lectern" });
  await expect(about).toBeVisible();
  await expect(about).toContainText("Version 0.0.0-fake");
  await expect(about).toContainText("MIT licence");
  await expect(about).toContainText("SIL Open Font License 1.1");
  await expect(about.locator(".about-close")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(about).toBeHidden();
});
