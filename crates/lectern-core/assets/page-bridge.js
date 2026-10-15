// Lectern's page bridge. Lectern inserts it at the top of every HTML page it shows
// (lectern_core::page), where it runs before the page's own scripts, in the page's sandboxed frame,
// even when the page's scripts are off. It speaks to Lectern only by postMessage, in plain objects
// tagged `lx: 1`, and listens to nothing but its parent window.
//
// To Lectern: `ready` once the page has loaded (its title, headings, text blocks, scroll state and
// colour scheme), `blocks` again when the page changes, `selection` (with the box of its last line,
// to place Lectern's Comment button by), `scroll`, `scheme`, `link` for a clicked link, which
// Lectern follows by its own rules instead of the frame, and `comment-click` with the offset of a
// plain click on text a comment marks.
// From Lectern: `theme` (the highlight colours), `paint` and `clear` (highlights by layer), and
// `scroll-to` (a fraction of the page, or an offset into its text).
//
// A place restored by fraction is held while the page grows (images loading), until the reader
// scrolls or a few seconds pass. When the page rebuilds nodes but keeps its text, the highlights
// are painted again on the new ones. Scrolling to an offset opens the closed `<details>` hiding it.
// Each reading of the page maps its text to the page once, in segments, which endpoints are looked
// up in by binary search.
//
// The page's text: its leaf blocks with text of their own (paragraphs, list items, headings, table
// cells and so on, and a div or section with loose text), in order, each with its whitespace
// collapsed and trimmed, joined by single spaces. Offsets are UTF-16 code units into that text,
// and the same walk maps them back to the page. Scripts, styles, templates, SVG and hidden
// elements hold no text.
//
// The page can post to Lectern itself, so Lectern trusts none of this: it checks every message.
(function (window) {
  "use strict";

  var parent = window.parent;
  if (!parent || parent === window) {
    return;
  }
  var document = window.document;

  /** The most text reported, in UTF-16 code units: whole blocks up to it. */
  var MAX_TEXT = 2 * 1024 * 1024;
  /** How long the page must stay unchanged before its blocks are reported again. */
  var CHANGE_MS = 500;
  /** How long a page that keeps changing waits, at most, before its blocks are reported again. */
  var CHANGE_MAX_MS = 2000;
  /** How long a restored place is held while the page grows (images loading), at most. */
  var SETTLE_MS = 5000;
  /** How long the selection must stay put before it is reported. */
  var SELECTION_MS = 100;
  /** How far below the top an offset scrolled to lands, in pixels. */
  var TOP_GAP = 12;
  /** The highlight layers Lectern paints, with their priorities: find's over comments'. */
  var LAYERS = { comment: 0, "comment-focus": 1, find: 2, "find-current": 3 };
  /** The most ranges painted in one layer. */
  var MAX_RANGES = 10000;
  /** What the outline lists at most: headings, each heading's text, an id (else none) and the title. */
  var MAX_HEADINGS = 5000;
  var MAX_HEADING_TEXT = 300;
  var MAX_ID = 200;
  var MAX_TITLE = 1000;

  /** Elements that hold no text. */
  var SKIP = {
    script: 1,
    style: 1,
    noscript: 1,
    template: 1,
    svg: 1,
    head: 1,
  };
  /** Elements that make a block of text, when text of their own is in them. */
  var BLOCK = {
    p: 1,
    li: 1,
    h1: 1,
    h2: 1,
    h3: 1,
    h4: 1,
    h5: 1,
    h6: 1,
    td: 1,
    th: 1,
    pre: 1,
    blockquote: 1,
    dt: 1,
    dd: 1,
    figcaption: 1,
    caption: 1,
    summary: 1,
    label: 1,
    div: 1,
    section: 1,
    // Other containers, so loose text in them is still text.
    address: 1,
    article: 1,
    aside: 1,
    body: 1,
    details: 1,
    dialog: 1,
    dl: 1,
    fieldset: 1,
    figure: 1,
    footer: 1,
    form: 1,
    header: 1,
    hgroup: 1,
    legend: 1,
    main: 1,
    nav: 1,
    ol: 1,
    table: 1,
    tbody: 1,
    tfoot: 1,
    thead: 1,
    tr: 1,
    ul: 1,
  };
  var HEADING = /^h([1-6])$/;

  function post(message) {
    message.lx = 1;
    try {
      parent.postMessage(message, "*");
    } catch (e) {
      // Lectern is gone; nothing to tell.
    }
  }

  /** Rust's `char::is_whitespace`, for a UTF-16 code unit: what Lectern's text collapses. */
  function isSpace(c) {
    return (
      (c >= 9 && c <= 13) ||
      c === 32 ||
      c === 0x85 ||
      c === 0xa0 ||
      c === 0x1680 ||
      (c >= 0x2000 && c <= 0x200a) ||
      c === 0x2028 ||
      c === 0x2029 ||
      c === 0x202f ||
      c === 0x205f ||
      c === 0x3000
    );
  }

  function childIndex(node) {
    var i = 0;
    for (var n = node.previousSibling; n; n = n.previousSibling) {
      i++;
    }
    return i;
  }

  // ---- The text ----

  /** The page's text: `{ text, blocks }`, each block `{ el, pieces, start, text, level }`. */
  var model = { text: "", blocks: [] };
  /** Numbers the ids made up for headings that have none. */
  var headingIds = 0;

  /**
   * Calls `visit(out, node, off, width, text)` for each run of a block's text, from its pieces (text
   * nodes and `<br>`s) in order: characters as they stand in a text node (`width` of them, from
   * `off`), or one space for the whitespace between two runs (`width` 1: the first character of that
   * whitespace, or a `<br>` as `node`'s child `off`). `out` is where the run starts in the block's
   * text. Leading and trailing whitespace makes nothing. Stops once `visit` returns true.
   */
  function runs(pieces, visit) {
    var out = 0;
    var gap = null;
    for (var p = 0; p < pieces.length; p++) {
      var piece = pieces[p];
      if (piece.nodeType !== 3) {
        if (out > 0 && gap === null && piece.parentNode) {
          gap = [piece.parentNode, childIndex(piece)];
        }
        continue;
      }
      var data = piece.data;
      var k = 0;
      while (k < data.length) {
        if (isSpace(data.charCodeAt(k))) {
          if (out > 0 && gap === null) {
            gap = [piece, k];
          }
          k++;
          continue;
        }
        var end = k + 1;
        while (end < data.length && !isSpace(data.charCodeAt(end))) {
          end++;
        }
        if (gap !== null) {
          if (visit(out, gap[0], gap[1], 1, " ")) {
            return;
          }
          out++;
          gap = null;
        }
        if (visit(out, piece, k, end - k, data.slice(k, end))) {
          return;
        }
        out += end - k;
        k = end;
      }
    }
  }

  /** Whether `el` shows nothing: hidden, or not displayed. */
  function hidden(el) {
    if (el.hidden) {
      return true;
    }
    var style = window.getComputedStyle(el);
    return style !== null && style.display === "none";
  }

  /** The block `node` is in: its nearest block element, or `root`. */
  function blockElement(node, root) {
    for (var el = node.parentElement; el && el !== root; el = el.parentElement) {
      if (BLOCK[el.localName] === 1) {
        return el;
      }
    }
    return root;
  }

  /** Reads the page's text again (see the module comment). */
  function read() {
    var root = document.body || document.documentElement;
    var blocks = [];
    if (!root) {
      model = { text: "", blocks: blocks };
      return;
    }
    var invisible = new Map();
    var walker = document.createTreeWalker(root, 1 | 4, {
      acceptNode: function (node) {
        if (node.nodeType === 1 && (SKIP[node.localName] === 1 || hidden(node))) {
          return 2; // FILTER_REJECT: nothing below it either
        }
        return 1;
      },
    });
    var current = null;
    for (var node = walker.nextNode(); node; node = walker.nextNode()) {
      var isText = node.nodeType === 3;
      if (!isText && node.localName !== "br") {
        continue;
      }
      var owner = node.parentElement;
      if (isText && owner) {
        var unseen = invisible.get(owner);
        if (unseen === undefined) {
          var style = window.getComputedStyle(owner);
          unseen =
            style !== null && (style.visibility === "hidden" || style.visibility === "collapse");
          invisible.set(owner, unseen);
        }
        if (unseen) {
          continue;
        }
      }
      var el = blockElement(node, root);
      if (current === null || current.el !== el) {
        if (!isText) {
          continue;
        }
        current = { el: el, pieces: [] };
        blocks.push(current);
      }
      current.pieces.push(node);
    }
    var kept = [];
    var length = 0;
    for (var i = 0; i < blocks.length; i++) {
      var block = blocks[i];
      var parts = [];
      var segments = [];
      runs(block.pieces, function (out, node, off, width, text) {
        parts.push(text);
        // Text that maps one to one onto its node goes on in the same segment.
        var last = segments[segments.length - 1];
        if (last && last.node === node && last.off + last.width === off) {
          last.width += width;
        } else {
          segments.push({ out: out, node: node, off: off, width: width });
        }
      });
      block.text = parts.join("");
      block.segments = segments;
      if (block.text === "") {
        continue;
      }
      var join = kept.length > 0 ? 1 : 0;
      if (length + join + block.text.length > MAX_TEXT) {
        break;
      }
      block.start = length + join;
      length = block.start + block.text.length;
      var heading = HEADING.exec(block.el.localName);
      block.level = heading ? Number(heading[1]) : 0;
      kept.push(block);
    }
    model = {
      text: kept
        .map(function (block) {
          return block.text;
        })
        .join(" "),
      blocks: kept,
    };
  }

  /** The blocks as Lectern takes them: their text, and a heading's level. */
  function blockList() {
    return model.blocks.map(function (block) {
      return block.level > 0
        ? { text: block.text, headingLevel: block.level }
        : { text: block.text };
    });
  }

  /**
   * The headings, each once, with an id (one is made up where it has none; one too long to list
   * goes as none) and its offset, as many as the outline lists.
   */
  function headingList() {
    var seen = new Set();
    var list = [];
    for (var i = 0; i < model.blocks.length && list.length < MAX_HEADINGS; i++) {
      var block = model.blocks[i];
      if (block.level === 0 || seen.has(block.el)) {
        continue;
      }
      seen.add(block.el);
      if (!block.el.id) {
        var id;
        do {
          headingIds++;
          id = "lx-heading-" + headingIds;
        } while (document.getElementById(id));
        block.el.id = id;
      }
      list.push({
        level: block.level,
        text: block.text.slice(0, MAX_HEADING_TEXT),
        id: block.el.id.length > MAX_ID ? "" : block.el.id,
        offset: block.start,
      });
    }
    return list;
  }

  /** The block holding text offset `offset`; at a space between two blocks, `after` picks the next. */
  function blockAt(offset, after) {
    var blocks = model.blocks;
    var lo = 0;
    var hi = blocks.length - 1;
    var found = -1;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      var b = blocks[mid];
      var starts = after ? b.start <= offset : b.start < offset;
      if (starts) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    return found < 0 ? blocks[0] : blocks[found];
  }

  /**
   * The point in the page at text offset `offset`: before the character there, or, for an `end`,
   * after the one before it. Null when the page has no such offset.
   */
  function point(offset, end) {
    if (model.blocks.length === 0 || offset < 0 || offset > model.text.length) {
      return null;
    }
    var block = blockAt(offset, !end);
    var local = Math.max(0, Math.min(offset - block.start, block.text.length));
    var at = end ? local - 1 : local;
    if (at < 0) {
      at = 0;
      end = false;
    }
    if (at >= block.text.length) {
      at = block.text.length - 1;
      end = true;
    }
    // The last segment that starts at or before the character.
    var segments = block.segments;
    var lo = 0;
    var hi = segments.length - 1;
    var found = 0;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      if (segments[mid].out <= at) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    var segment = segments[found];
    return segment ? [segment.node, segment.off + (at - segment.out) + (end ? 1 : 0)] : null;
  }

  /** A range over text offsets `start` to `end`, or null. */
  function rangeOf(start, end) {
    var from = point(start, false);
    var to = point(end, true);
    if (!from || !to) {
      return null;
    }
    try {
      var range = document.createRange();
      range.setStart(from[0], from[1]);
      range.setEnd(to[0], to[1]);
      return range;
    } catch (e) {
      return null;
    }
  }

  /** Where `probe` (a collapsed range) falls relative to the page point `node`, `off`: -1, 0 or 1. */
  function compare(probe, node, off) {
    try {
      return -probe.comparePoint(node, off);
    } catch (e) {
      return 1;
    }
  }

  /**
   * The text offset of the page point `container`, `off`, or -1 when it can't be placed. A point
   * between two blocks (or past the last) counts as where the next block starts, or, for the `end`
   * of a selection, where the one before it ends.
   */
  function offsetOf(container, off, end) {
    var blocks = model.blocks;
    if (blocks.length === 0) {
      return -1;
    }
    var probe;
    try {
      probe = document.createRange();
      probe.setStart(container, off);
    } catch (e) {
      return -1;
    }
    // The last block that starts at or before the point.
    var lo = 0;
    var hi = blocks.length - 1;
    var found = 0;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      var first = blocks[mid].pieces[0];
      if (compare(probe, first, 0) >= 0) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    var block = blocks[found];
    var next = blocks[found + 1];
    var result = !end && next ? next.start : block.start + block.text.length;
    for (var i = 0; i < block.segments.length; i++) {
      var segment = block.segments[i];
      if (compare(probe, segment.node, segment.off) <= 0) {
        return block.start + segment.out;
      }
      if (compare(probe, segment.node, segment.off + segment.width) < 0) {
        var into = segment.node === container && container.nodeType === 3 ? off - segment.off : 0;
        return block.start + segment.out + into;
      }
    }
    return result;
  }

  // ---- What Lectern hears ----

  /** The scroll state: how far down the page is, and which block is at its top. */
  function scrollState() {
    var scroller = document.scrollingElement || document.documentElement;
    var top = scroller.scrollTop;
    var max = scroller.scrollHeight - scroller.clientHeight;
    var fraction = max > 0 ? Math.min(1, Math.max(0, top / max)) : 0;
    return { fraction: fraction, top: top <= 0 ? 0 : topBlock() };
  }

  /** The first block whose text ends below the top of the view. */
  function topBlock() {
    var blocks = model.blocks;
    var lo = 0;
    var hi = blocks.length;
    while (lo < hi) {
      var mid = (lo + hi) >> 1;
      var range = rangeOf(blocks[mid].start, blocks[mid].start + blocks[mid].text.length);
      var bottom = range && range.getBoundingClientRect ? range.getBoundingClientRect().bottom : 0;
      if (bottom <= 0) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    return Math.min(lo, Math.max(0, blocks.length - 1));
  }

  var darkQuery =
    typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-color-scheme: dark)")
      : null;

  function dark() {
    return darkQuery !== null && darkQuery.matches === true;
  }

  /** What `blocks` last reported, to say nothing when a change leaves the text as it was. */
  var reported = "";

  /** The text, where its blocks break, and the headings: a change in any is reported. */
  function signature(headings) {
    var breaks = model.blocks
      .map(function (block) {
        return block.text.length;
      })
      .join(",");
    return model.text + "\u0000" + breaks + "\u0000" + JSON.stringify(headings);
  }

  function ready() {
    read();
    var headings = headingList();
    reported = signature(headings);
    post({
      type: "ready",
      title: document.title.slice(0, MAX_TITLE),
      headings: headings,
      blocks: blockList(),
      scroll: scrollState(),
      dark: dark(),
    });
    watch();
  }

  var changeTimer = null;
  /** When the first change not yet read happened; null when every change has been read. */
  var firstChange = null;

  /**
   * Reads the page again once it has stayed unchanged for a moment, or, if it keeps changing, once
   * the first change waiting has waited long enough.
   */
  function watch() {
    if (typeof window.MutationObserver !== "function") {
      return;
    }
    var observer = new window.MutationObserver(function () {
      var now = Date.now();
      if (firstChange === null) {
        firstChange = now;
      }
      window.clearTimeout(changeTimer);
      var wait = Math.max(0, Math.min(CHANGE_MS, firstChange + CHANGE_MAX_MS - now));
      changeTimer = window.setTimeout(changed, wait);
    });
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      attributeFilter: ["hidden", "class", "style", "open"],
    });
  }

  /**
   * Reads the page again: changed text goes to Lectern, which paints it again itself; the same
   * text, perhaps on new nodes, is painted again here.
   */
  function changed() {
    firstChange = null;
    read();
    var headings = headingList();
    var now = signature(headings);
    if (now === reported) {
      repaint();
      return;
    }
    reported = now;
    post({ type: "blocks", headings: headings, blocks: blockList() });
  }

  var selectionTimer = null;
  var hadSelection = false;

  function selectionChanged() {
    window.clearTimeout(selectionTimer);
    selectionTimer = window.setTimeout(function () {
      var selection = window.getSelection();
      var range = selection && selection.rangeCount > 0 ? selection.getRangeAt(0) : null;
      var start =
        range && !range.collapsed ? offsetOf(range.startContainer, range.startOffset, false) : -1;
      var end =
        range && !range.collapsed ? offsetOf(range.endContainer, range.endOffset, true) : -1;
      if (start < 0 || end <= start) {
        if (hadSelection) {
          hadSelection = false;
          post({ type: "selection", selection: null });
        }
        return;
      }
      hadSelection = true;
      var box = lastBox(range);
      post({
        type: "selection",
        selection: {
          quote: model.text.slice(start, end),
          start: start,
          end: end,
          rect: {
            x: box ? box.x || box.left || 0 : 0,
            y: box ? box.y || box.top || 0 : 0,
            width: box ? box.width || 0 : 0,
            height: box ? box.height || 0 : 0,
          },
        },
      });
    }, SELECTION_MS);
  }

  var scrollQueued = false;

  function scrolled() {
    if (scrollQueued) {
      return;
    }
    scrollQueued = true;
    window.requestAnimationFrame(function () {
      scrollQueued = false;
      post({ type: "scroll", scroll: scrollState() });
    });
  }

  /** Where `range`'s last line is in the viewport: its last box with a size, else its bounds. */
  function lastBox(range) {
    var rects = typeof range.getClientRects === "function" ? range.getClientRects() : null;
    for (var i = rects ? rects.length - 1 : -1; i >= 0; i--) {
      if (rects[i].width > 0 || rects[i].height > 0) {
        return rects[i];
      }
    }
    return typeof range.getBoundingClientRect === "function" ? range.getBoundingClientRect() : null;
  }

  /** The page point at `x`, `y` in the viewport, as `[node, offset]`; null when there's none. */
  function caretAt(x, y) {
    if (typeof document.caretPositionFromPoint === "function") {
      var position = document.caretPositionFromPoint(x, y);
      return position ? [position.offsetNode, position.offset] : null;
    }
    if (typeof document.caretRangeFromPoint === "function") {
      var range = document.caretRangeFromPoint(x, y);
      return range ? [range.startContainer, range.startOffset] : null;
    }
    return null;
  }

  /**
   * A plain click (no link, no selection made by it) on text a comment marks: where it fell goes
   * to Lectern, which shows that comment.
   */
  function clickedMark(e) {
    var marks = painted.comment;
    var selection = window.getSelection();
    if (!marks || marks.length === 0 || (selection && !selection.isCollapsed)) {
      return;
    }
    var caret = caretAt(e.clientX, e.clientY);
    var offset = caret ? offsetOf(caret[0], caret[1], true) : -1;
    for (var i = 0; offset >= 0 && i < marks.length; i++) {
      if (marks[i].start <= offset && offset <= marks[i].end) {
        post({ type: "comment-click", offset: offset });
        return;
      }
    }
  }

  /**
   * A click on a link: Lectern follows it instead, unless it is the page's own to handle. A click
   * elsewhere may be on text a comment marks.
   */
  function clicked(e) {
    if (e.defaultPrevented || e.button !== 0) {
      return;
    }
    var target = e.target;
    var link =
      target && typeof target.closest === "function" ? target.closest("a[href], area[href]") : null;
    if (!link) {
      clickedMark(e);
      return;
    }
    var href = link.getAttribute("href") || "";
    var trimmed = href.trim();
    // In-page anchors scroll the page; `javascript:` links are the page's own code.
    if (trimmed === "" || trimmed.charAt(0) === "#" || /^javascript:/i.test(trimmed)) {
      return;
    }
    e.preventDefault();
    post({ type: "link", href: href });
  }

  // ---- What Lectern asks ----

  function isInteger(n) {
    return typeof n === "number" && Math.floor(n) === n && isFinite(n);
  }

  /** A CSS colour as Lectern sends one, or null for anything else. */
  function colour(value) {
    return typeof value === "string" && /^[#(),.%/\w\s-]{1,80}$/.test(value) ? value : null;
  }

  var themeStyle = null;

  function theme(colours) {
    if (!colours || typeof colours !== "object") {
      return;
    }
    var rules = [];
    for (var layer in LAYERS) {
      var c = colours[layer];
      if (!c || typeof c !== "object") {
        continue;
      }
      var background = colour(c.background);
      var color = colour(c.color);
      var body =
        (background ? " background-color: " + background + ";" : "") +
        (color ? " color: " + color + ";" : "");
      if (body !== "") {
        rules.push("::highlight(lx-" + layer + ") {" + body + " }");
      }
    }
    if (themeStyle === null || !themeStyle.isConnected) {
      themeStyle = document.createElement("style");
      themeStyle.setAttribute("data-lx", "");
      (document.head || document.documentElement).appendChild(themeStyle);
    }
    themeStyle.textContent = rules.join("\n");
  }

  /** What each layer was last asked to paint, to paint it again on rebuilt nodes. */
  var painted = {};

  function repaint() {
    for (var layer in painted) {
      paint(layer, painted[layer]);
    }
  }

  function paint(layer, list) {
    if (!(layer in LAYERS) || !Array.isArray(list) || list.length > MAX_RANGES) {
      return;
    }
    var ranges = [];
    for (var i = 0; i < list.length; i++) {
      var r = list[i];
      if (
        !r ||
        !isInteger(r.start) ||
        !isInteger(r.end) ||
        r.start < 0 ||
        r.end <= r.start ||
        r.end > model.text.length
      ) {
        return;
      }
      var range = rangeOf(r.start, r.end);
      if (range) {
        ranges.push(range);
      }
    }
    var highlights = window.CSS && window.CSS.highlights;
    if (!highlights || typeof window.Highlight !== "function") {
      return;
    }
    painted[layer] = list;
    var highlight = new window.Highlight(...ranges);
    highlight.priority = LAYERS[layer];
    highlights.set("lx-" + layer, highlight);
  }

  function clear(layer) {
    var highlights = window.CSS && window.CSS.highlights;
    if (layer in LAYERS) {
      delete painted[layer];
      if (highlights) {
        highlights.delete("lx-" + layer);
      }
    }
  }

  /** The fraction a restore asked for, held until the reader scrolls or `SETTLE_MS` pass. */
  var settling = null;

  /** Scrolls to `fraction` of the page's height, as it now is. */
  function toFraction(fraction) {
    var scroller = document.scrollingElement || document.documentElement;
    scroller.scrollTop = fraction * Math.max(0, scroller.scrollHeight - scroller.clientHeight);
  }

  /** The page grew or changed size: a place being held is put back. */
  function resettle() {
    if (settling !== null && Date.now() < settling.until) {
      toFraction(settling.fraction);
    } else {
      settling = null;
    }
  }

  /** The reader scrolled, or reached to: the page is theirs to place. */
  function unsettle() {
    settling = null;
  }

  function scrollTo(message) {
    var scroller = document.scrollingElement || document.documentElement;
    if (typeof message.fraction === "number" && isFinite(message.fraction)) {
      var fraction = Math.min(1, Math.max(0, message.fraction));
      toFraction(fraction);
      settling = { fraction: fraction, until: Date.now() + SETTLE_MS };
      return;
    }
    settling = null;
    if (!isInteger(message.offset) || message.offset < 0 || message.offset > model.text.length) {
      return;
    }
    var offset = Math.min(message.offset, Math.max(0, model.text.length - 1));
    var range = rangeOf(offset, offset + 1);
    if (!range) {
      return;
    }
    openAround(range.startContainer);
    if (typeof range.getBoundingClientRect !== "function") {
      return;
    }
    var box = range.getBoundingClientRect();
    var view = window.innerHeight;
    if (message.reveal === true) {
      if (box.top >= 0 && box.bottom <= view) {
        return;
      }
      scroller.scrollTop = scroller.scrollTop + box.top - (view - box.height) / 2;
      return;
    }
    scroller.scrollTop = scroller.scrollTop + box.top - TOP_GAP;
  }

  /** Opens the closed `<details>` that hide `node`: those it is in, outside their summary. */
  function openAround(node) {
    var el = node.nodeType === 1 ? node : node.parentElement;
    var details = el ? el.closest("details") : null;
    while (details) {
      var summary = details.querySelector(":scope > summary");
      if (!details.open && !(summary && summary.contains(el))) {
        details.open = true;
      }
      details = details.parentElement ? details.parentElement.closest("details") : null;
    }
  }

  function received(e) {
    if (e.source !== parent) {
      return;
    }
    var message = e.data;
    if (!message || typeof message !== "object" || message.lx !== 1) {
      return;
    }
    try {
      switch (message.type) {
        case "theme":
          theme(message.theme);
          break;
        case "paint":
          paint(message.layer, message.ranges);
          break;
        case "clear":
          clear(message.layer);
          break;
        case "scroll-to":
          scrollTo(message);
          break;
      }
    } catch (err) {
      // A message the page can't take is dropped.
    }
  }

  window.addEventListener("message", received);
  window.addEventListener("click", clicked);
  window.addEventListener("scroll", scrolled, { passive: true });
  // What grows the page (an image, a font, the page's own load) puts a held place back, until the
  // reader takes over.
  // An image's or a font's `load` reaches the document, not the window.
  document.addEventListener("load", resettle, true);
  window.addEventListener("load", resettle);
  if (typeof window.ResizeObserver === "function") {
    new window.ResizeObserver(resettle).observe(document.documentElement);
  }
  ["wheel", "touchstart", "pointerdown", "keydown"].forEach(function (type) {
    window.addEventListener(type, unsettle, { capture: true, passive: true });
  });
  document.addEventListener("selectionchange", selectionChanged);
  if (darkQuery !== null && typeof darkQuery.addEventListener === "function") {
    darkQuery.addEventListener("change", function () {
      post({ type: "scheme", dark: dark() });
    });
  }
  function start() {
    window.requestAnimationFrame(ready);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }
})(window);
