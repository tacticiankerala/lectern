// Review comments, the read path, on the fake backend: its review store stands in for the sidecar
// and its text blocks for core's text map (dev/backend-fake.ts).
import { expect, test, type Page } from "@playwright/test";
import type { CommentView } from "../src/generated/CommentView";
import type { LibraryPayload } from "../src/generated/LibraryPayload";
import type { ReviewPayload } from "../src/generated/ReviewPayload";
import { fixturePath, openFixture } from "./util";

const ALPHA = "work/alpha/README.md";
const PLAN = "work/alpha/plans/2026-01-01-big-plan.md";

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

/**
 * Selects `phrase` in the note by dragging the mouse from the middle of its first character's left
 * half to the middle of its last character's right half, inside the first block holding it.
 */
async function dragSelect(page: Page, phrase: string): Promise<void> {
  const ends = await page.evaluate(async (phrase) => {
    const block = [...document.querySelectorAll<HTMLElement>("#lx-doc [data-sourcepos]")]
      .reverse()
      .find((el) => el.textContent.includes(phrase));
    const walker = document.createTreeWalker(block ?? document.body, NodeFilter.SHOW_TEXT);
    let text: Text | null = null;
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (n instanceof Text && n.data.includes(phrase)) {
        text = n;
        break;
      }
    }
    if (!text) throw new Error(`no text node with ${phrase}`);
    // Blocks near the view take their real height a frame later: scroll until it holds still.
    const holder = text.parentElement;
    for (let tries = 0, last = NaN; tries < 10; tries++) {
      holder?.scrollIntoView({ block: "center" });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const top = holder?.getBoundingClientRect().top ?? 0;
      if (Math.abs(top - last) < 1) break;
      last = top;
    }
    const at = text.data.indexOf(phrase);
    const charBox = (i: number) => {
      const range = document.createRange();
      range.setStart(text, i);
      range.setEnd(text, i + 1);
      return range.getBoundingClientRect();
    };
    const first = charBox(at);
    const last = charBox(at + phrase.length - 1);
    return {
      from: { x: first.left + first.width / 4, y: first.top + first.height / 2 },
      to: { x: last.right - last.width / 4, y: last.top + last.height / 2 },
    };
  }, phrase);
  await page.mouse.move(ends.from.x, ends.from.y);
  await page.mouse.down();
  await page.mouse.move(ends.to.x, ends.to.y, { steps: 8 });
  await page.mouse.up();
  expect(await page.evaluate(() => document.getSelection()?.toString())).toBe(phrase);
}

/** The block in the note whose text is just `text`, scrolled to the very top of the pane. */
async function scrollToTop(page: Page, selector: string, text: string): Promise<void> {
  await expect
    .poll(() =>
      page.evaluate(
        ({ selector, text }) => {
          const el = [...document.querySelectorAll(`#lx-doc ${selector}`)].find(
            (e) => e.textContent.trim() === text,
          );
          const pane = document.getElementById("lx-doc-pane");
          if (!el || !pane) throw new Error(`no ${selector} saying ${text}`);
          const off = el.getBoundingClientRect().top - pane.getBoundingClientRect().top;
          pane.scrollTop += off;
          return Math.abs(off) < 1;
        },
        { selector, text },
      ),
    )
    .toBe(true);
}

const editor = (page: Page) => page.locator("#lx-doc-pane .comment-editor");

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

