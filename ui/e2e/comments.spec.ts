// Review comments, the read path, on the fake backend: its review store stands in for the sidecar
// and its text blocks for core's text map (dev/backend-fake.ts).
import { expect, test, type Page } from "@playwright/test";
import type { CommentView } from "../src/generated/CommentView";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import { fixturePath, openFixture } from "./util";

const ALPHA = "work/alpha/README.md";

/**
 * The fixtures whose visible text must match core's. Together they hold a code block, a footnote,
 * an alert, a wikilink with an alias, a task list, a table, a tag, and raw HTML both shown as text
 * and passed through; the test checks they still do.
 */
const TEXT_DOCS = [
  // A code block (with a note), a footnote, alerts, a table, and a raw HTML block.
  "friends/readme-style.md",
  // Wikilinks, one with an alias, and tags.
  "memory/index.md",
  // A task list, and a table with a raw <br>.
  "work/alpha/README.md",
  // Raw HTML shown as text, and allowed raw HTML (<details>, <kbd>).
  "prompts/writer.md",
  // Code blocks and nested lists.
  "stress/long-lines.md",
  // Code in list items, a quote, front matter.
  "work/alpha/plans/2026-01-01-big-plan.md",
];

function comment(over: Partial<CommentView> & { id: number }): CommentView {
  return {
    status: "open",
    state: "anchored",
    startLine: 1,
    endLine: 1,
    headingPath: [],
    jumpLine: 1,
    pinnedHeading: null,
    quote: "",
    currentText: null,
    entries: [],
    ...over,
  };
}

function you(text: string) {
  return { author: "you", kind: null, text, html: `<p>${text}</p>` } as const;
}

/** Comments on work/alpha/README.md: three open (one detached), one resolved. */
function alphaReview(): ReviewPayload {
  const path = fixturePath(ALPHA);
  return {
    notePath: path,
    sidecarPath: path.replace(/README\.md$/, "README.review.md"),
    noteWslPath: "/mnt/c/Fixtures/vault/work/alpha/README.md",
    sidecarWslPath: "/mnt/c/Fixtures/vault/work/alpha/README.review.md",
    exists: true,
    readOnly: null,
    comments: [
      comment({
        id: 1,
        startLine: 8,
        endLine: 8,
        headingPath: ["Alpha"],
        jumpLine: 8,
        quote: "a review from the platform team",
        entries: [you("Which team?")],
      }),
      comment({
        id: 2,
        startLine: 13,
        endLine: 13,
        headingPath: ["Alpha", "Tasks"],
        jumpLine: 13,
        quote: "Write the first notes",
        entries: [you("Done already?")],
      }),
      comment({
        id: 3,
        status: "resolved",
        startLine: 21,
        endLine: 21,
        headingPath: ["Alpha", "Files"],
        jumpLine: 21,
        quote: "with every step",
        entries: [
          you("Split this cell."),
          { author: "claude", kind: "resolved", text: "Split.", html: "<p>Split.</p>" },
        ],
      }),
      comment({
        id: 4,
        state: "detached",
        startLine: 30,
        endLine: 30,
        headingPath: ["Alpha", "Files"],
        jumpLine: 17,
        pinnedHeading: "Files",
        quote: "the archive moved last week",
        entries: [you("Is this still true?")],
      }),
    ],
    unreadable: [],
    openCount: 3,
  };
}

/** Gives the note its sidecar, and says it changed, as the watcher would. */
async function seed(page: Page, payload: ReviewPayload): Promise<void> {
  await page.evaluate((p) => {
    window.__fake.setReview(p.notePath, p);
    window.__fake.emit("review-changed", { path: p.notePath });
  }, payload);
}

function highlighted(page: Page): Promise<boolean> {
  return page.evaluate(() => CSS.highlights.has("comment"));
}

function card(page: Page, id: number) {
  return page.locator(`#lx-comments-pane article.comment-card[data-id="${String(id)}"]`);
}

