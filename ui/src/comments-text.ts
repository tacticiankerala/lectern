// The note's visible text as the review anchors see it, read from the rendered page: the UI's side
// of the contract with core (crates/lectern-core/src/review/text.rs), which builds the same text
// from the Markdown.
//
// A text node counts when its nearest block-level ancestor carries `data-sourcepos`: core's leaf
// blocks do, raw HTML blocks (`<details>`, a raw `<div>`) don't, and neither does the chrome the
// renderer adds inside a block, such as an alert's title or a code block's note. A `<pre>` doesn't
// count as a block: its code belongs to the `.code-block` around it. What `SKIP` matches is never
// text. Each block's text is joined to the next by a space, and every run of whitespace becomes
// one space, trimmed, as core's `normalize` does.

/** Chrome inside blocks that core never sees, and the comment marks this module's users add. */
export const SKIP =
  ".code-head, .footnote-backref, .footnote-ref, .markdown-alert-title, .lx-cdot, .lx-block-plus, input, .img-placeholder";

/** Rust's `char::is_whitespace`, which core's `normalize` splits on (unlike `\s`, no U+FEFF). */
const WHITESPACE = /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/;
const WHITESPACE_RUNS = new RegExp(`${WHITESPACE.source}+`, "g");

/** Elements that make a block of text. `PRE` isn't one here: see the module comment. */
const BLOCKS = new Set([
  "ADDRESS",
  "ARTICLE",
  "ASIDE",
  "BLOCKQUOTE",
  "CAPTION",
  "DD",
  "DETAILS",
  "DIV",
  "DL",
  "DT",
  "FIELDSET",
  "FIGCAPTION",
  "FIGURE",
  "FOOTER",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "HEADER",
  "HR",
  "LI",
  "MAIN",
  "NAV",
  "OL",
  "P",
  "SECTION",
  "SUMMARY",
  "TABLE",
  "TBODY",
  "TD",
  "TFOOT",
  "TH",
  "THEAD",
  "TR",
  "UL",
]);

/**
 * The footnotes' wrapper, as the renderer writes it: its lines are only its first definition's,
 * so walks over the blocks look through it.
 */
const FOOTNOTES = "section.footnotes";

/** The blocks a comment's dot can sit on: a table's rows rather than their cells. */
const DOT_BLOCKS = new Set([...BLOCKS].filter((tag) => tag !== "TD" && tag !== "TH"));

/** `s` with every run of whitespace turned into one space, and trimmed, as core's `normalize`. */
export function normalize(s: string): string {
  return s.replace(WHITESPACE_RUNS, " ").trim();
}

/**
 * Visible text and where each of its characters comes from: `text[i]` is
 * `nodes[nodeOf[i]].data[offOf[i]]`, except for a space that joins two blocks, which points just
 * past the end of the node before it.
 */
export interface TextIndex {
  text: string;
  nodes: Text[];
  nodeOf: Uint32Array;
  offOf: Uint32Array;
}

/** The visible text of `roots`, in order. */
export function buildTextIndex(roots: Node[]): TextIndex {
  const nodes: Text[] = [];
  const chars: string[] = [];
  const nodeOf: number[] = [];
  const offOf: number[] = [];
  let pendingSpace = false;
  let lastBlock: Element | null = null;
  for (const root of roots) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode: (node) =>
        node instanceof Element && node.matches(SKIP)
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_ACCEPT,
    });
    for (let node: Node | null = walker.currentNode; node; node = walker.nextNode()) {
      if (!(node instanceof Text) || node.data === "") {
        continue;
      }
      const block = countingBlock(node);
      if (block === null) {
        continue;
      }
      if (block !== lastBlock) {
        pendingSpace = true;
        lastBlock = block;
      }
      const n = nodes.length;
      nodes.push(node);
      const data = node.data;
      for (let i = 0; i < data.length; i++) {
        const ch = data.charAt(i);
        if (WHITESPACE.test(ch)) {
          pendingSpace = true;
          continue;
        }
        if (pendingSpace && chars.length > 0) {
          // A space inside this node stands for the run; one between blocks for the join.
          chars.push(" ");
          const inNode = i > 0 && WHITESPACE.test(data.charAt(i - 1));
          nodeOf.push(inNode ? n : Math.max(0, n - 1));
          offOf.push(inNode ? i - 1 : (nodes[Math.max(0, n - 1)]?.data.length ?? 0));
        }
        pendingSpace = false;
        chars.push(ch);
        nodeOf.push(n);
        offOf.push(i);
      }
    }
  }
  return {
    text: chars.join(""),
    nodes,
    nodeOf: Uint32Array.from(nodeOf),
    offOf: Uint32Array.from(offOf),
  };
}

