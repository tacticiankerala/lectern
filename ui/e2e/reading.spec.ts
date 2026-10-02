import { expect, test, type Page } from "@playwright/test";
import { expectLandedOn, fixturePath, headingOffset, openFixture } from "./util";

const BIG_PLAN = "work/alpha/plans/2026-01-01-big-plan.md";

function scrollTop(page: Page): Promise<number> {
  return page.locator("#lx-doc-pane").evaluate((el) => el.scrollTop);
}

/** How far the block holding source line `line` sits below the pane's top. */
function lineOffset(page: Page, line: number): Promise<number | null> {
  return page.evaluate((line) => {
    const pane = document.getElementById("lx-doc-pane");
    const block = document.querySelector(`#lx-doc [data-sourcepos^="${String(line)}:"]`);
    if (!pane || !block) return null;
    return Math.round(block.getBoundingClientRect().top - pane.getBoundingClientRect().top);
  }, line);
}

/** The last heading at or above the pane's top, and how far above it sits. */
function topHeading(page: Page): Promise<{ id: string; offset: number }> {
  return page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    if (!pane) throw new Error("no pane");
    const top = pane.getBoundingClientRect().top;
    let found = { id: "", offset: 0 };
    for (const h of document.querySelectorAll<HTMLElement>("#lx-doc :is(h1,h2,h3,h4)[id]")) {
      const at = h.getBoundingClientRect().top - top;
      if (at > 1) break;
      found = { id: h.id, offset: Math.round(at) };
    }
    return found;
  });
}

/** Whether the current find match lies inside the pane. */
function currentMatchOnScreen(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    const current = CSS.highlights.get("find-current");
    if (!pane || !current) return false;
    const range = [...(current as unknown as Set<Range>)][0];
    if (!range) return false;
    const r = range.getBoundingClientRect();
    const p = pane.getBoundingClientRect();
    return r.top >= p.top && r.bottom <= p.bottom;
  });
}

test("Ctrl+F counts the matches, and Enter and Shift+Enter move through them", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  await page.keyboard.press("Control+f");
  const bar = page.getByRole("search", { name: "Find in page" });
  const input = bar.getByRole("textbox", { name: "Find" });
  await expect(input).toBeFocused();
  await page.keyboard.type("Step");
  const count = bar.locator(".find-count");
  await expect(count).toHaveText(/^1 \/ \d+$/);
  const total = Number((await count.textContent())?.split(" / ")[1]);
  expect(total).toBeGreaterThan(10);
  expect(await page.evaluate(() => CSS.highlights.get("find")?.size)).toBe(total);

  const before = await scrollTop(page);
  for (let i = 0; i < 3; i++) await page.keyboard.press("Enter");
  await expect(count).toHaveText(`4 / ${String(total)}`);
  await expect.poll(() => scrollTop(page)).toBeGreaterThan(before);
  await expect.poll(() => currentMatchOnScreen(page)).toBe(true);
  await page.keyboard.press("Shift+Enter");
  await expect(count).toHaveText(`3 / ${String(total)}`);
  // The buttons work too, and Shift+Enter from the first wraps to the last.
  await bar.getByRole("button", { name: "Previous match" }).click();
  await bar.getByRole("button", { name: "Previous match" }).click();
  await expect(count).toHaveText(`1 / ${String(total)}`);
  await input.press("Shift+Enter");
  await expect(count).toHaveText(`${String(total)} / ${String(total)}`);

  await input.press("Escape");
  await expect(bar).toBeHidden();
  expect(await page.evaluate(() => CSS.highlights.has("find"))).toBe(false);
  // Opened again, it keeps the query.
  await page.keyboard.press("Control+f");
  await expect(input).toHaveValue("Step");
  await expect(count).toHaveText(/ \/ \d+$/);
});