/** Where to click on the middle of `word` in the note's paragraph that holds it. */
function wordPoint(page: Page, word: string): Promise<{ x: number; y: number }> {
  return page.evaluate((word) => {
    const p = [...document.querySelectorAll("#lx-doc p")].find((el) =>
      el.textContent.includes(word),
    );
    const text = [...(p?.childNodes ?? [])].find(
      (n): n is Text => n instanceof Text && n.data.includes(word),
    );
    if (!text) throw new Error(`no paragraph with ${word}`);
    const range = document.createRange();
    const at = text.data.indexOf(word);
    range.setStart(text, at);
    range.setEnd(text, at + word.length);
    const box = range.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  }, word);
}

test("header toggle hides every comment surface and persists across reload", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  const button = page.locator("#lx-comments-btn");
  const badge = button.locator(".count-badge");
  const dots = page.locator("#lx-doc .lx-cdot");
  await expect(badge).toHaveText("3");
  await expect(button).toHaveAttribute("aria-pressed", "true");
  // Next to Aa.
  await expect(page.locator("#lx-reading-btn + #lx-comments-btn")).toHaveCount(1);
  await expect(dots).toHaveCount(2);
  await expect.poll(() => highlighted(page)).toBe(true);
  await expect(page.getByRole("tab", { name: "Comments (3)" })).toBeVisible();

  await page.keyboard.press("Control+Shift+M");
  await expect(dots).toHaveCount(0);
  await expect.poll(() => highlighted(page)).toBe(false);
  await expect(page.locator("#lx-app")).toHaveClass(/no-comments/);
  await expect(page.locator("#lx-outline [role=tablist]")).toBeHidden();
  await expect(page.locator("#lx-outline-pane")).toBeVisible();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  // The count still shows, dimmed.
  await expect(badge).toHaveText("3");
  await expect(badge).toHaveClass(/muted/);

  await page.reload();
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
  await seed(page, alphaReview());
  await expect(badge).toHaveText("3");
  await expect(badge).toHaveClass(/muted/);
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("#lx-app")).toHaveClass(/no-comments/);
  await expect(dots).toHaveCount(0);
  expect(await highlighted(page)).toBe(false);

  // And the button brings them back.
  await button.click();
  await expect(dots).toHaveCount(2);
  await expect.poll(() => highlighted(page)).toBe(true);
  await expect(page.locator("#lx-outline [role=tablist]")).toBeVisible();
});

test("clicking a dot opens the Comments tab and focuses the card", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await expect(page.locator("#lx-doc .lx-cdot")).toHaveCount(2);
  await expect(page.getByRole("tab", { name: "Outline" })).toHaveAttribute("aria-selected", "true");
  // The panel closed: the dot opens it.
  await page.keyboard.press("Control+Shift+O");
  await expect(page.locator("#lx-outline")).toBeHidden();
  await page.locator('#lx-doc .lx-cdot[data-comments="2"]').click();
  await expect(page.locator("#lx-outline")).toBeVisible();
  await expect(page.getByRole("tab", { name: /^Comments/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(card(page, 2)).toHaveClass(/selected/);
  await expect(card(page, 2)).toBeInViewport();
  await expect.poll(() => page.evaluate(() => CSS.highlights.has("comment-focus"))).toBe(true);

  // A click on highlighted text does the same for its comment.
  const point = await wordPoint(page, "platform");
  await page.mouse.click(point.x, point.y);
  await expect(card(page, 1)).toHaveClass(/selected/);
  await expect(card(page, 2)).not.toHaveClass(/selected/);
});

test("focus mode hides the dots, keeps the highlights and ignores clicks on them", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await page.locator('#lx-doc .lx-cdot[data-comments="2"]').click();
  await expect(card(page, 2)).toHaveClass(/selected/);
  await page.keyboard.press("F11");
  await expect(page.locator("body")).toHaveClass(/focus/);
  await expect(page.locator("#lx-doc .lx-cdot")).toHaveCount(2);
  await expect(page.locator("#lx-doc .lx-cdot:visible")).toHaveCount(0);
  expect(await highlighted(page)).toBe(true);
  const point = await wordPoint(page, "platform");
  await page.mouse.click(point.x, point.y);
  await page.waitForTimeout(200);
  await expect(card(page, 2)).toHaveClass(/selected/);
  await expect(card(page, 1)).not.toHaveClass(/selected/);
  await expect(page.locator("#lx-outline")).toBeHidden();

  // Out of focus mode the same click selects its comment.
  await page.keyboard.press("Escape");
  await expect(page.locator("body")).not.toHaveClass(/focus/);
  await expect(page.locator("#lx-doc .lx-cdot:visible")).toHaveCount(2);
  const again = await wordPoint(page, "platform");
  await page.mouse.click(again.x, again.y);
  await expect(card(page, 1)).toHaveClass(/selected/);
});

test("a live reload while typing a reply keeps the caret", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await page.getByRole("tab", { name: /^Comments/ }).click();
  await card(page, 1).getByRole("button", { name: "Reply" }).click();
  const box = card(page, 1).locator("textarea");
  await box.fill("Name the reviewer");
  await box.evaluate((el: HTMLTextAreaElement) => {
    el.setSelectionRange(5, 8, "backward");
  });
  await page.evaluate((path) => {
    window.__fake.claudeReply(path, 1, "reply", "Noted.");
    window.__fake.emit("review-changed", { path });
  }, fixturePath(ALPHA));
  await expect(card(page, 1).locator(".comment-entry.claude")).toHaveCount(1);
  expect(
    await box.evaluate((el: HTMLTextAreaElement) => [
      el === document.activeElement,
      el.selectionStart,
      el.selectionEnd,
      el.selectionDirection,
    ]),
  ).toEqual([true, 5, 8, "backward"]);
  await page.keyboard.type("a");
  await expect(box).toHaveValue("Name a reviewer");
});

