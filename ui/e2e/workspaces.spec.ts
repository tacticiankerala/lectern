// Workspaces on the fake backend: Studio over the fixture library, with `?garden` a second one,
// Garden, with no folders, and with `?many=<n>` n more (dev/main-fake.ts). Opening one here
// reloads the page, and the fake's model survives the reload in sessionStorage.
import { expect, test, type Page } from "@playwright/test";
import { VAULT, fixturePath } from "./util";

const ALPHA = "work/alpha/README.md";

/** Loads the app with the fake seeded by `query`, and waits for startup. */
async function start(page: Page, query: string): Promise<void> {
  await page.goto(`/?${query}`);
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
}

/** Runs `act`, which turns the window to another workspace, and waits for the page to reload. */
async function reloadsAfter(page: Page, act: () => Promise<void>): Promise<void> {
  const reloaded = page.waitForEvent("load");
  await act();
  await reloaded;
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
}

/** Marks the page, so a test can tell it was not reloaded. */
async function mark(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.body.dataset.marked = "";
  });
}

const chip = (page: Page) => page.locator("#lx-workspace-btn");
const dropdown = (page: Page) => page.getByRole("dialog", { name: "Workspaces" });
const rows = (page: Page) => dropdown(page).locator(".ws-open");

test("the chip names the workspace, and Enter on another turns the window to it", async ({
  page,
}) => {
  await start(page, "garden");
  await expect(chip(page)).toHaveText("Studio▾");
  await expect(page).toHaveTitle("Studio — Lectern");
  // At the left of the header: after back and forward, before the breadcrumbs.
  await expect(page.locator("#lx-history-nav + #lx-workspace-btn + #lx-breadcrumbs")).toHaveCount(
    1,
  );
  await chip(page).click();
  await expect(rows(page).locator(".ws-name")).toHaveText(["Studio", "Garden"]);
  await expect(rows(page).locator(".ws-hint")).toHaveText(["vault +1", ""]);
  // Open in new window shows at rest, on Garden's row alone.
  const second = dropdown(page).getByRole("button", { name: "Open in new window" });
  await expect(second).toHaveCount(1);
  await expect(second).toBeVisible();
  await expect(rows(page).first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(rows(page).nth(1)).toBeFocused();
  await reloadsAfter(page, () => page.keyboard.press("Enter"));

  await expect(chip(page)).toHaveText("Garden▾");
  await expect(page).toHaveTitle("Garden — Lectern");
  await expect(page.locator("#lx-library .lib-root")).toHaveCount(0);
  // No folders yet: the welcome screen offers to add one.
  await expect(page.locator("#lx-doc .welcome-add-folder")).toBeFocused();
  expect(await page.evaluate(() => window.__fake.workspaces().map((ws) => ws.open))).toEqual([
    false,
    true,
  ]);
});

test("Ctrl+Enter opens a workspace in a new window, and this one stays", async ({ page }) => {
  await start(page, "garden");
  await mark(page);
  await chip(page).click();
  // The dropdown loads on first use.
  await expect(rows(page).first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Control+Enter");
  await expect(dropdown(page)).toBeHidden();
  await expect
    .poll(() => page.evaluate(() => window.__fake.windowCalls))
    .toEqual([{ call: "openWorkspace", id: "w2", where: "newWindow" }]);
  await expect(chip(page)).toHaveText("Studio▾");
  await expect(page.locator("body[data-marked]")).toHaveCount(1);
});

test("a workspace open in another window says so, and brings that window forward", async ({
  page,
}) => {
  await start(page, "garden=open");
  await mark(page);
  await chip(page).click();
  await expect(rows(page).locator(".ws-hint")).toHaveText(["vault +1", ""]);
  await expect(rows(page).nth(1).locator(".ws-switch")).toHaveText("Switch to window");
  await expect(dropdown(page).locator(".ws-new-window")).toHaveCount(0);
  await rows(page).nth(1).click();
  await expect
    .poll(() => page.evaluate(() => window.__fake.windowCalls))
    .toEqual([{ call: "openWorkspace", id: "w2", where: "newWindow" }]);
  await expect(chip(page)).toHaveText("Studio▾");
  await expect(page.locator("body[data-marked]")).toHaveCount(1);
});

test("a new workspace takes the suggested name, selected, and Enter opens it here", async ({
  page,
}) => {
  await start(page, "garden");
  await chip(page).click();
  await dropdown(page).getByRole("button", { name: "New workspace" }).click();
  const name = dropdown(page).getByRole("textbox", { name: "Workspace name" });
  await expect(name).toHaveValue("Workspace 2");
  await expect(name).toBeFocused();
  expect(
    await name.evaluate((el: HTMLInputElement) => [el.selectionStart, el.selectionEnd]),
  ).toEqual([0, "Workspace 2".length]);
  await expect(dropdown(page).locator(".ws-name-actions .btn")).toHaveText([
    "Open here",
    "New window",
  ]);
  await reloadsAfter(page, async () => {
    // Typing replaces the selected name.
    await page.keyboard.type("Orchard");
    await page.keyboard.press("Enter");
  });
  await expect(chip(page)).toHaveText("Orchard▾");
  await expect(page.locator("#lx-doc .welcome-add-folder")).toBeFocused();
  expect(await page.evaluate(() => window.__fake.workspaces().map((ws) => ws.name))).toEqual([
    "Studio",
    "Garden",
    "Orchard",
  ]);
});

test("renaming from the dropdown renames the chip and the title, cut short in a narrow window", async ({
  page,
}) => {
  await start(page, `garden&open=${encodeURIComponent(fixturePath(ALPHA))}`);
  await expect(page).toHaveTitle("Alpha — Studio");
  await chip(page).click();
  await dropdown(page).getByRole("button", { name: 'Rename "Studio"' }).click();
  const name = dropdown(page).getByRole("textbox", { name: "Workspace name" });
  await expect(name).toHaveValue("Studio");
  const long = "Field notes from the long walk along the northern coast";
  await name.fill(long);
  await name.press("Enter");
  await expect(dropdown(page)).toBeHidden();
  await expect(chip(page)).toHaveAttribute("title", long);
  await expect(page).toHaveTitle(`Alpha — ${long}`);

  await page.setViewportSize({ width: 640, height: 800 });
  const sizes = await page.evaluate(() => {
    const width = (sel: string) => document.querySelector(sel)?.getBoundingClientRect().width ?? 0;
    const label = document.querySelector(".ws-chip-name");
    return {
      chip: width("#lx-workspace-btn"),
      crumbs: width("#lx-breadcrumbs"),
      clipped: label ? label.scrollWidth > label.clientWidth : false,
      overflow: document.documentElement.scrollWidth > window.innerWidth,
    };
  });
  expect(sizes.clipped).toBe(true);
  expect(sizes.chip).toBeLessThanOrEqual(0.15 * 640 + 1);
  expect(sizes.crumbs).toBeGreaterThan(150);
  expect(sizes.overflow).toBe(false);
  await expect(page.locator("#lx-breadcrumbs .crumb.current")).toBeVisible();
});

test("a long list scrolls inside the dropdown, with Rename still in reach", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 600 });
  await start(page, "many=24");
  await chip(page).click();
  await expect(rows(page)).toHaveCount(25);
  await expect(rows(page).first()).toBeFocused();
  const box = await dropdown(page).boundingBox();
  expect(box).not.toBeNull();
  expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual(600);
  // The rows scroll; the actions below them stay in view.
  const scrolls = await dropdown(page)
    .locator(".ws-rows")
    .evaluate((el) => el.scrollHeight > el.clientHeight);
  expect(scrolls).toBe(true);
  const rename = dropdown(page).getByRole("button", { name: 'Rename "Studio"' });
  await expect(rename).toBeInViewport();
  await expect(rows(page).last()).not.toBeInViewport();
  await rows(page).last().scrollIntoViewIfNeeded();
  await expect(rows(page).last()).toBeInViewport();
  await expect(rename).toBeInViewport();
  await rename.click();
  await expect(dropdown(page).getByRole("textbox", { name: "Workspace name" })).toHaveValue(
    "Studio",
  );
});