test("F3 and Shift+F3 move through the matches, and F3 opens the find bar", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  await page.keyboard.press("Control+f");
  const input = page.getByRole("textbox", { name: "Find" });
  await expect(input).toBeFocused();
  await page.keyboard.type("Step");
  const count = page.locator(".find-count");
  await expect(count).toHaveText(/^1 \/ \d+$/);
  await page.keyboard.press("F3");
  await page.keyboard.press("F3");
  await expect(count).toHaveText(/^3 \/ \d+$/);
  await page.keyboard.press("Shift+F3");
  await expect(count).toHaveText(/^2 \/ \d+$/);
  // From the document too, not only from the input.
  await page.locator("#lx-doc h1").click();
  await page.keyboard.press("F3");
  await expect(count).toHaveText(/^3 \/ \d+$/);
  await page.keyboard.press("Escape");
  await expect(input).toBeHidden();
  await page.keyboard.press("F3");
  await expect(input).toBeFocused();
  await expect(input).toHaveValue("Step");
});

test("find highlights at most 5000 matches", async ({ page }) => {
  await openFixture(page, BIG_PLAN);
  await page.keyboard.press("Control+f");
  await expect(page.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("e");
  await expect(page.locator(".find-count")).toHaveText(/^\d+ \/ 5000\+$/);
  expect(await page.evaluate(() => CSS.highlights.get("find")?.size)).toBe(5000);
});

test("full-text search opens a result scrolled to its line, with the find bar", async ({
  page,
}) => {
  await openFixture(page, "memory/index.md");
  await page.keyboard.press("Control+Shift+F");
  const panel = page.getByRole("dialog", { name: "Search" });
  const input = panel.getByRole("combobox", { name: "Search the library" });
  await expect(input).toBeFocused();
  await page.keyboard.type("Section 2");
  const file = panel.locator(".sp-file").first();
  await expect(file.locator(".sp-title")).toHaveText("Big Plan");
  await expect(file.locator(".sp-rel")).toHaveText(BIG_PLAN);
  const hit = panel.locator(".sp-hit", { hasText: "Section 2 covers" });
  await expect(hit.locator("mark")).toHaveText(["Section 2"]);
  await hit.click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await expect(panel).toBeHidden();
  await page.waitForTimeout(1000);
  const offset = await lineOffset(page, 225);
  expect(Math.abs((offset ?? Infinity) - 12)).toBeLessThanOrEqual(8);
  const bar = page.getByRole("search", { name: "Find in page" });
  await expect(bar.getByRole("textbox", { name: "Find" })).toHaveValue("Section 2");
  await expect(bar.locator(".find-count")).toHaveText(/^\d+ \/ \d+$/);
  expect(await currentMatchOnScreen(page)).toBe(true);
});

test("search results open from the keyboard", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.keyboard.press("Control+Shift+F");
  const panel = page.getByRole("dialog", { name: "Search" });
  await expect(panel.getByRole("combobox", { name: "Search the library" })).toBeFocused();
  await page.keyboard.type("Section 2");
  await expect(panel.locator(".sp-hit").first()).toBeVisible();
  // The first row, the file, is chosen; ↓ moves to its first hit.
  await expect(panel.locator('[aria-selected="true"]')).toHaveClass(/sp-file/);
  await page.keyboard.press("ArrowDown");
  await expect(panel.locator('[aria-selected="true"]')).toContainText("Section 2");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1000);
  const offset = await headingOffset(page, "section-2");
  expect(Math.abs((offset ?? Infinity) - 12)).toBeLessThanOrEqual(8);
});

test("a search hit only in Markdown syntax keeps the reader at its line", async ({ page }) => {
  // "zebra" shows in paragraphs 1 and 190; in 150 and 260 it is only in a link's destination.
  const path = fixturePath("notes/links.md");
  await openFixture(page, "memory/index.md");
  await page.evaluate((path) => {
    const source: string[] = [];
    const html: string[] = [];
    for (let n = 1; n <= 300; n++) {
      let text = `Filler paragraph ${String(n)}.`;
      let markup = text;
      if (n === 1) text = markup = "A zebra at the top.";
      if (n === 190) text = markup = "Another zebra below.";
      if (n === 150) {
        text = "See [the notes](notes/zebra.md).";
        markup = 'See <a href="#">the notes</a>.';
      }
      if (n === 260) {
        text = "Also [more notes](zebra/more.md).";
        markup = 'Also <a href="#">more notes</a>.';
      }
      const line = String(2 * n - 1);
      source.push(text, "");
      html.push(`<p data-sourcepos="${line}:1-${line}:${String(text.length)}">${markup}</p>`);
    }
    window.__fake.setDoc(path, html.join("\n"), source.join("\n"));
  }, path);
  const panel = page.getByRole("dialog", { name: "Search" });
  const count = page.locator(".find-count");
  for (const [line, hit] of [
    [299, "See [the notes]"],
    [519, "Also [more notes]"],
  ] as const) {
    await page.keyboard.press("Control+Shift+F");
    const input = panel.getByRole("combobox", { name: "Search the library" });
    await expect(input).toBeFocused();
    await input.fill("zebra");
    await panel.locator(".sp-hit", { hasText: hit }).click();
    await expect(page).toHaveTitle("links — Lectern");
    // The nearest shown match is current (paragraph 190 either way), and nothing scrolls to it.
    await expect(count).toHaveText("2 / 2");
    await page.waitForTimeout(1000);
    const offset = await lineOffset(page, line);
    expect(Math.abs((offset ?? Infinity) - 12), `line ${String(line)}`).toBeLessThanOrEqual(8);
    expect(await page.evaluate(() => CSS.highlights.get("find")?.size)).toBe(2);
    await page.keyboard.press("Escape");
  }
});

