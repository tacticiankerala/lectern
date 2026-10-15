// Text files, HTML pages and PDFs beside notes, in the fixture vault: the library marks them, quick
// open and links open them in Lectern, and search reads text files only.
import { expect, test, type Page } from "@playwright/test";
import { fixturePath, launch, openFixture } from "./util";

/** A sidebar row by its fixture path. */
function row(page: Page, rel: string) {
  return page.locator(
    `#lx-library .tree-row[data-path="${fixturePath(rel).replaceAll("\\", "\\\\")}"]`,
  );
}

test("the library marks text files, pages and PDFs with their type", async ({ page }) => {
  await launch(page);
  await row(page, "artifacts").locator(".tree-name").click();
  await expect(row(page, "artifacts/brief.pdf").locator(".tree-kind")).toHaveText("PDF");
  await expect(row(page, "artifacts/report.html").locator(".tree-kind")).toHaveText("HTML");
  await row(page, "notes").locator(".tree-name").click();
  await expect(row(page, "notes/readme.txt").locator(".tree-kind")).toHaveText("TXT");
  await expect(row(page, "notes/résumé notes.md")).toBeVisible();
  await expect(row(page, "notes/résumé notes.md").locator(".tree-kind")).toHaveCount(0);
  // The mark opens its file, as the name does.
  await row(page, "artifacts/brief.pdf").locator(".tree-kind").click();
  await expect(page).toHaveTitle("brief.pdf — Lectern");
  await expect(row(page, "artifacts/brief.pdf")).toHaveClass(/active/);
});

test("quick open finds a page by name", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.keyboard.press("Control+p");
  await expect(page.locator(".quick-open input")).toBeFocused();
  await page.keyboard.type("report");
  const first = page.locator(".quick-open .qo-item").first();
  await expect(first.locator(".qo-name")).toHaveText("report.html");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("report.html — Lectern");
  await expect(page.locator("#lx-doc .state")).toHaveText("Coming in this branch");
});

test("a link to a page opens it in Lectern, and Back returns", async ({ page }) => {
  await openFixture(page, "notes/résumé notes.md");
  const link = page.locator("#lx-doc a", { hasText: "report" });
  await expect(link).toHaveAttribute("data-kind", "doc");
  await link.click();
  await expect(page).toHaveTitle("report.html — Lectern");
  await expect(page.locator("#lx-doc .state")).toHaveText("Coming in this branch");
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Résumé notes — Lectern");
});

test("search reads text files, not pages", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.keyboard.press("Control+Shift+F");
  const panel = page.getByRole("dialog", { name: "Search" });
  await expect(panel.getByRole("combobox", { name: "Search the library" })).toBeFocused();
  await page.keyboard.type("lanternfish");
  await expect(panel.locator(".sp-file .sp-rel")).toHaveText(["notes/readme.txt"]);
});