test("a blank window lists the workspaces to choose from, and has no chip", async ({ page }) => {
  await start(page, "blank&garden");
  await expect(chip(page)).toBeHidden();
  await expect(page).toHaveTitle("Lectern");
  await expect(page.getByRole("heading", { name: "Choose a workspace" })).toBeVisible();
  const list = page.locator("#lx-doc .ws-chooser");
  await expect(list.locator(".ws-name")).toHaveText(["Studio", "Garden"]);
  await expect(list.locator(".ws-hint")).toHaveText(["vault +1", ""]);
  await expect(list.getByRole("button", { name: "Open in new window" })).toHaveCount(2);
  await expect(list.getByRole("button", { name: "New workspace" })).toBeVisible();
  await expect(page.locator("#lx-library .lib-empty-title")).toHaveText(
    "Choose a workspace, or add a folder to start a new one.",
  );
  // The rows read as choices at rest: framed.
  const frame = await list
    .locator(".ws-row")
    .first()
    .evaluate((el) => getComputedStyle(el).borderTopStyle);
  expect(frame).toBe("solid");
  await reloadsAfter(page, () => list.locator(".ws-open").first().click());
  await expect(chip(page)).toHaveText("Studio▾");
  await expect(
    page.locator(`#lx-library .lib-root[data-root="${VAULT.replaceAll("\\", "\\\\")}"]`),
  ).toHaveCount(1);
});

