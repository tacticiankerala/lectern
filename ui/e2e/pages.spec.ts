// HTML pages in their sandboxed frame, with Lectern's bridge in them as the dev server serves them
// (dev/serve.mjs), as `lxpage` does: the outline, find, links, what each HTML pages setting lets a
// page do, the notice of what a page loads, the theme, and keys from inside the frame (which only
// Rust sees, so the fake sends them on request).
import { expect, test, type Page } from "@playwright/test";
import { launch, openFixture } from "./util";

/** The frame a page shows in. */
function frame(page: Page) {
  return page.locator("#lx-doc-pane > iframe.lx-frame");
}

/** The page inside its frame. */
function inside(page: Page) {
  return page.frameLocator("#lx-doc-pane > iframe.lx-frame");
}

/** What the page on screen has reported through its bridge, once it has. */
async function reported(page: Page) {
  await expect.poll(() => page.evaluate(() => window.__lxPage()?.ready ?? false)).toBe(true);
  const state = await page.evaluate(() => window.__lxPage());
  if (!state) throw new Error("no page");
  return state;
}

function outline(page: Page) {
  return page.locator("#lx-outline-pane a");
}

function notice(page: Page) {
  return page.locator("#lx-doc-pane > .page-notice");
}

/** Changes the HTML pages setting, as another window would. */
async function htmlPages(page: Page, mode: "full" | "offline" | "static"): Promise<void> {
  await page.evaluate((htmlPages) => {
    window.__fake.settingsElsewhere({ htmlPages });
  }, mode);
}

test("a page shows in its sandbox, styled by its own sheet, its headings in the outline", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  await expect(frame(page)).toHaveAttribute("sandbox", "allow-scripts");
  await reported(page);
  await expect(outline(page)).toHaveText(["Weekly report", "Results", "Next steps"]);
  await expect(inside(page).locator("h1")).toHaveCSS("color", "rgb(0, 110, 110)");
  // The second heading: the frame scrolls it to the top, and the bridge says how far that is.
  await outline(page).nth(1).click();
  await expect
    .poll(() => page.evaluate(() => window.__lxPage()?.fraction ?? 0))
    .toBeGreaterThan(0.2);
  const top = await inside(page)
    .locator("h2")
    .first()
    .evaluate((h) => h.getBoundingClientRect().top);
  expect(Math.abs(top - 12)).toBeLessThanOrEqual(8);
});

