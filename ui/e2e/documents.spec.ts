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

test("a text file reads as its paragraphs, as written, in the code face", async ({ page }) => {
  await openFixture(page, "notes/readme.txt");
  await expect(page).toHaveTitle("readme.txt — Lectern");
  const blocks = page.locator("#lx-doc p.lx-plain");
  await expect(blocks).toHaveCount(3);
  expect(await blocks.evaluateAll((ps) => ps.map((p) => p.dataset.sourcepos))).toEqual([
    "1:1-2:48",
    "4:1-5:61",
    "7:1-9:300",
  ]);
  // Markup is text: the script never runs, and the stars stay stars.
  await expect(page.locator("#lx-doc script, #lx-doc em")).toHaveCount(0);
  await expect(blocks.nth(2)).toContainText(
    '*these stars stay stars*. <script>document.title = "Parsed";</script> stays text',
  );
  const look = await blocks.nth(2).evaluate((p) => {
    const probe = document.createElement("span");
    probe.style.fontFamily = "var(--code-font)";
    document.body.append(probe);
    const codeFont = getComputedStyle(probe).fontFamily;
    probe.remove();
    const doc = document.getElementById("lx-doc");
    const pane = document.getElementById("lx-doc-pane");
    const style = getComputedStyle(p);
    return {
      codeFont: style.fontFamily === codeFont,
      bodySize: doc !== null && style.fontSize === getComputedStyle(doc).fontSize,
      whiteSpace: style.whiteSpace,
      overflowWrap: style.overflowWrap,
      sideways: (pane?.scrollWidth ?? Infinity) - (pane?.clientWidth ?? 0),
      lines: Math.round(p.getBoundingClientRect().height / parseFloat(style.lineHeight)),
    };
  });
  expect(look).toMatchObject({
    codeFont: true,
    bodySize: true,
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
    sideways: 0,
  });
  // Three lines as written, the 300-character one wrapped onto more.
  expect(look.lines).toBeGreaterThan(3);
  await expect(page).toHaveTitle("readme.txt — Lectern");

  // Find reads it as it reads a note.
  await page.keyboard.press("Control+f");
  const bar = page.getByRole("search", { name: "Find in page" });
  await expect(bar.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("survey");
  await expect(bar.locator(".find-count")).toHaveText("1 / 2");
});