test("find opens a closed <details> and scrolls code sideways to show the match", async ({
  page,
}) => {
  const path = fixturePath("memory/index.md");
  await openFixture(page, "memory/index.md");
  await page.evaluate((path) => {
    window.__fake.setDoc(
      path,
      '<h1 id="hidden">Hidden</h1>' +
        "<details><summary>More</summary><p>The kiwi hides here.</p></details>" +
        '<div class="code-block" data-lang="text"><div class="code-head"><span class="code-lang">text</span>' +
        '<button type="button" class="code-copy">Copy</button></div>' +
        `<pre><code>${"x".repeat(400)} mango</code></pre></div>`,
    );
    window.__fake.emit("doc-changed", { path });
  }, path);
  await expect(page.locator("#lx-doc h1")).toHaveText("Hidden");
  await page.keyboard.press("Control+f");
  await expect(page.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("kiwi");
  await expect(page.locator(".find-count")).toHaveText("1 / 1");
  await expect(page.locator("#lx-doc details")).toHaveAttribute("open", "");

  await page.getByRole("textbox", { name: "Find" }).fill("mango");
  await expect(page.locator(".find-count")).toHaveText("1 / 1");
  await expect
    .poll(() =>
      page.evaluate(() => {
        const pre = document.querySelector<HTMLElement>("#lx-doc pre");
        const range = [...((CSS.highlights.get("find-current") ?? []) as Iterable<Range>)][0];
        if (!pre || !range) return "missing";
        const r = range.getBoundingClientRect();
        const box = pre.getBoundingClientRect();
        return pre.scrollLeft > 0 && r.left >= box.left && r.right <= box.right;
      }),
    )
    .toBe(true);
});

test("a tag click searches the library for the tag", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.locator("#lx-doc .tag", { hasText: "#nvim" }).click();
  const panel = page.getByRole("dialog", { name: "Search" });
  await expect(panel.getByRole("combobox", { name: "Search the library" })).toHaveValue("#nvim");
  await expect(panel.locator(".sp-file .sp-rel")).toHaveText(["memory/index.md"]);
  await expect(panel.locator(".sp-hit mark")).toHaveText(["#nvim"]);
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
});

test("the reading position survives leaving the document and reloading the window", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const pane = page.locator("#lx-doc-pane");
  const viewport = await pane.evaluate((el) => el.clientHeight);
  await pane.evaluate((el) => {
    el.scrollTop = 0.6 * (el.scrollHeight - el.clientHeight);
  });
  // The position is saved once scrolling has stopped for a moment.
  await page.waitForTimeout(800);
  const spot = await topHeading(page);
  expect(spot.id).not.toBe("");

  await page.keyboard.press("Control+p");
  await expect(page.locator(".quick-open input")).toBeFocused();
  await page.keyboard.type("memory index");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Memory index — Lectern");
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1100);
  const back = await headingOffset(page, spot.id);
  expect(Math.abs((back ?? Infinity) - spot.offset)).toBeLessThanOrEqual(viewport);

  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1100);
  const reopened = await headingOffset(page, spot.id);
  expect(Math.abs((reopened ?? Infinity) - spot.offset)).toBeLessThanOrEqual(viewport);
});