test("its script runs, and find finds what it wrote", async ({ page }) => {
  await openFixture(page, "artifacts/report.html");
  const state = await reported(page);
  expect(state.text).toContain("Rendered by script");
  await page.keyboard.press("Control+f");
  await expect(page.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("rendered by");
  await expect(page.locator(".find-count")).toHaveText("1 / 1");
});

test("with no page scripts, only the bridge runs: the outline works, and nothing is noticed", async ({
  page,
}) => {
  await launch(page);
  await htmlPages(page, "static");
  await page.evaluate(() => {
    window.__fake.emit("open-request", {
      path: "C:\\Fixtures\\vault\\artifacts\\report.html",
      t0Ms: null,
      folder: false,
    });
  });
  await expect(frame(page)).toHaveAttribute("src", /lxmode=static/);
  const state = await reported(page);
  expect(state.text).not.toContain("Rendered by script");
  await expect(inside(page).locator("p", { hasText: "Rendered by script" })).toHaveCount(0);
  await expect(outline(page)).toHaveText(["Weekly report", "Results", "Next steps"]);
  await page.waitForTimeout(300);
  await expect(notice(page)).toBeHidden();
});

test("a page that loads from the web says from where, or, offline, that it was blocked", async ({
  page,
}) => {
  await openFixture(page, "artifacts/remote.html");
  await expect(notice(page)).toBeVisible();
  await expect(notice(page).locator(".page-notice-text")).toHaveText(
    "This page loads from cdn.example.test",
  );
  await htmlPages(page, "offline");
  await page.keyboard.press("F5");
  await expect(frame(page)).toHaveAttribute("src", /lxmode=offline/);
  await expect(notice(page).locator(".page-notice-text")).toHaveText(
    "Blocked loads from cdn.example.test",
  );
  await notice(page).getByRole("button", { name: "Change in Preferences" }).click();
  await expect(page.getByRole("dialog", { name: "Preferences" })).toBeVisible();
  await expect(page.locator('input[name="lx-html-pages"][value="offline"]')).toBeChecked();
  // The notice goes for focus mode, and × puts it away for this open.
  await page.keyboard.press("Escape");
  await page.keyboard.press("F11");
  await expect(notice(page)).toBeHidden();
  await page.keyboard.press("F11");
  await expect(notice(page)).toBeVisible();
  await notice(page).getByRole("button", { name: "Dismiss" }).click();
  await expect(notice(page)).toBeHidden();
});

test("a link in a page opens the page it leads to, and Back returns; a web link goes out", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  await reported(page);
  await inside(page).getByRole("link", { name: "other page" }).click();
  await expect(page).toHaveTitle("other.html — Lectern");
  // Alt+Left, pressed inside the frame, comes from Rust.
  await page.evaluate(() => {
    window.__fake.frameKey({ key: "ArrowLeft", code: "ArrowLeft", alt: true });
  });
  await expect(page).toHaveTitle("report.html — Lectern");
  await reported(page);
  const before = await frame(page).getAttribute("src");
  await inside(page).getByRole("link", { name: "survey site" }).click();
  await expect
    .poll(() => page.evaluate(() => window.__fake.followed.map((t) => `${t.kind} ${t.target}`)))
    .toContain("external https://example.com");
  await expect(page).toHaveTitle("report.html — Lectern");
  expect(await frame(page).getAttribute("src")).toBe(before);
  await expect(inside(page).locator("h1")).toHaveText("Weekly report");
});

test("a message from another window, or one over 2 MB, changes nothing and throws nothing", async ({
  page,
}) => {
  const errors: Error[] = [];
  page.on("pageerror", (e) => errors.push(e));
  await openFixture(page, "artifacts/report.html");
  const state = await reported(page);
  await page.evaluate(() => {
    window.postMessage(
      {
        lx: 1,
        type: "blocks",
        headings: [{ level: 1, text: "Forged", id: "forged", offset: 0 }],
        blocks: [{ text: "Forged", headingLevel: 1 }],
      },
      "*",
    );
  });
  await inside(page)
    .locator("body")
    .evaluate(() => {
      parent.postMessage(
        { lx: 1, type: "blocks", headings: [], blocks: [{ text: "x".repeat(3 * 1024 * 1024) }] },
        "*",
      );
    });
  await page.waitForTimeout(300);
  await expect(outline(page)).toHaveText(["Weekly report", "Results", "Next steps"]);
  expect((await reported(page)).text).toBe(state.text);
  expect(errors).toEqual([]);
});

test("Lectern's dark theme reaches the page's frame, and the bridge reports what the page sees", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  expect((await reported(page)).dark).toBe(false);
  await expect(frame(page)).toHaveCSS("color-scheme", "light");
  await page.keyboard.press("Control+Shift+T");
  await expect(frame(page)).toHaveCSS("color-scheme", "dark");
  // WebView2 hands the frame's colour scheme to the page inside it (the 2026-10-15 spike); the
  // Chromium Playwright runs doesn't yet, so here the page sees dark through the system's.
  await page.emulateMedia({ colorScheme: "dark" });
  await expect.poll(() => page.evaluate(() => window.__lxPage()?.dark)).toBe(true);
  await expect(inside(page).locator("body")).toHaveCSS("background-color", "rgb(34, 34, 34)");
});

test("Lectern's shortcuts pressed inside a page, as Rust sends them, run as they do outside", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  await reported(page);
  await page.evaluate(() => {
    window.__fake.frameKey({ key: "F11", code: "F11" });
  });
  await expect(page.locator("body")).toHaveClass(/focus/);
  await page.evaluate(() => {
    window.__fake.frameKey({ key: "Escape", code: "Escape" });
  });
  await expect(page.locator("body")).not.toHaveClass(/focus/);
});