/** The note's visible text, as core's text map has it. */
export function docText(doc: Element): string {
  return buildTextIndex([doc]).text;
}

/** The first place `needle` (normalised) is in the text, as a Range; null when it isn't there. */
export function locate(index: TextIndex, needle: string): Range | null {
  const wanted = normalize(needle);
  if (wanted === "") {
    return null;
  }
  const at = index.text.indexOf(wanted);
  if (at === -1) {
    return null;
  }
  const last = at + wanted.length - 1;
  const startNode = index.nodes[index.nodeOf[at] ?? 0];
  const endNode = index.nodes[index.nodeOf[last] ?? 0];
  if (!startNode || !endNode) {
    return null;
  }
  const range = document.createRange();
  range.setStart(startNode, index.offOf[at] ?? 0);
  range.setEnd(endNode, Math.min(endNode.length, (index.offOf[last] ?? 0) + 1));
  return range;
}

/** The visible text a selection covers, normalised: what a comment on it quotes. */
export function quoteFromRange(range: Range): string {
  const common = range.commonAncestorContainer;
  const root = common instanceof Element ? common : common.parentElement;
  if (!root) {
    return "";
  }
  const index = buildTextIndex([root]);
  // Each node's characters inside the range, as [from, to).
  const spans = index.nodes.map((node): [number, number] => {
    if (!range.intersectsNode(node)) {
      return [0, 0];
    }
    const from = node === range.startContainer ? range.startOffset : 0;
    const to = node === range.endContainer ? range.endOffset : node.length;
    return [from, to];
  });
  let first = -1;
  let last = -1;
  for (let i = 0; i < index.text.length; i++) {
    if (index.text[i] === " ") {
      continue;
    }
    const [from, to] = spans[index.nodeOf[i] ?? 0] ?? [0, 0];
    const off = index.offOf[i] ?? 0;
    if (off >= from && off < to) {
      if (first === -1) first = i;
      last = i;
    }
  }
  return first === -1 ? "" : index.text.slice(first, last + 1);
}

/**
 * An element's source lines from its `data-sourcepos` (`12:1-14:3`); an end at column 0 is the
 * end of the line before. Null without one.
 */
export function blockLines(el: Element): [number, number] | null {
  const match = /^(\d+):\d+-(\d+):(\d+)$/.exec(el.getAttribute("data-sourcepos") ?? "");
  if (!match) {
    return null;
  }
  const start = Number(match[1]);
  const endLine = Number(match[2]);
  const end = Number(match[3]) === 0 ? endLine - 1 : endLine;
  return [start, Math.max(start, end)];
}

/** The block a text node's text counts in (see the module comment), or null when it doesn't. */
export function leafBlockOf(node: Node, doc: Element): HTMLElement | null {
  const block = nearestBlock(node, doc);
  return block?.hasAttribute("data-sourcepos") ? block : null;
}

/**
 * The deepest block holding source line `line`: a list item rather than its list, a table row
 * rather than its table (never a cell). Null when no block does.
 */
