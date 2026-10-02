import { expect, test, type Page } from "@playwright/test";
import { THEMES, type ThemeDef } from "../src/themes";
import { headingOffset, openFixture } from "./util";

const DOCS = [
  { name: "readme-style", rel: "friends/readme-style.md" },
  { name: "big-plan", rel: "work/alpha/plans/2026-01-01-big-plan.md" },
];

/** `#rrggbb` as getComputedStyle reports it. */
function rgb(hex: string): string {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return `rgb(${String(r)}, ${String(g)}, ${String(b)})`;
}

async function pickTheme(page: Page, theme: ThemeDef): Promise<void> {
  await page.locator("#lx-reading-btn").click();
  const panel = page.getByRole("dialog", { name: "Reading settings" });
  // A click, not check(): the default dark theme's radio is already checked while in light mode.
  await panel.getByRole("radio", { name: theme.name, exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
  // Off the outline, so the screenshot shows no hover.
  await page.mouse.move(700, 18);
}

for (const doc of DOCS) {
  test(`every theme through the panel on ${doc.name}`, async ({ page }) => {
    // Taller than the other specs, so each screenshot shows more of the document.
    await page.setViewportSize({ width: 1400, height: 1600 });
    await openFixture(page, doc.rel);
    for (const theme of THEMES) {
      await pickTheme(page, theme);
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme.id);
      await expect
        .poll(() => page.evaluate(() => getComputedStyle(document.body).backgroundColor))
        .toBe(rgb(theme.bg));
      await page.screenshot({
        path: `test-results/themes/${theme.id}-${doc.name}.png`,
        fullPage: true,
      });
    }
  });
}

test("Ctrl+= increases --font-size", async ({ page }) => {
  await openFixture(page, DOCS[1]?.rel ?? "");
  const fontSize = () =>
    page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue("--font-size").trim(),
    );
  expect(await fontSize()).toBe("18px");
  await page.keyboard.press("Control+Equal");
  expect(await fontSize()).toBe("19px");
  await page.keyboard.press("Control+Minus");
  await page.keyboard.press("Control+Minus");
  expect(await fontSize()).toBe("17px");
  await page.keyboard.press("Control+Digit0");
  expect(await fontSize()).toBe("18px");
});

test("a bigger font keeps the reader on the same passage", async ({ page }) => {
  await openFixture(page, DOCS[1]?.rel ?? "");
  await page
    .locator("#lx-outline li", { hasText: /^Section 5$/ })
    .locator("a")
    .click();
  await page.waitForTimeout(1000);
  const before = (await headingOffset(page, "section-5")) ?? NaN;
  for (let i = 0; i < 3; i++) await page.keyboard.press("Control+Equal");
  await page.waitForTimeout(1000);
  const after = (await headingOffset(page, "section-5")) ?? NaN;
  expect(Math.abs(after - before)).toBeLessThan(12);
});

test("F11 hides the chrome for focus mode and Esc brings it back", async ({ page }) => {
  await openFixture(page, DOCS[1]?.rel ?? "");
  await page.keyboard.press("F11");
  for (const id of ["lx-header", "lx-library", "lx-outline", "lx-progress"]) {
    await expect(page.locator(`#${id}`), id).toBeHidden();
  }
  await expect(page.locator("#lx-doc h1")).toBeVisible();
  // At the top of the document, focus mode stays there: the properties strip still shows.
  await expect(page.locator("#lx-properties")).toBeInViewport();
  expect(await page.evaluate(() => window.__fake.fullscreen)).toEqual([true]);
  await page.keyboard.press("Escape");
  await expect(page.locator("#lx-header")).toBeVisible();
  await expect(page.locator("#lx-outline")).toBeVisible();
  expect(await page.evaluate(() => window.__fake.fullscreen)).toEqual([true, false]);
});

test("focus mode keeps the reader's place through the full-screen resize", async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await openFixture(page, DOCS[1]?.rel ?? "");
  // Full width, so the text reflows with the window.
  await page.locator("#lx-reading-btn").click();
  await page.getByRole("checkbox", { name: "Full width" }).check();
  await page.keyboard.press("Escape");
  // Section 5 just below the top (the outline is hidden at this width).
  for (let i = 0; i < 3; i++) {
    await page.evaluate(() => {
      const pane = document.getElementById("lx-doc-pane");
      const heading = document.querySelector('#lx-doc [id="section-5"]');
      if (pane && heading) {
        pane.scrollTop +=
          heading.getBoundingClientRect().top - pane.getBoundingClientRect().top - 12;
      }
    });
    await page.waitForTimeout(200);
  }
  const before = (await headingOffset(page, "section-5")) ?? NaN;
  // The native full-screen resize lands a little after the toggle; so does the way back.
  await page.keyboard.press("F11");
  await page.waitForTimeout(150);
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.waitForTimeout(1000);
  const full = (await headingOffset(page, "section-5")) ?? NaN;
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.waitForTimeout(1000);
  const back = (await headingOffset(page, "section-5")) ?? NaN;
  expect(Math.abs(full - before), `full screen: ${String(before)} -> ${String(full)}`).toBeLessThan(
    4,
  );
  expect(Math.abs(back - before), `back: ${String(before)} -> ${String(back)}`).toBeLessThan(4);
});

test("system mode follows the OS between light and dark, title bar included", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await openFixture(page, DOCS[1]?.rel ?? "");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "paper");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "graphite");
  expect(await page.evaluate(() => window.__fake.chromeColors)).toEqual([
    ["#f8f5ee", "#2b2a27", false],
    ["#1e1f22", "#d7d8db", true],
  ]);
});

test("narrow windows drop the outline, then the library", async ({ page }) => {
  await openFixture(page, DOCS[1]?.rel ?? "");
  await expect(page.locator("#lx-outline")).toBeVisible();
  await page.setViewportSize({ width: 1000, height: 800 });
  await expect(page.locator("#lx-outline")).toBeHidden();
  await expect(page.locator("#lx-library")).toBeVisible();
  await page.setViewportSize({ width: 700, height: 800 });
  await expect(page.locator("#lx-library")).toBeHidden();
  await expect(page.locator("#lx-doc h1")).toBeVisible();
});

test("only the selected bundled faces load", async ({ page }) => {
  await openFixture(page, DOCS[1]?.rel ?? "");
  const fonts = () =>
    page.evaluate(() =>
      performance
        .getEntriesByType("resource")
        .map((e) => e.name.slice(e.name.lastIndexOf("/") + 1))
        .filter((name) => name.endsWith(".woff2"))
        .sort(),
    );
  // Segoe UI Variable is the system's; JetBrains Mono, the default code font, is bundled (its
  // italic loads too, for the code comments).
  await expect.poll(fonts).toContain("JetBrainsMono-Variable.woff2");
  expect((await fonts()).filter((name) => !name.startsWith("JetBrainsMono-"))).toEqual([]);
  await page.locator("#lx-reading-btn").click();
  await page.getByRole("combobox", { name: "Text font" }).selectOption("Literata");
  await expect.poll(fonts).toContain("Literata-Variable.woff2");
  expect(await fonts()).not.toContain("Inter-Variable.woff2");
});