test("a long code line in a comment scrolls in its block, not the pane", async ({ page }) => {
  await openFixture(page, ALPHA);
  const review = alphaReview();
  const line = `const batch = ${"x".repeat(85)};`;
  expect(line.length).toBe(100);
  review.comments[0]?.entries.push({
    author: "claude",
    kind: null,
    text: `\`\`\`js\n${line}\n\`\`\``,
    html: `<div class="code-block" data-lang="js" data-sourcepos="1:1-3:3"><div class="code-head"><span class="code-lang">js</span><button type="button" class="code-copy" aria-label="Copy code">Copy</button></div><pre><code class="language-js">${line}\n</code></pre></div>`,
  });
  await seed(page, review);
  await page.getByRole("tab", { name: /^Comments/ }).click();
  const pre = card(page, 1).locator(".comment-body pre");
  await expect(pre).toBeVisible();
  // The line is there, scrolling inside its block...
  expect(await pre.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  // ...and the pane doesn't scroll sideways.
  const pane = page.locator("#lx-comments-pane");
  expect(await pane.evaluate((el) => el.scrollWidth - el.clientWidth)).toBe(0);
});

test("Claude's reply appears live", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await expect(card(page, 1)).toHaveCount(1);
  await expect(card(page, 1).locator(".comment-entry.claude")).toHaveCount(0);
  await page.evaluate((path) => {
    window.__fake.claudeReply(path, 1, "question", "The platform team, or the data team?");
    window.__fake.emit("review-changed", { path });
  }, fixturePath(ALPHA));
  await expect(card(page, 1).locator(".comment-entry.claude .comment-body")).toHaveText(
    "The platform team, or the data team?",
  );
  await expect(card(page, 1).locator(".comment-status")).toHaveText("question");
  await expect(page.locator("#lx-tab-comments")).toHaveClass(/flash/);
});

test("an edit that deletes the passage shows the comment as detached with its original text", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await page.getByRole("tab", { name: /^Comments/ }).click();
  await expect(card(page, 1)).toHaveAttribute("data-state", "anchored");
  await expect(page.locator('#lx-doc .lx-cdot[data-comments="1"]')).toHaveCount(1);
  await page.evaluate((path) => {
    const blocks = window.__fake.blocksOf(path);
    window.__fake.setText(
      path,
      blocks.filter(([, , text]) => !text.includes("platform team")),
    );
    window.__fake.emit("review-changed", { path });
  }, fixturePath(ALPHA));
  const detached = page.locator("#lx-comments-pane .comment-group.detached");
  await expect(detached.locator("h3")).toHaveText("Detached");
  await expect(detached.locator('article.comment-card[data-id="1"]')).toHaveAttribute(
    "data-state",
    "detached",
  );
  await expect(card(page, 1).locator(".comment-quote")).toHaveText(
    "a review from the platform team",
  );
  await expect(card(page, 1)).toContainText("Detached");
  await expect(page.locator('#lx-doc .lx-cdot[data-comments="1"]')).toHaveCount(0);
  await expect(page.locator('#lx-doc .lx-cdot[data-comments="2"]')).toHaveCount(1);
});

