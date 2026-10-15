// Copying paths in Windows or WSL form, on the fake backend, whose WSL distribution is Ubuntu.
import { expect, test, type Page } from "@playwright/test";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import { fixturePath, openFixture } from "./util";

const INDEX = "memory/index.md";
const LINK_MENU = [
  "Open",
  "Copy",
  "Copy as Windows path",
  "Copy as WSL path",
  "Reveal in Explorer",
];

function clipboard(page: Page): Promise<string> {
  return page.evaluate(() => navigator.clipboard.readText());
}

function lastToast(page: Page) {
  return page.locator("#lx-toasts .toast").last();
}

test("a link's menu, a selection and the shortcuts copy paths in either form", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, INDEX);
  const index = fixturePath(INDEX);
  const readme = fixturePath("README.md");
  await page.evaluate(
    ({ index, readme }) => {
      window.__fake.setDoc(
        index,
        `<h1 id="memory-index">Memory index</h1>
<p><a href="#" class="wikilink" data-kind="doc" data-target="${readme}">Vault home</a></p>
<pre><code id="cmd">/home/me/projects/app/run.sh</code></pre>`,
      );
      window.__fake.emit("doc-changed", { path: index });
    },
    { index, readme },
  );

  await page.locator("#lx-doc a.wikilink", { hasText: "Vault home" }).click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveText(LINK_MENU);
  await menu.getByRole("menuitem", { name: "Copy as WSL path" }).click();
  await expect(menu).toHaveCount(0);
  await expect(lastToast(page)).toContainText("Copied the WSL path");
  expect(await clipboard(page)).toBe("/mnt/c/Fixtures/vault/README.md");

  // Nothing selected or focused: the open note.
  await page.keyboard.press("Control+Shift+C");
  await expect(lastToast(page).locator(".toast-detail")).toHaveText(
    "/mnt/c/Fixtures/vault/memory/index.md",
  );
  expect(await clipboard(page)).toBe("/mnt/c/Fixtures/vault/memory/index.md");

  // A Linux path selected in a code block, in Windows form, through the distribution.
  await page.locator("#cmd").selectText();
  await page.keyboard.press("Control+Shift+W");
  await expect(lastToast(page)).toContainText("Copied the Windows path");
  await expect
    .poll(() => clipboard(page))
    .toBe("\\\\wsl.localhost\\Ubuntu\\home\\me\\projects\\app\\run.sh");
});

test("a link in a review comment has the same menu", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, INDEX);
  const index = fixturePath(INDEX);
  const review: ReviewPayload = {
    notePath: index,
    sidecarPath: fixturePath("memory/index.review.md"),
    noteWslPath: "/mnt/c/Fixtures/vault/memory/index.md",
    sidecarWslPath: "/mnt/c/Fixtures/vault/memory/index.review.md",
    exists: true,
    readOnly: null,
    comments: [
      {
        id: 1,
        status: "open",
        state: "detached",
        startLine: 1,
        endLine: 1,
        headingPath: [],
        jumpLine: 1,
        pinnedHeading: null,
        pinnedAt: null,
        quote: "Memory index",
        textStart: null,
        textEnd: null,
        currentText: null,
        entries: [
          {
            author: "agent",
            name: "Codex",
            kind: "reply",
            text: "See `/home/me/notes/plan.md`.",
            html: '<p>See <a href="#" class="code-link" data-kind="path" data-target="\\\\wsl.localhost\\Ubuntu\\home\\me\\notes\\plan.md"><code>/home/me/notes/plan.md</code></a>.</p>',
          },
        ],
      },
    ],
    unreadable: [],
    openCount: 1,
  };
  await page.evaluate((p) => {
    window.__fake.setReview(p.notePath, p);
    window.__fake.emit("review-changed", { path: p.notePath });
  }, review);
  await page.getByRole("tab", { name: /^Comments/ }).click();

  await page.locator(".comment-body a.code-link").click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveText(LINK_MENU);
  await menu.getByRole("menuitem", { name: "Copy as WSL path" }).click();
  await expect(lastToast(page)).toContainText("Copied the WSL path");
  expect(await clipboard(page)).toBe("/home/me/notes/plan.md");
});

test("a selected relative path or command copies as written, its slashes in either form", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, INDEX);
  const index = fixturePath(INDEX);
  await page.evaluate((index) => {
    window.__fake.setDoc(
      index,
      `<h1 id="memory-index">Memory index</h1>
<p>Run <code id="rel">app/services/x/writing_type.rb</code> after a change.</p>
<pre><code id="cmd">bundle exec rubocop app/services/x spec/services/x</code></pre>`,
    );
    window.__fake.emit("doc-changed", { path: index });
  }, index);

  const rel = page.locator("#rel");
  await rel.selectText();
  await rel.click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveText([
    "Copy",
    "Copy as Windows path",
    "Copy as WSL path",
  ]);
  await menu.getByRole("menuitem", { name: "Copy as Windows path" }).click();
  await expect(lastToast(page)).toContainText("Copied the Windows path");
  expect(await clipboard(page)).toBe("app\\services\\x\\writing_type.rb");

  // The shortcuts on the same selection: WSL with Ctrl+Shift+C, Windows with Ctrl+Shift+W.
  await rel.selectText();
  await page.keyboard.press("Control+Shift+C");
  await expect.poll(() => clipboard(page)).toBe("app/services/x/writing_type.rb");
  await page.keyboard.press("Control+Shift+W");
  await expect.poll(() => clipboard(page)).toBe("app\\services\\x\\writing_type.rb");

  // A command: each path converted, the rest as it is.
  const cmd = page.locator("#cmd");
  await cmd.selectText();
  await cmd.click({ button: "right" });
  await expect(menu.getByRole("menuitem")).toHaveText([
    "Copy",
    "Copy with Windows paths",
    "Copy with WSL paths",
  ]);
  await menu.getByRole("menuitem", { name: "Copy with Windows paths" }).click();
  await expect(lastToast(page)).toContainText("Copied with 2 Windows paths");
  expect(await clipboard(page)).toBe("bundle exec rubocop app\\services\\x spec\\services\\x");
});

test("a relative Markdown link copies its path as written, not where it leads", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, "notes/résumé notes.md");
  await page.locator("#lx-doc a", { hasText: "report" }).click({ button: "right" });
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem")).toHaveText(LINK_MENU);
  await menu.getByRole("menuitem", { name: "Copy as Windows path" }).click();
  await expect(lastToast(page)).toContainText("Copied the Windows path");
  expect(await clipboard(page)).toBe("..\\artifacts\\report.html");
});