test("a document left at the top comes back at the top, its properties in view", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  await expect(page.locator("#lx-properties")).toBeVisible();
  await page.keyboard.press("Control+p");
  await expect(page.locator(".quick-open input")).toBeFocused();
  await page.keyboard.type("memory index");
  await page.keyboard.press("Enter");
  await expect(page).toHaveTitle("Memory index — Lectern");
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.waitForTimeout(1100);
  expect(await scrollTop(page)).toBe(0);
});

test("live reload keeps the place when the content above it shrinks, and find runs again", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  await page.keyboard.press("Control+f");
  await expect(page.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("Section");
  const count = page.locator(".find-count");
  await expect(count).toHaveText(/^1 \/ \d+$/);
  const totalBefore = Number((await count.textContent())?.split(" / ")[1]);
  await page
    .locator("#lx-outline li", { hasText: /^Section 12$/ })
    .locator("a")
    .click();
  await page.waitForTimeout(1100);
  await page.mouse.move(700, 500);
  await page.mouse.wheel(0, 300);
  await page.waitForTimeout(500);
  const spot = await topHeading(page);
  expect(spot.id).not.toBe("");

  await page.evaluate((path) => {
    const html = window.__fake.html(path);
    if (html === null) throw new Error("no big plan");
    const start = html.indexOf('<h2 id="section-1"');
    const end = html.indexOf('<h2 id="section-11"');
    window.__fake.setDoc(path, html.slice(0, start) + html.slice(end));
    window.__fake.emit("doc-changed", { path });
  }, fixturePath(BIG_PLAN));
  await expect(page.locator('#lx-doc [id="section-1"]')).toHaveCount(0);
  // A quiet note in the properties strip says so, not a toast.
  await expect(page.locator("#lx-properties .props-updated")).toHaveText(/^Updated \d{2}:\d{2}$/);
  await page.waitForTimeout(1000);
  const after = await headingOffset(page, spot.id);
  expect(Math.abs((after ?? Infinity) - spot.offset)).toBeLessThanOrEqual(40);

  // The find bar stayed open and counted again.
  await expect(page.getByRole("search", { name: "Find in page" })).toBeVisible();
  await expect
    .poll(async () => Number((await count.textContent())?.split(" / ")[1]))
    .toBeLessThan(totalBefore);
  await expect(page.locator("#lx-toasts .toast")).toHaveCount(0);
});

test("a document deleted while open shows not-found, and Back still works", async ({ page }) => {
  await openFixture(page, "memory/index.md");
  await page.locator('#lx-doc a.wikilink[data-anchor="Section 2"]').click();
  await expect(page).toHaveTitle("Big Plan — Lectern");
  await page.evaluate((path) => {
    window.__fake.remove(path);
    window.__fake.emit("doc-removed", { path });
  }, fixturePath(BIG_PLAN));
  const state = page.locator("#lx-doc .state-error");
  await expect(state).toContainText("This file isn't there");
  await expect(state.getByRole("button", { name: "Retry" })).toBeVisible();
  // Search for it: quick open, with the file's name.
  await state.getByRole("button", { name: "Search for it" }).click();
  await expect(page.locator(".quick-open input")).toHaveValue("2026-01-01-big-plan");
  await page.keyboard.press("Escape");

  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveTitle("Memory index — Lectern");
  await expect(page.locator("#lx-doc h1")).toHaveText("Memory index");
});

test("the last heading can reach the top, and progress ends where the text does", async ({
  page,
}) => {
  await openFixture(page, BIG_PLAN);
  const last = page.locator("#lx-outline li a").last();
  const id = await last.getAttribute("data-id");
  await last.click();
  await expectLandedOn(page, id ?? "");
  await expect(page.locator("#lx-outline li.active a")).toHaveAttribute("data-id", id ?? "");
  // The progress bar is full once the end of the text is on screen.
  await page.locator("#lx-doc-pane").evaluate((el) => {
    const doc = document.getElementById("lx-doc");
    if (!doc) return;
    el.scrollTop += doc.getBoundingClientRect().bottom - el.getBoundingClientRect().bottom;
  });
  await expect
    .poll(() =>
      page
        .locator("#lx-progress")
        .evaluate((el) => Number(/scaleX\((.*)\)/.exec(el.style.transform)?.[1] ?? 0)),
    )
    .toBeGreaterThan(0.995);
});
