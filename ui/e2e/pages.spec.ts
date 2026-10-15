// HTML pages in their sandboxed frame, with Lectern's bridge in them as the dev server serves them
// (dev/serve.mjs), as `lxpage` does: the outline, find, links, what each HTML pages setting lets a
// page do, the notice of what a page loads, the theme, keys from inside the frame (which only Rust
// sees, so the fake sends them on request), what a page loads from its own folder, and a page
// that navigates its own frame.
import { expect, test, type Page } from "@playwright/test";
import { fixturePath, launch, openFixture } from "./util";

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
  const src = await frame(page).getAttribute("src");
  await reported(page);
  await expect(outline(page)).toHaveText(["Weekly report", "Results", "Next steps"]);
  // Its own load, and its style sheet's and script's, are no navigation of its frame.
  await page.waitForTimeout(500);
  await expect(page.locator("#lx-toasts .toast")).toHaveCount(0);
  expect(await frame(page).getAttribute("src")).toBe(src);
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
  // The setting takes effect at once: the page opens again under it.
  await htmlPages(page, "offline");
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
    .toContain("external https://example.com/");
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
      // A page's script forging its bridge's `ready`, without the open's key.
      parent.postMessage(
        {
          lx: 1,
          type: "ready",
          headings: [{ level: 1, text: "Forged", id: "forged", offset: 0 }],
          blocks: [{ text: "Forged", headingLevel: 1 }],
          scroll: { fraction: 0, top: 0 },
          dark: false,
        },
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

/** The text of each range the page's bridge paints in a highlight layer, in the page. */
function painted(page: Page, layer: string) {
  return inside(page)
    .locator("body")
    .evaluate((_, name) => {
      const highlight = CSS.highlights.get(`lx-${name}`);
      return highlight ? [...highlight].map((range) => (range as Range).toString()) : [];
    }, layer);
}

test("a comment on words selected in a page is painted there, again when it reopens, and a click on them shows it", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  await reported(page);
  await page.locator("#lx-tab-comments").click();
  await expect(page.locator("#lx-comments-pane .comments-empty")).toHaveText(
    "No comments yet. Select text in the page to comment on it.",
  );
  // The reader selects the words inside the page.
  await inside(page)
    .locator("p", { hasText: "conversion dipped" })
    .evaluate((p) => {
      const text = p.firstChild as Text;
      const start = text.data.indexOf("conversion dipped in week 3");
      const range = document.createRange();
      range.setStart(text, start);
      range.setEnd(text, start + "conversion dipped in week 3".length);
      getSelection()?.removeAllRanges();
      getSelection()?.addRange(range);
    });
  const button = page.locator(".lx-sel-comment");
  await expect(button).toBeVisible();
  // By the selection's last line, over the page's frame.
  const words = await inside(page)
    .locator("body")
    .evaluate(() => {
      const box = getSelection()?.getRangeAt(0).getBoundingClientRect();
      return box ? { bottom: box.bottom, right: box.right } : null;
    });
  const frameBox = await frame(page).boundingBox();
  const buttonBox = await button.boundingBox();
  if (!words || !frameBox || !buttonBox) throw new Error("no boxes");
  expect(
    Math.abs(buttonBox.y + buttonBox.height / 2 - (frameBox.y + words.bottom - 9)),
  ).toBeLessThan(12);
  await button.click();
  await page.locator(".comment-editor textarea").fill("weekly, not daily");
  await page.keyboard.press("Control+Enter");
  const card = page.locator(".comment-card[data-id='1']");
  await expect(card.locator(".comment-quote")).toHaveText("conversion dipped in week 3");
  await expect.poll(() => painted(page, "comment")).toEqual(["conversion dipped in week 3"]);
  await expect.poll(() => painted(page, "comment-focus")).toEqual(["conversion dipped in week 3"]);

  // Changed on disk, the page loads again, reports its text, and its comment is painted again.
  const report = fixturePath("artifacts/report.html");
  const before = await frame(page).getAttribute("src");
  await page.evaluate((path) => {
    window.__fake.emit("doc-changed", { path });
  }, report);
  await expect(frame(page)).not.toHaveAttribute("src", before ?? "");
  await expect.poll(() => painted(page, "comment")).toEqual(["conversion dipped in week 3"]);
  // Opened again from elsewhere, nothing selected: a click on the words shows the comment.
  const open = (path: string) =>
    page.evaluate((path) => {
      window.__fake.emit("open-request", { path, t0Ms: null, folder: false });
    }, path);
  await open(fixturePath("artifacts/other.html"));
  await expect(page).toHaveTitle("other.html — Lectern");
  await open(report);
  await expect(page).toHaveTitle("report.html — Lectern");
  await expect.poll(() => painted(page, "comment")).toEqual(["conversion dipped in week 3"]);
  await expect(card).not.toHaveClass(/selected/);
  // Clicked by where the words are in their paragraph, which holds while the page settles.
  const paragraph = inside(page).locator("p", { hasText: "conversion dipped" });
  const position = await paragraph.evaluate((p) => {
    const range = [...(CSS.highlights.get("lx-comment") ?? [])][0] as Range | undefined;
    const words = range?.getBoundingClientRect();
    const box = p.getBoundingClientRect();
    return words
      ? { x: words.x - box.x + words.width / 2, y: words.y - box.y + words.height / 2 }
      : null;
  });
  if (!position) throw new Error("no marked words");
  await paragraph.click({ position });
  await expect(card).toHaveClass(/selected/);
});