test("a folder dropped on a blank window asks for a name, then becomes a workspace", async ({
  page,
}) => {
  await start(page, "blank");
  // A file not named as Markdown opens as a loose file, without asking.
  await page.evaluate((path) => {
    window.__fake.setDoc(path, "<p>High water at noon</p>");
    window.__fake.drop([path]);
  }, `${VAULT}\\tide.txt`);
  await expect(page).toHaveTitle("tide.txt — Lectern");
  const ask = page.getByRole("dialog", { name: "New workspace" });
  await expect(ask).toHaveCount(0);
  await page.evaluate(() => {
    window.__fake.drop(["D:\\Elsewhere\\Notes"]);
  });
  await expect(ask).toContainText("D:\\Elsewhere\\Notes");
  const name = ask.getByRole("textbox", { name: "Workspace name" });
  await expect(name).toHaveValue("Workspace 2");
  await expect(name).toBeFocused();
  await reloadsAfter(page, async () => {
    await page.keyboard.type("Notes");
    await page.keyboard.press("Enter");
  });
  await expect(chip(page)).toHaveText("Notes▾");
  await expect(
    page.locator('#lx-library .lib-root[data-root="D:\\\\Elsewhere\\\\Notes"]'),
  ).toHaveCount(1);
});

test("Ctrl+N opens a new window and Ctrl+Q quits; the ⋯ menu has both", async ({ page }) => {
  await start(page, "");
  await page.keyboard.press("Control+n");
  // Still Add folder, which the fake answers with no folder.
  await page.keyboard.press("Control+Shift+N");
  await expect
    .poll(() => page.evaluate(() => window.__fake.windowCalls))
    .toEqual([{ call: "newWindow" }]);
  await page.locator("#lx-more-btn").click();
  const items = page.getByRole("menu").getByRole("menuitem");
  await expect(items.first()).toBeVisible();
  const labels = await items.locator(".menu-label").allTextContents();
  expect(labels.indexOf("New window")).toBe(labels.indexOf("Preferences") - 1);
  expect(labels.at(-1)).toBe("Quit Lectern");
  await expect(items.filter({ hasText: "New window" }).locator("kbd")).toHaveText("Ctrl+N");
  await expect(items.filter({ hasText: "Quit Lectern" }).locator("kbd")).toHaveText("Ctrl+Q");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Control+q");
  await expect
    .poll(() => page.evaluate(() => window.__fake.windowCalls))
    .toEqual([{ call: "newWindow" }, { call: "quit", force: false }]);
});

test("a half-written comment asks before the window turns to another workspace", async ({
  page,
}) => {
  await start(page, `garden&open=${encodeURIComponent(fixturePath(ALPHA))}`);
  await mark(page);
  await page.keyboard.press("Control+Alt+KeyM");
  const editor = page.locator("#lx-doc-pane .comment-editor");
  await expect(editor).toBeVisible();
  await page.keyboard.type("Worth a second look");

  await chip(page).click();
  await rows(page).nth(1).click();
  const confirm = page.getByRole("alertdialog", { name: "Your comment isn't saved" });
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "Keep writing" }).click();
  await expect(confirm).toHaveCount(0);
  await expect(editor.locator("textarea")).toHaveValue("Worth a second look");
  await expect(page.locator("body[data-marked]")).toHaveCount(1);
  expect(await page.evaluate(() => window.__fake.windowCalls)).toEqual([]);

  await chip(page).click();
  await rows(page).nth(1).click();
  await reloadsAfter(page, () => confirm.getByRole("button", { name: "Switch anyway" }).click());
  await expect(chip(page)).toHaveText("Garden▾");
});