test("copy produces the documented format", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await page.getByRole("tab", { name: /^Comments/ }).click();
  await page.getByRole("button", { name: "Copy comments" }).click();
  await expect(page.locator("#lx-toasts .toast").last()).toHaveText("Copied 3 comments");
  const copied = () => page.evaluate(() => navigator.clipboard.readText());
  expect(await copied()).toBe(
    [
      `Review comments on ${fixturePath(ALPHA)}`,
      "  WSL: /mnt/c/Fixtures/vault/work/alpha/README.md",
      `(sidecar: ${fixturePath("work/alpha/README.review.md")} — reply there using its format, or edit the note directly)`,
      "  WSL: /mnt/c/Fixtures/vault/work/alpha/README.review.md",
      "",
      'C4 [detached — original text: "the archive moved last week"] (was L30 · Alpha › Files)',
      "  You: Is this still true?",
      "",
      "C1 [open] L8 · Alpha",
      "  > a review from the platform team",
      "  You: Which team?",
      "",
      "C2 [open] L13 · Alpha › Tasks",
      "  > Write the first notes",
      "  You: Done already?",
      "",
    ].join("\n"),
  );

  await page.getByRole("button", { name: "More ways to copy" }).click();
  await page.getByRole("menuitem", { name: "Copy all, including resolved" }).click();
  await expect(page.locator("#lx-toasts .toast").last()).toHaveText("Copied 4 comments");
  expect(await copied()).toContain(
    [
      "C3 [resolved] L21 · Alpha › Files",
      "  > with every step",
      "  You: Split this cell.",
      "  Claude (resolved): Split.",
    ].join("\n"),
  );

  // One card's own copy.
  await card(page, 2).getByRole("button", { name: "Copy", exact: true }).click();
  await expect(page.locator("#lx-toasts .toast").last()).toHaveText("Copied 1 comment");
  expect(await copied()).toContain("\n\nC2 [open] L13 · Alpha › Tasks\n");
  expect(await copied()).not.toContain("C1 [open]");
});

test("UI text matches core text on real fixtures", async ({ page }) => {
  const seen = new Set<string>();
  for (const rel of TEXT_DOCS) {
    await openFixture(page, rel);
    const { ui, core, has } = await page.evaluate((path) => {
      const doc = document.querySelector("#lx-doc");
      const found = (selector: string) => doc?.querySelector(selector) !== null;
      return {
        ui: window.__lxDocText(),
        core: window.__fake.coreText(path),
        has: {
          "code block": found(".code-block"),
          footnote: found(".footnote-ref"),
          alert: found(".markdown-alert"),
          "wikilink with an alias": [...(doc?.querySelectorAll("a.wikilink") ?? [])].some(
            (a) => a.textContent === "Vault home",
          ),
          "task list": found("li > input[type=checkbox]"),
          table: found("table"),
          tag: found(".tag"),
          "raw HTML shown as text": doc?.textContent.includes("<script>") === true,
          "raw HTML passed through": found("details, kbd"),
        },
      };
    }, fixturePath(rel));
    for (const [construct, present] of Object.entries(has)) {
      if (present) seen.add(construct);
    }
    if (ui !== core) {
      let at = 0;
      while (at < ui.length && ui[at] === core[at]) at++;
      console.log(
        `${rel}: the texts differ at ${String(at)}\n  UI:   ${JSON.stringify(ui.slice(at, at + 80))}\n  core: ${JSON.stringify(core.slice(at, at + 80))}`,
      );
    }
    expect(core.length, rel).toBeGreaterThan(0);
    expect(ui, rel).toBe(core);
  }
  expect([...seen].sort()).toEqual(
    [
      "alert",
      "code block",
      "footnote",
      "raw HTML passed through",
      "raw HTML shown as text",
      "table",
      "tag",
      "task list",
      "wikilink with an alias",
    ].sort(),
  );
});