test("Ctrl+Alt+M inside a page, as Rust sends it, comments on the words selected there", async ({
  page,
}) => {
  await openFixture(page, "artifacts/report.html");
  await reported(page);
  await inside(page)
    .locator("p", { hasText: "Rendered by script" })
    .evaluate((p) => {
      const range = document.createRange();
      range.selectNodeContents(p);
      getSelection()?.removeAllRanges();
      getSelection()?.addRange(range);
    });
  await expect(page.locator(".lx-sel-comment")).toBeVisible();
  await page.evaluate(() => {
    window.__fake.frameKey({ key: "m", code: "KeyM", ctrl: true, alt: true });
  });
  await page.locator(".comment-editor textarea").fill("Which script?");
  await page.keyboard.press("Control+Enter");
  await expect(page.locator(".comment-card[data-id='1'] .comment-quote")).toHaveText(
    "Rendered by script",
  );
  await expect.poll(() => painted(page, "comment")).toEqual(["Rendered by script"]);
});

test("a page's module script imports the module beside it, but can't read the data there", async ({
  page,
}) => {
  await openFixture(page, "artifacts/chart/index.html");
  await expect(inside(page).locator("p", { hasText: "Charted" })).toHaveText(
    "Charted 3 invented readings.",
  );
  await expect(inside(page).locator("p", { hasText: "tide log" })).toHaveText(
    "Couldn't read the tide log beside it.",
  );
  await htmlPages(page, "offline");
  await expect(frame(page)).toHaveAttribute("src", /lxmode=offline/);
  await expect(inside(page).locator("p", { hasText: "Charted" })).toHaveText(
    "Charted 3 invented readings.",
  );
});

/** The toasts on screen. */
function toasts(page: Page) {
  return page.locator("#lx-toasts .toast");
}

/** What the guard says in place of a page that keeps navigating away. */
function guard(page: Page) {
  return page.locator("#lx-doc .state");
}