test("add a comment from a selection, reply, resolve", async ({ page }) => {
  await openFixture(page, ALPHA);
  const button = page.locator("#lx-doc-pane button.lx-sel-comment");
  await expect(button).toBeHidden();
  await dragSelect(page, "review from the platform team");
  await expect(button).toBeVisible();
  await expect(button).toHaveText("Comment");
  // On the selection's last line, just after its end, outside the note's own markup.
  await expect(page.locator("#lx-doc .lx-sel-comment")).toHaveCount(0);
  const end = await page.evaluate(() => {
    const rects = [...(document.getSelection()?.getRangeAt(0).getClientRects() ?? [])];
    const last = rects[rects.length - 1];
    return last ? { right: last.right, middle: (last.top + last.bottom) / 2 } : null;
  });
  const buttonBox = await button.boundingBox();
  expect(end && buttonBox && Math.abs(buttonBox.y + buttonBox.height / 2 - end.middle) < 3).toBe(
    true,
  );
  expect(end && buttonBox && buttonBox.x - end.right).toBeGreaterThanOrEqual(4);
  expect(end && buttonBox && buttonBox.x - end.right).toBeLessThanOrEqual(8);
  await button.click();
  await expect(editor(page)).toBeVisible();
  await expect(button).toBeHidden();
  const text = editor(page).locator("textarea");
  await expect(text).toBeFocused();
  await expect(text).toHaveAttribute("placeholder", "Comment…");
  await page.keyboard.type("Which team, and since when?");
  await page.keyboard.press("Control+Enter");

  await expect(editor(page)).toBeHidden();
  // Anchored by the fake against core's text: the quote the page took is core's too.
  await expect(card(page, 1)).toHaveAttribute("data-state", "anchored");
  await expect(card(page, 1)).toHaveClass(/selected/);
  await expect(card(page, 1).locator(".comment-quote")).toHaveText("review from the platform team");
  await expect(card(page, 1).locator(".comment-place")).toHaveText("L8");
  await expect(card(page, 1).locator(".comment-entry.you .comment-body")).toHaveText(
    "Which team, and since when?",
  );
  await expect(page.getByRole("tab", { name: "Comments (1)" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.locator('#lx-doc .lx-cdot[data-comments="1"]')).toHaveCount(1);
  await expect(page.locator("#lx-comments-btn .count-badge")).toHaveText("1");

  await card(page, 1).getByRole("button", { name: "Reply" }).click();
  await card(page, 1).locator("textarea").fill("The data team may know.");
  await page.keyboard.press("Control+Enter");
  await expect(card(page, 1).locator(".comment-entry.you")).toHaveCount(2);

  await card(page, 1).getByRole("button", { name: "Resolve" }).click();
  await expect(card(page, 1)).toHaveCount(0);
  await expect(page.locator("#lx-comments-pane .comments-empty")).toHaveText("No open comments.");
  await page.locator('#lx-comments-pane [data-filter="all"]').click();
  await expect(card(page, 1)).toHaveAttribute("data-status", "resolved");
});

test("add a comment on a code block via +, and on a list item via Ctrl+Alt+M", async ({ page }) => {
  await openFixture(page, PLAN);
  const plus = page.locator("#lx-doc-pane button.lx-block-plus");
  const code = page.locator('#lx-doc .code-block[data-sourcepos="30:3-42:5"]');
  await code.hover();
  await expect(plus).toBeVisible();
  await expect(plus).toHaveAttribute("aria-label", "Comment on this block");
  await expect(page.locator("#lx-doc .lx-block-plus")).toHaveCount(0);
  // Just left of the block's own edge, though it's indented in a list item, level with its top.
  const plusBox = await plus.boundingBox();
  const codeBox = await code.boundingBox();
  const gap = plusBox && codeBox ? codeBox.x - (plusBox.x + plusBox.width) : -1;
  expect(gap).toBeGreaterThanOrEqual(4);
  expect(gap).toBeLessThanOrEqual(12);
  expect(plusBox && codeBox && Math.abs(plusBox.y - codeBox.y) < 24).toBe(true);
  // By a list item, it clears the bullet.
  const item = page.locator("#lx-doc li", { hasText: "Create: app/models/alert_1_1.rb" });
  await item.hover();
  await expect(plus).toBeVisible();
  const itemBox = await item.boundingBox();
  const byItem = await plus.boundingBox();
  const clear = itemBox && byItem ? itemBox.x - (byItem.x + byItem.width) : -1;
  expect(clear).toBeGreaterThanOrEqual(24);
  expect(clear).toBeLessThanOrEqual(48);
  await code.hover();
  await plus.click();
  await expect(editor(page)).toBeVisible();
  await page.keyboard.type("Add a case for an empty name.");
  await page.keyboard.press("Control+Enter");
  await expect(card(page, 1)).toHaveAttribute("data-state", "anchored");
  await expect(card(page, 1).locator(".comment-place")).toHaveText("L30–L42");
  await expect(card(page, 1).locator(".comment-quote")).toHaveText(
    /^RSpec\.describe Alert11 do let\(:record\) \{ described_class\.new/,
  );

  // Nothing selected: Ctrl+Alt+M comments on the block at the top of the view.
  await page.evaluate(() => document.getSelection()?.removeAllRanges());
  await scrollToTop(page, "li", "Create: app/models/alert_1_1.rb");
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(editor(page)).toBeVisible();
  await page.keyboard.type("Name it after the alert.");
  await page.keyboard.press("Control+Enter");
  await expect(card(page, 2)).toHaveAttribute("data-state", "anchored");
  await expect(card(page, 2).locator(".comment-place")).toHaveText("L24");
  await expect(card(page, 2).locator(".comment-quote")).toHaveText(
    "Create: app/models/alert_1_1.rb",
  );
  await expect(card(page, 2)).toHaveClass(/selected/);

  // Hidden comments come back for it, and focus mode is left first.
  await page.keyboard.press("Control+Shift+M");
  await expect(page.locator("#lx-app")).toHaveClass(/no-comments/);
  await page.keyboard.press("F11");
  await expect(page.locator("body")).toHaveClass(/focus/);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(page.locator("body")).not.toHaveClass(/focus/);
  await expect(page.locator("#lx-app")).not.toHaveClass(/no-comments/);
  await expect(editor(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(editor(page)).toBeHidden();
});

test("re-attach a detached comment", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await page.getByRole("tab", { name: /^Comments/ }).click();
  await expect(card(page, 4)).toHaveAttribute("data-state", "detached");
  await card(page, 4).getByRole("button", { name: "Re-attach" }).click();
  const banner = page.locator("#lx-comments-pane .comments-attach");
  await expect(banner).toContainText("Select the new text for C4, then press Attach here.");
  await dragSelect(page, "Ship the first slice");
  const button = page.locator("#lx-doc-pane button.lx-sel-comment");
  await expect(button).toHaveText("Attach C4 here");
  await button.click();
  await expect(card(page, 4)).toHaveAttribute("data-state", "anchored");
  await expect(page.locator("#lx-comments-pane .comment-group.detached")).toHaveCount(0);
  await expect(card(page, 4).locator(".comment-quote")).toHaveText("Ship the first slice");
  await expect(card(page, 4)).toHaveClass(/selected/);
  await expect(banner).toBeHidden();
  await expect(page.locator('#lx-doc .lx-cdot[data-comments="4"]')).toHaveCount(1);
  // No editor: the selection is all a re-attach needs.
  await expect(editor(page)).toBeHidden();
});

test("turning the feature off in Preferences removes every surface and the header button", async ({
  page,
}) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  // A note in the library with open comments, as the scan counts them.
  await page.evaluate(async () => {
    const fake = window.__fake as unknown as { getLibrary(): Promise<LibraryPayload> };
    const library = await fake.getLibrary();
    const note = library.roots[0]?.tree?.children.find((c) => !c.isDir);
    if (!note) throw new Error("no note at the top of the library");
    note.comments = 2;
    window.__fake.emit("library-updated", library);
  });
  const button = page.locator("#lx-comments-btn");
  const dots = page.locator("#lx-doc .lx-cdot");
  const counts = page.locator("#lx-library .tree-count");
  const tabs = page.locator("#lx-outline [role=tablist]");
  await expect(button).toBeVisible();
  await expect(dots).toHaveCount(2);
  await expect(counts).toHaveCount(1);
  await expect(tabs).toBeVisible();

  await page.keyboard.press("Control+,");
  const toggle = page.locator('input[name="lx-review-comments"]');
  await expect(toggle).toBeChecked();
  await expect(page.locator(".prefs")).toContainText(
    "Comments are saved next to each note as <note>.review.md, a Markdown file Claude can read and reply in.",
  );
  await toggle.uncheck();
  await page.keyboard.press("Escape");
  await expect(button).toBeHidden();
  await expect(dots).toHaveCount(0);
  await expect(counts).toHaveCount(0);
  await expect(tabs).toBeHidden();
  await expect(page.locator("#lx-comments-pane")).toBeEmpty();
  expect(await highlighted(page)).toBe(false);
  // Nothing offers to add one.
  await dragSelect(page, "review from the platform team");
  await page.waitForTimeout(100);
  await expect(page.locator(".lx-sel-comment:visible, .lx-block-plus:visible")).toHaveCount(0);
  await page.evaluate(() => document.getSelection()?.removeAllRanges());
  await page.locator("#lx-doc li").first().hover();
  await page.waitForTimeout(100);
  await expect(page.locator(".lx-sel-comment:visible, .lx-block-plus:visible")).toHaveCount(0);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(page.locator(".comment-editor:visible")).toHaveCount(0);

  await page.keyboard.press("Control+,");
  await toggle.check();
  await page.keyboard.press("Escape");
  await expect(button).toBeVisible();
  await expect(dots).toHaveCount(2);
  await expect(counts).toHaveCount(1);
  await expect(tabs).toBeVisible();
  await expect.poll(() => highlighted(page)).toBe(true);
});

test("Ctrl+Alt+M scrolled partway into a block comments on that block", async ({ page }) => {
  await openFixture(page, "stress/long-lines.md");
  // 100 px into the first code block, which runs on well past the top of the view.
  await expect
    .poll(() =>
      page.evaluate(() => {
        const code = document.querySelector("#lx-doc .code-block");
        const pane = document.getElementById("lx-doc-pane");
        if (!code || !pane) throw new Error("no code block");
        const off = code.getBoundingClientRect().top - pane.getBoundingClientRect().top + 100;
        pane.scrollTop += off;
        return Math.abs(off) < 1;
      }),
    )
    .toBe(true);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(editor(page)).toBeVisible();
  await page.keyboard.type("Fold the repeated lines.");
  await page.keyboard.press("Control+Enter");
  await expect(card(page, 1)).toHaveAttribute("data-state", "anchored");
  await expect(card(page, 1).locator(".comment-place")).toHaveText("L11–L50");
  await expect(card(page, 1).locator(".comment-quote")).toHaveText(
    /^\/\/ block 1 func step2\(ctx \*Context\) error \{/,
  );
});

test("Ctrl+Alt+M on a selection scrolled out of view brings it back and opens by it", async ({
  page,
}) => {
  await openFixture(page, PLAN);
  await dragSelect(page, "a synthetic plan");
  await page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    if (pane) pane.scrollTop += 3000;
  });
  // Where the selected words are, as the editor takes the selection into its box once open.
  const selectionInView = () =>
    page.evaluate(() => {
      const p = [...document.querySelectorAll("#lx-doc p")].find((el) =>
        el.textContent.includes("a synthetic plan"),
      );
      const pane = document.getElementById("lx-doc-pane")?.getBoundingClientRect();
      const r = p?.getBoundingClientRect();
      return !!r && !!pane && r.top >= pane.top && r.bottom <= pane.bottom;
    });
  expect(await selectionInView()).toBe(false);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(editor(page)).toBeVisible();
  await expect(editor(page)).toBeInViewport({ ratio: 1 });
  expect(await selectionInView()).toBe(true);
  // What's typed goes into it.
  await page.keyboard.type("Say what it exercises.");
  await expect(editor(page).locator("textarea")).toHaveValue("Say what it exercises.");
});

test("a kept draft comes back into view when a comment is asked for again", async ({ page }) => {
  await openFixture(page, PLAN);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(editor(page)).toBeVisible();
  await page.keyboard.type("Half a thought");
  await page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    if (pane) pane.scrollTop += 3000;
  });
  await expect(editor(page)).not.toBeInViewport();
  // A click in the note (its margin, clear of links) leaves the draft open, out of view.
  const doc = await page.locator("#lx-doc").boundingBox();
  if (!doc) throw new Error("no note");
  await page.mouse.click(doc.x + 10, 450);
  await expect(editor(page)).toBeVisible();
  await expect(editor(page)).not.toBeInViewport();
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(editor(page)).toBeInViewport({ ratio: 1 });
  const text = editor(page).locator("textarea");
  await expect(text).toBeFocused();
  await expect(text).toHaveValue("Half a thought");
});

