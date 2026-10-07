import { describe, expect, it } from "vitest";
import {
  agentEntryCount,
  formatCopy,
  isOpen,
  lineLabel,
  matchPart,
  orderComments,
  placeLabel,
} from "../src/comments-model";
import type { ClaudeKind } from "../src/generated/ClaudeKind";
import type { CommentView } from "../src/generated/CommentView";
import type { EntryView } from "../src/generated/EntryView";
import type { ReviewPayload } from "../src/generated/ReviewPayload";

const NOTE = "S:\\Notes\\My Vault\\work\\tide\\plans\\2026-03-02-tide-sync.md";
const SIDECAR = "S:\\Notes\\My Vault\\work\\tide\\plans\\2026-03-02-tide-sync.review.md";

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
    textStart: null,
    textEnd: null,
    currentText: null,
    entries: [],
    ...over,
  };
}

function you(text: string): EntryView {
  return { author: "you", name: "You", kind: null, text, html: `<p>${text}</p>` };
}

function agent(name: string, kind: ClaudeKind | null, text: string): EntryView {
  return { author: "agent", name, kind, text, html: `<p>${text}</p>` };
}

const DETACHED = comment({
  id: 2,
  state: "detached",
  startLine: 88,
  endLine: 88,
  headingPath: ["Lantern rollout"],
  jumpLine: null,
  quote: "keeps a lock per notebook",
  entries: [you("Is this still needed?")],
});
const ANCHORED = comment({
  id: 3,
  status: "question",
  startLine: 120,
  endLine: 128,
  headingPath: ["Tide sync", "Batching"],
  jumpLine: 120,
  quote: "sends at most 50 changes per batch",
  entries: [
    you("Why 50? Make it a setting."),
    agent("Codex", "question", "Per device or per notebook?"),
  ],
});
const MOVED = comment({
  id: 4,
  state: "moved",
  startLine: 40,
  endLine: 40,
  headingPath: ["Setup"],
  jumpLine: 40,
  quote: "install the old helper",
  currentText: "install the new helper from the share",
  entries: [
    you("Which helper?\nThe old one is gone."),
    agent("Claude", null, "The one on the share."),
  ],
});
const RESOLVED = comment({
  id: 5,
  status: "resolved",
  startLine: 12,
  endLine: 12,
  jumpLine: 12,
  quote: "two lines\nof quote",
  entries: [you("Typo here."), agent("GitHub Copilot", "resolved", "Fixed it.")],
});

function payload(wsl: boolean): ReviewPayload {
  return {
    notePath: NOTE,
    sidecarPath: SIDECAR,
    noteWslPath: wsl ? "/mnt/s/Notes/My Vault/work/tide/plans/2026-03-02-tide-sync.md" : null,
    sidecarWslPath: wsl
      ? "/mnt/s/Notes/My Vault/work/tide/plans/2026-03-02-tide-sync.review.md"
      : null,
    exists: true,
    readOnly: null,
    comments: [ANCHORED, RESOLVED, DETACHED, MOVED],
    unreadable: [],
    openCount: 3,
  };
}

const HEADER_WSL = [
  `Review comments on ${NOTE}`,
  "  WSL: /mnt/s/Notes/My Vault/work/tide/plans/2026-03-02-tide-sync.md",
  `(sidecar: ${SIDECAR} — reply there using its format, or edit the note directly)`,
  "  WSL: /mnt/s/Notes/My Vault/work/tide/plans/2026-03-02-tide-sync.review.md",
];
const HEADER = [
  `Review comments on ${NOTE}`,
  `(sidecar: ${SIDECAR} — reply there using its format, or edit the note directly)`,
];
const C2 = [
  'C2 [detached — original text: "keeps a lock per notebook"] (was L88 · Lantern rollout)',
  "  You: Is this still needed?",
];
const C3 = [
  "C3 [question] L120–L128 · Tide sync › Batching",
  "  > sends at most 50 changes per batch",
  "  You: Why 50? Make it a setting.",
  "  Codex (question): Per device or per notebook?",
];
const C4 = [
  "C4 [open, text changed] L40 · Setup",
  "  > install the old helper",
  "  now: install the new helper from the share",
  "  You: Which helper?",
  "    The old one is gone.",
  "  Claude: The one on the share.",
];
const C5 = [
  "C5 [resolved] L12",
  "  > two lines",
  "  > of quote",
  "  You: Typo here.",
  "  GitHub Copilot (resolved): Fixed it.",
];