test("a page that refreshes itself elsewhere is opened again, then let go", async ({ page }) => {
  const path = fixturePath("artifacts/refresh.html");
  await openFixture(page, "artifacts/refresh.html");
  await expect(toasts(page)).toContainText([
    "This page tried to open another page; open it from Lectern instead.",
  ]);
  await expect(guard(page).locator(".state-title")).toHaveText("This page keeps navigating away.");
  await expect(frame(page)).toHaveCount(0);
  await expect(page).toHaveTitle("refresh.html — Lectern");
  await guard(page).getByRole("button", { name: "Open in default app" }).click();
  await expect
    .poll(() => page.evaluate(() => window.__fake.followed.at(-1)))
    .toEqual({ kind: "file", target: path, line: null, anchor: null });
  // Nothing of the other page was taken as this one's text.
  expect(await page.evaluate((p) => window.__fake.coreText(p), path)).not.toContain(
    "second invented page",
  );
});

test("a page whose script sends its frame elsewhere shows again, and is let go the next time", async ({
  page,
}) => {
  const path = fixturePath("artifacts/redirect.html");
  await openFixture(page, "artifacts/redirect.html");
  expect((await reported(page)).text).toContain("Survey index");
  const first = await frame(page).getAttribute("src");
  await inside(page).getByRole("button", { name: "Next page" }).click();
  await expect(toasts(page)).toContainText([
    "This page tried to open another page; open it from Lectern instead.",
  ]);
  // The page Lectern meant, at a URL of a new open.
  await expect(frame(page)).not.toHaveAttribute("src", first ?? "");
  const token = (src: string | null) => /\/o\/([^/]+)\//.exec(src ?? "")?.[1];
  expect(token(await frame(page).getAttribute("src"))).not.toBe(token(first));
  await expect(inside(page).locator("h1")).toHaveText("Survey index");
  expect((await reported(page)).text).toContain("Survey index");
  await expect(guard(page)).toHaveCount(0);
  // Again, within 10 seconds: let go.
  await inside(page).getByRole("button", { name: "Next page" }).click();
  await expect(guard(page).locator(".state-title")).toHaveText("This page keeps navigating away.");
  await expect(guard(page).getByRole("button", { name: "Open in default app" })).toBeVisible();
  const text = await page.evaluate((p) => window.__fake.coreText(p), path);
  expect(text).toContain("Survey index");
  expect(text).not.toContain("second invented page");
});

test("a page that leaves for another before it has loaded is pulled back", async ({ page }) => {
  await openFixture(page, "artifacts/leaving.html");
  await expect(toasts(page)).toContainText([
    "This page tried to open another page; open it from Lectern instead.",
  ]);
  // It leaves again at once: let go.
  await expect(guard(page).locator(".state-title")).toHaveText("This page keeps navigating away.");
  await expect(page).toHaveTitle("leaving.html — Lectern");
});

test("a page that is ready, then leaves before its own load, is pulled back", async ({ page }) => {
  await openFixture(page, "artifacts/late-leaving.html");
  await reported(page);
  await expect(toasts(page)).toContainText([
    "This page tried to open another page; open it from Lectern instead.",
  ]);
  await expect(guard(page).locator(".state-title")).toHaveText("This page keeps navigating away.");
});

test("a page that fakes its own load, then leaves, is pulled back", async ({ page }) => {
  await openFixture(page, "artifacts/faking.html");
  await reported(page);
  await expect(toasts(page)).toContainText([
    "This page tried to open another page; open it from Lectern instead.",
  ]);
  await expect(guard(page).locator(".state-title")).toHaveText("This page keeps navigating away.");
});

test("a page that loads itself again is read again, with no toast and no guard", async ({
  page,
}) => {
  await openFixture(page, "artifacts/self-refresh.html");
  await reported(page);
  const src = await frame(page).getAttribute("src");
  const loadedAt = () =>
    inside(page)
      .locator("body")
      .evaluate(() => performance.timeOrigin);
  const first = await loadedAt();
  await expect.poll(loadedAt, { timeout: 5000 }).not.toBe(first);
  await page.waitForTimeout(2500);
  await expect(toasts(page)).toHaveCount(0);
  await expect(guard(page)).toHaveCount(0);
  expect(await frame(page).getAttribute("src")).toBe(src);
  await expect(outline(page)).toHaveText(["Live board"]);
});