test("Ctrl+Alt+M from focus mode leaves the editor in view", async ({ page }) => {
  await openFixture(page, PLAN);
  await page.keyboard.press("F11");
  await expect(page.locator("body")).toHaveClass(/focus/);
  // Reader input ends the hold that entering focus mode started, so the drag lands where aimed.
  await page.keyboard.press("Shift");
  await dragSelect(page, "Section 2 covers the search area");
  // The selection below the view, the pane still scrolled: leaving focus mode holds that place.
  const scrolled = await page.evaluate(() => {
    const pane = document.getElementById("lx-doc-pane");
    if (!pane) return 0;
    pane.scrollTop -= 1500;
    return pane.scrollTop;
  });
  expect(scrolled).toBeGreaterThan(0);
  await page.keyboard.press("Control+Alt+KeyM");
  await expect(page.locator("body")).not.toHaveClass(/focus/);
  await expect(editor(page)).toBeVisible();
  // Past the time the old place would have been held for.
  await page.waitForTimeout(1600);
  await expect(editor(page)).toBeInViewport({ ratio: 1 });
  // The window leaving full screen shrinks some frames later: the editor stays in view.
  await page.setViewportSize({ width: 1400, height: 600 });
  await expect(editor(page)).toBeInViewport({ ratio: 1 });
  await expect(editor(page).locator("textarea")).toBeFocused();
});

test("find in page paints over comment highlights", async ({ page }) => {
  await openFixture(page, ALPHA);
  await seed(page, alphaReview());
  await expect.poll(() => highlighted(page)).toBe(true);
  await page.keyboard.press("Control+f");
  await expect(page.getByRole("textbox", { name: "Find" })).toBeFocused();
  await page.keyboard.type("platform");
  await expect.poll(() => page.evaluate(() => CSS.highlights.has("find-current"))).toBe(true);
  expect(
    await page.evaluate(() =>
      ["comment", "find", "find-current"].map((name) => CSS.highlights.get(name)?.priority),
    ),
  ).toEqual([0, 2, 3]);
});
