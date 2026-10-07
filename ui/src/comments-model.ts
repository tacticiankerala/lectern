// Review comments as data: which are open, the order the Comments tab lists them in, their labels,
// and the plain text "Copy comments" puts on the clipboard for pasting into an AI agent. No DOM.
import type { CommentView } from "./generated/CommentView";
import type { EntryView } from "./generated/EntryView";
import type { ReviewPayload } from "./generated/ReviewPayload";

/** Which comments the Comments tab lists: the open ones, or all of them. */
export type Filter = "open" | "all";

/** As core caps a stored quote: this many characters, then an ellipsis. */
export const QUOTE_CAP = 500;

/** Still open: neither resolved nor dismissed. An agent's reply or question keeps it open. */
export function isOpen(c: CommentView): boolean {
  return c.status !== "resolved" && c.status !== "dismissed";
}

/**
 * The comments `filter` lets through, as the tab lists them: the detached first, then the rest,
 * each by start line, then id.
 */
export function orderComments(
  list: CommentView[],
  filter: Filter,
): { detached: CommentView[]; attached: CommentView[] } {
  const shown = list
    .filter((c) => filter === "all" || isOpen(c))
    .sort((a, b) => a.startLine - b.startLine || a.id - b.id);
  return {
    detached: shown.filter((c) => c.state === "detached"),
    attached: shown.filter((c) => c.state !== "detached"),
  };
}

/** "L12", or "L12–L14" for a range. */
export function lineLabel(c: CommentView): string {
  const start = `L${String(c.startLine)}`;
  return c.endLine > c.startLine ? `${start}–L${String(c.endLine)}` : start;
}

/** The lines and the heading path: "L12–L14 · Tide sync › Batching". */
export function placeLabel(c: CommentView): string {
  return c.headingPath.length === 0
    ? lineLabel(c)
    : `${lineLabel(c)} · ${c.headingPath.join(" › ")}`;
}

/** The part of a stored quote that is the note's text: without the ellipsis a cap added. */
export function matchPart(quote: string): string {
  if (!quote.endsWith("…")) {
    return quote;
  }
  const head = quote.slice(0, -1);
  return Array.from(head).length === QUOTE_CAP ? head : quote;
}

/** How many entries AI agents have written, across every comment. */
export function agentEntryCount(p: ReviewPayload | null): number {
  let n = 0;
  for (const c of p?.comments ?? []) {
    n += c.entries.filter((e) => e.author === "agent").length;
  }
  return n;
}

/**
 * The comments as plain text for an AI agent: the note and its sidecar, then each comment with its
 * quote and thread, detached first, then by line. Only the open ones unless `includeResolved`;
 * only those in `ids` when given.
 */
export function formatCopy(
  p: ReviewPayload,
  opts: { includeResolved: boolean; ids?: number[] },
): string {
  const lines = [`Review comments on ${p.notePath}`];
  if (p.noteWslPath !== null) lines.push(`  WSL: ${p.noteWslPath}`);
  lines.push(
    `(sidecar: ${p.sidecarPath} — reply there using its format, or edit the note directly)`,
  );
  if (p.sidecarWslPath !== null) lines.push(`  WSL: ${p.sidecarWslPath}`);
  const wanted = opts.ids === undefined ? null : new Set(opts.ids);
  const picked = p.comments.filter((c) => wanted === null || wanted.has(c.id));
  const { detached, attached } = orderComments(picked, opts.includeResolved ? "all" : "open");
  for (const c of [...detached, ...attached]) {
    lines.push("", ...copyLines(c));
  }
  return `${lines.join("\n")}\n`;
}

/** The number of comments `formatCopy` would copy with these options. */
export function copyCount(
  p: ReviewPayload,
  opts: { includeResolved: boolean; ids?: number[] },
): number {
  const wanted = opts.ids === undefined ? null : new Set(opts.ids);
  return p.comments.filter(
    (c) => (wanted === null || wanted.has(c.id)) && (opts.includeResolved || isOpen(c)),
  ).length;
}

/** One comment's lines in the copy. */
function copyLines(c: CommentView): string[] {
  const id = `C${String(c.id)}`;
  if (c.state === "detached") {
    // The status is said when it isn't plain "open", which the format leaves unsaid.
    const status = c.status === "open" ? "" : `${c.status}, `;
    const quote = c.quote.replace(/\s+/g, " ").trim();
    return [
      `${id} [${status}detached — original text: "${quote}"] (was ${placeLabel(c)})`,
      ...c.entries.map(entryLines).flat(),
    ];
  }
  const state = c.state === "moved" ? `${c.status}, text changed` : c.status;
  const out = [`${id} [${state}] ${placeLabel(c)}`];
  for (const line of c.quote.split("\n")) {
    out.push(`  > ${line}`);
  }
  if (c.state === "moved" && c.currentText !== null) {
    out.push(...continued("  now: ", c.currentText));
  }
  out.push(...c.entries.map(entryLines).flat());
  return out;
}

/** An entry's lines: who wrote it (an agent by name, with its kind), then its text. */
function entryLines(e: EntryView): string[] {
  const who = e.author === "you" ? "You" : e.kind === null ? e.name : `${e.name} (${e.kind})`;
  return continued(`  ${who}: `, e.text);
}

/** `text` after `first` on its first line, its further lines indented four spaces. */
function continued(first: string, text: string): string[] {
  const [head = "", ...rest] = text.split("\n");
  return [`${first}${head}`, ...rest.map((line) => `    ${line}`)];
}