/** The copy's lines: blocks separated by one blank line, ending with one newline. */
function text(...blocks: string[][]): string {
  return `${blocks.map((b) => b.join("\n")).join("\n\n")}\n`;
}

describe("comments model", () => {
  it("orders detached first, then by line", () => {
    const sameLine = comment({ id: 1, startLine: 40, endLine: 41 });
    const { detached, attached } = orderComments([...payload(true).comments, sameLine], "all");
    expect(detached.map((c) => c.id)).toEqual([2]);
    expect(attached.map((c) => c.id)).toEqual([5, 1, 4, 3]);
  });

  it("filters open", () => {
    const dismissed = comment({ id: 6, status: "dismissed" });
    const replied = comment({ id: 7, status: "replied" });
    const pushback = comment({ id: 8, status: "pushback" });
    const { detached, attached } = orderComments(
      [...payload(true).comments, dismissed, replied, pushback],
      "open",
    );
    expect(detached.map((c) => c.id)).toEqual([2]);
    expect(attached.map((c) => c.id).sort()).toEqual([3, 4, 7, 8]);
    expect(isOpen(RESOLVED)).toBe(false);
    expect(isOpen(dismissed)).toBe(false);
    expect(isOpen(ANCHORED)).toBe(true);
  });

  it("labels lines and places", () => {
    expect(lineLabel(RESOLVED)).toBe("L12");
    expect(lineLabel(ANCHORED)).toBe("L120–L128");
    expect(placeLabel(ANCHORED)).toBe("L120–L128 · Tide sync › Batching");
    expect(placeLabel(RESOLVED)).toBe("L12");
  });

  it("formats copy exactly", () => {
    expect(formatCopy(payload(true), { includeResolved: false })).toBe(
      text(HEADER_WSL, C2, C4, C3),
    );
    expect(formatCopy(payload(true), { includeResolved: true })).toBe(
      text(HEADER_WSL, C2, C5, C4, C3),
    );
    expect(formatCopy(payload(false), { includeResolved: false })).toBe(text(HEADER, C2, C4, C3));
    expect(formatCopy(payload(false), { includeResolved: true })).toBe(
      text(HEADER, C2, C5, C4, C3),
    );
  });

  it("says a detached comment's status when it isn't plain open", () => {
    const p = payload(false);
    p.comments = [{ ...DETACHED, status: "resolved" }];
    expect(formatCopy(p, { includeResolved: true })).toBe(
      text(HEADER, [
        'C2 [resolved, detached — original text: "keeps a lock per notebook"] (was L88 · Lantern rollout)',
        "  You: Is this still needed?",
      ]),
    );
  });

  it("copies one comment by id", () => {
    expect(formatCopy(payload(false), { includeResolved: true, ids: [5] })).toBe(text(HEADER, C5));
    expect(formatCopy(payload(true), { includeResolved: false, ids: [3] })).toBe(
      text(HEADER_WSL, C3),
    );
    expect(formatCopy(payload(false), { includeResolved: false, ids: [5] })).toBe(text(HEADER));
  });

  it("counts agent entries, whatever the agent", () => {
    expect(agentEntryCount(payload(true))).toBe(3);
    expect(agentEntryCount(null)).toBe(0);
  });

  it("matches a capped quote without its ellipsis", () => {
    const head = "x".repeat(500);
    expect(matchPart(`${head}…`)).toBe(head);
    expect(matchPart("short…")).toBe("short…");
    expect(matchPart("plain")).toBe("plain");
  });
});