export function blockForLine(doc: Element, line: number): HTMLElement | null {
  let best: HTMLElement | null = null;
  // Down from the top: the first block holding the line at each level, then into it.
  const visit = (parent: Element): boolean => {
    for (const child of parent.children) {
      if (!(child instanceof HTMLElement) || !BLOCKS.has(child.tagName)) {
        continue;
      }
      const lines = walkLines(child);
      if (lines === null) {
        // No lines of its own (a table's wrapper or body, raw HTML): what's inside may have them.
        if (visit(child)) return true;
        continue;
      }
      if (lines[0] <= line && line <= lines[1]) {
        if (DOT_BLOCKS.has(child.tagName)) best = child;
        visit(child);
        return true;
      }
    }
    return false;
  };
  visit(doc);
  return best;
}

/**
 * The blocks covering source lines `start` to `end`, in order and none inside another: a block
 * wholly inside the lines comes whole, one reaching past them gives the blocks inside it that
 * touch them, or itself when it has none. A block reaching past them also comes whole when text
 * of its own (a tight list item's, outside its nested list) is on the lines, so that text isn't
 * lost. A table's wrapper is looked through.
 */
export function elementsForLines(doc: Element, start: number, end: number): HTMLElement[] {
  const out: HTMLElement[] = [];
  const visit = (parent: Element): void => {
    for (const [child, lines] of linedBlocks(parent)) {
      if (lines[1] < start || lines[0] > end) {
        continue;
      }
      if (start <= lines[0] && lines[1] <= end) {
        out.push(child);
        continue;
      }
      if (ownTextOnLines(child, lines, start, end)) {
        out.push(child);
        continue;
      }
      const before = out.length;
      visit(child);
      if (out.length === before) {
        out.push(child);
      }
    }
  };
  visit(doc);
  return out;
}

/**
 * The blocks with lines just inside `parent`, with their lines, looking through blocks without
 * (a table's wrapper or body, raw HTML, the footnotes' wrapper).
 */
function linedBlocks(parent: Element): [HTMLElement, [number, number]][] {
  const out: [HTMLElement, [number, number]][] = [];
  for (const child of parent.children) {
    if (!(child instanceof HTMLElement) || !BLOCKS.has(child.tagName)) {
      continue;
    }
    const lines = walkLines(child);
    if (lines === null) {
      out.push(...linedBlocks(child));
    } else {
      out.push([child, lines]);
    }
  }
  return out;
}

/**
 * Whether `el` holds text outside its child blocks, and one of lines `start` to `end` that it
 * spans falls outside them: where that text is.
 */
function ownTextOnLines(
  el: HTMLElement,
  lines: [number, number],
  start: number,
  end: number,
): boolean {
  const owns = [...el.childNodes].some((node) =>
    node instanceof Text
      ? normalize(node.data) !== ""
      : node instanceof HTMLElement &&
        !BLOCKS.has(node.tagName) &&
        !node.matches(SKIP) &&
        normalize(node.textContent) !== "",
  );
  if (!owns) {
    return false;
  }
  const children = linedBlocks(el).map(([, l]) => l);
  for (let line = Math.max(start, lines[0]); line <= Math.min(end, lines[1]); line++) {
    if (!children.some(([from, to]) => from <= line && line <= to)) {
      return true;
    }
  }
  return false;
}

/** A block's lines for walking the blocks: none for the footnotes' wrapper (see `FOOTNOTES`). */
function walkLines(el: Element): [number, number] | null {
  return el.matches(FOOTNOTES) ? null : blockLines(el);
}

/** The text node's nearest block-level ancestor below `stop` (a `.doc` when null), or null. */
function nearestBlock(node: Node, stop: Element | null): HTMLElement | null {
  for (let el = node.parentElement; el && el !== stop; el = el.parentElement) {
    if (stop === null && el.classList.contains("doc")) {
      return null;
    }
    if (el instanceof HTMLElement && BLOCKS.has(el.tagName)) {
      return el;
    }
  }
  return null;
}

/** The block a text node counts in, or null when its text isn't part of the note's. */
function countingBlock(node: Text): HTMLElement | null {
  const block = nearestBlock(node, null);
  return block?.hasAttribute("data-sourcepos") ? block : null;
}