test("typing a comment tells Rust it's unsaved, and Quit asks about other windows' drafts", async ({
  page,
}) => {
  await start(page, `open=${encodeURIComponent(fixturePath(ALPHA))}`);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(page.locator("#lx-doc-pane .comment-editor")).toBeVisible();
  await page.keyboard.type("Worth a second look");
  await expect.poll(() => page.evaluate(() => window.__fake.unsavedReports)).toEqual([true]);
  // Esc drops it on purpose: saved or not, nothing is held any more.
  await page.keyboard.press("Escape");
  await expect.poll(() => page.evaluate(() => window.__fake.unsavedReports)).toEqual([true, false]);

  await page.evaluate(() => {
    window.__fake.unsavedElsewhere = ["Garden"];
  });
  await page.keyboard.press("Control+q");
  const confirm = page.getByRole("alertdialog", { name: "Unsaved comment in Garden." });
  await expect(confirm).toContainText("Quit anyway?");
  await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
  await confirm.getByRole("button", { name: "Quit", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.__fake.windowCalls))
    .toEqual([
      { call: "quit", force: false },
      { call: "quit", force: true },
    ]);
});

test("Use its own theme: picking sepia changes only this window's theme", async ({ page }) => {
  await start(page, "garden");
  await page.keyboard.press("Control+,");
  const prefs = page.getByRole("dialog", { name: "Preferences" });
  await expect(prefs.locator(".prefs-group-title")).toHaveText(["This workspace", "All windows"]);
  await expect(prefs.getByRole("textbox", { name: "Workspace name" })).toHaveValue("Studio");
  await expect(prefs).toContainText("Same as other windows.");
  const own = prefs.getByRole("checkbox", { name: "Use its own theme" });
  await own.check();
  await prefs.getByRole("radio", { name: "Light" }).check();
  await prefs.getByRole("combobox", { name: "Light theme" }).selectOption("sepia");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "sepia");
  const themes = () =>
    page.evaluate(() => ({
      shared: window.__fake.sharedSettings().lightTheme,
      own: window.__fake.workspaces()[0]?.theme?.lightTheme ?? null,
    }));
  expect(await themes()).toEqual({ shared: "paper", own: "sepia" });
  // Off: the shared theme again.
  await own.uncheck();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "paper");
  expect(await themes()).toEqual({ shared: "paper", own: null });
});

test("Preferences deletes a closed workspace once it's confirmed", async ({ page }) => {
  await start(page, "garden");
  await page.keyboard.press("Control+,");
  const prefs = page.getByRole("dialog", { name: "Preferences" });
  const deleteStudio = prefs.getByRole("button", { name: "Delete Studio" });
  await expect(deleteStudio).toBeDisabled();
  await expect(deleteStudio).toHaveAttribute("title", "Close its window first.");
  await prefs.getByRole("button", { name: "Delete Garden" }).click();
  const confirm = page.getByRole("alertdialog", { name: 'Delete the workspace "Garden"?' });
  await expect(confirm).toContainText("Lectern forgets it; its folders stay on disk.");
  const cancel = confirm.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeFocused();
  await cancel.click();
  await expect(confirm).toHaveCount(0);
  await expect(prefs.locator(".prefs-workspaces li")).toHaveCount(2);
  await prefs.getByRole("button", { name: "Delete Garden" }).click();
  await confirm.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(prefs.locator(".prefs-workspaces li")).toHaveCount(1);
  expect(await page.evaluate(() => window.__fake.workspaces().map((ws) => ws.name))).toEqual([
    "Studio",
  ]);
  // One workspace left: the title is Lectern's again.
  await expect(page).toHaveTitle("Lectern");
});

test("a settings change from another window applies at once", async ({ page }) => {
  await start(page, `open=${encodeURIComponent(fixturePath(ALPHA))}`);
  await page.evaluate(() => {
    window.__fake.settingsElsewhere({ fontSize: 22, darkTheme: "nord", themeMode: "dark" });
  });
  await expect
    .poll(() =>
      page.evaluate(() =>
        getComputedStyle(document.documentElement).getPropertyValue("--font-size").trim(),
      ),
    )
    .toBe("22px");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "nord");
});

test("focus mode hides the chip with the header", async ({ page }) => {
  await start(page, `open=${encodeURIComponent(fixturePath(ALPHA))}`);
  await expect(chip(page)).toBeVisible();
  await page.keyboard.press("F11");
  await expect(chip(page)).toBeHidden();
  await page.keyboard.press("Escape");
  await expect(chip(page)).toBeVisible();
});
