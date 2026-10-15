// How the fake's dev server serves an HTML page (dev/serve.mjs), as `lectern_core::page` does: its
// policies, where Lectern's bridge goes in it, read as the browser's tokenizer reads a page's
// prologue, the charset it goes out with, found as the browser's prescan finds a declared one, and
// which files beside it the page may read. test/page-serving.test.ts holds them to what Rust gives
// (src/generated/page-vectors.ts).

const ALWAYS = "object-src 'none'; form-action 'none'; base-uri 'self'";

/** Where the bridge takes its open's key, as `lectern_core::page::BRIDGE_KEY_SLOT`. */
export const BRIDGE_KEY_SLOT = '"LECTERN_BRIDGE_KEY"';

/**
 * As `lectern_core::page::SHARED_EXTENSIONS`: the files beside a page it may read, its scripts and
 * fonts, by extension, as WebView2 tells `lxpage` nothing of what a request is for.
 */
export const SHARED_EXTENSIONS = ["js", "mjs", "woff", "woff2", "ttf", "otf"];

/** Whether a page may read `file` beside it: its extension, in any letter case, is shared. */
export function sharedWithPage(file) {
  const ext = /\.([^.\\/]+)$/.exec(file)?.[1]?.toLowerCase();
  return ext !== undefined && SHARED_EXTENSIONS.includes(ext);
}

/**
 * The key of the fake open `token`. The fake backend and the dev server keep nothing between
 * them, so both work it out from the token, as no page would (an FNV-1a hash, twice).
 */
export function fakeBridgeKey(token) {
  const hash = (seed) => {
    let h = seed;
    for (const c of token) {
      h = Math.imul(h ^ (c.codePointAt(0) ?? 0), 16777619) >>> 0;
    }
    return h.toString(16).padStart(8, "0");
  };
  return `k${hash(2166136261)}${hash(374761393)}`;
}

/** As `lectern_core::page::bridge_for`: the bridge with the open's key in its slot. */
export function bridgeFor(bridge, key) {
  return bridge.replace(BRIDGE_KEY_SLOT, JSON.stringify(key));
}

/**
 * As `lectern_core::page::page_policy`, its reports going to `reportOrigin`. Without an open to
 * report to, `full` gives the offline policy.
 */
export function pagePolicy(mode, nonce, token, reportOrigin) {
  const report = token ? `report-uri ${reportOrigin}/__csp-report?t=${token}` : null;
  if (mode === "offline" || mode === "static" || report === null) {
    const scripts =
      mode === "static" ? `'nonce-${nonce}'` : "'self' 'unsafe-inline' 'unsafe-eval' blob:";
    const enforced = `sandbox allow-scripts; default-src 'self' data: blob:; script-src ${scripts}; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' data: blob:; frame-src 'none'; ${ALWAYS}`;
    return { enforced: report ? `${enforced}; ${report}` : enforced, reportOnly: null };
  }
  return {
    enforced: `sandbox allow-scripts; default-src 'self' https: data: blob:; script-src 'self' https: 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' https: 'unsafe-inline'; img-src 'self' https: data: blob:; font-src 'self' https: data:; connect-src 'self' https:; media-src 'self' https: data: blob:; frame-src https:; ${ALWAYS}; ${report}`,
    reportOnly: `default-src 'self' data: blob: 'unsafe-inline' 'unsafe-eval'; ${report}`,
  };
}

/** As `lectern_core::page::inject_script`: `html` with `script` where the bridge goes. */
export function injectScript(html, script) {
  const at = insertionPoint(html);
  return `${html.slice(0, at)}${script}${html.slice(at)}`;
}

const SPACE = /[\t\n\f\r ]/;

function isSpace(c) {
  return c !== undefined && SPACE.test(c);
}

/** Right after the `<head>` start tag, else `<html>`, else a leading doctype, else first. */
function insertionPoint(html) {
  const start = html.startsWith("﻿") ? 1 : 0;
  let doctype = null;
  let htmlTag = null;
  let at = start;
  for (;;) {
    while (isSpace(html[at])) at++;
    const rest = html.slice(at);
    if (rest.startsWith("<!--")) {
      at = commentEnd(html, at);
      continue;
    }
    if (rest.startsWith("<!") || rest.startsWith("<?")) {
      const close = html.indexOf(">", at);
      const end = close < 0 ? html.length : close + 1;
      if (rest.slice(2, 9).toLowerCase() === "doctype" && htmlTag === null) doctype = end;
      at = end;
      continue;
    }
    const htmlEnd = htmlTag === null ? startTag(html, at, "html") : null;
    if (htmlEnd !== null) {
      htmlTag = htmlEnd;
      at = htmlEnd;
      continue;
    }
    const headEnd = startTag(html, at, "head");
    if (headEnd !== null) return headEnd;
    break;
  }
  return htmlTag ?? doctype ?? start;
}

/** `<!-->` and `<!--->` are whole comments; any other ends at `-->` or `--!>`, else at the end. */
function commentEnd(html, at) {
  const body = at + 4;
  const rest = html.slice(body);
  if (rest.startsWith(">")) return body + 1;
  if (rest.startsWith("->")) return body + 2;
  const closed = html.indexOf("-->", body);
  const banged = html.indexOf("--!>", body);
  const ends = [closed < 0 ? null : closed + 3, banged < 0 ? null : banged + 4].filter(
    (end) => end !== null,
  );
  return ends.length > 0 ? Math.min(...ends) : html.length;
}

/**
 * The end of the start tag of `name` at `at`, its attributes read as the tokenizer reads them; null
 * when there is no such start tag, or it never ends.
 */
function startTag(html, at, name) {
  if (html[at] !== "<" || html.slice(at + 1, at + 1 + name.length).toLowerCase() !== name) {
    return null;
  }
  const after = html[at + 1 + name.length];
  if (after !== ">" && after !== "/" && !isSpace(after)) return null;
  let state = "beforeName";
  let quote = "";
  for (let i = at + 1 + name.length; i < html.length; i++) {
    const c = html[i];
    const space = isSpace(c);
    if (state === "quoted") {
      if (c === quote) state = "afterQuoted";
      continue;
    }
    if (c === ">") return i + 1;
    switch (state) {
      case "beforeName":
      case "afterQuoted":
        state = c === "/" || space ? "beforeName" : "name";
        break;
      case "unquoted":
        if (space) state = "beforeName";
        break;
      case "name":
      case "afterName":
        if (c === "=") state = "beforeValue";
        else if (c === "/") state = "beforeName";
        else state = space ? "afterName" : "name";
        break;
      case "beforeValue":
        if (c === '"' || c === "'") {
          state = "quoted";
          quote = c;
        } else if (!space) {
          state = "unquoted";
        }
        break;
    }
  }
  return null;
}

/** How far into a page the browser looks for the encoding it declares. */
const CHARSET_PRESCAN = 1024;

/**
 * As `lectern_core::page::prepare_page` gives it, the charset a page's type carries, from its
 * bytes: UTF-8 for a page in UTF-16 (by its byte order mark), which goes out as UTF-8; else the
 * charset it declares; else, with no UTF-8 byte order mark, UTF-8 when it is valid UTF-8; else
 * none.
 */
export function servedCharset(bytes) {
  if ((bytes[0] === 0xff && bytes[1] === 0xfe) || (bytes[0] === 0xfe && bytes[1] === 0xff)) {
    return "utf-8";
  }
  const declared = declaredCharset(bytes);
  if (declared !== null) return declared;
  const bom = bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf;
  return !bom && validUtf8(bytes) ? "utf-8" : null;
}

function validUtf8(bytes) {
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return true;
  } catch {
    return false;
  }
}

/**
 * As `lectern_core::page::declared_charset`: the encoding a `<meta>` in the first 1024 bytes
 * declares, outside comments and other tags, lower-cased; UTF-16 labels mean UTF-8. Each byte is a
 * character here.
 */
function declaredCharset(bytes) {
  const html = String.fromCharCode(...bytes.subarray(0, CHARSET_PRESCAN));
  const cursor = { at: 0 };
  while (cursor.at < html.length) {
    const at = cursor.at;
    const rest = html.slice(at);
    if (rest.startsWith("<!--")) {
      const end = html.indexOf("-->", at + 2);
      cursor.at = end < 0 ? html.length : end + 2;
    } else if (
      rest.slice(0, 5).toLowerCase() === "<meta" &&
      (rest[5] === "/" || isSpace(rest[5]))
    ) {
      cursor.at += 6;
      const charset = metaCharset(html, cursor);
      if (charset !== null) return charset;
      continue;
    } else if (rest[0] === "<" && (isAlpha(rest[1]) || (rest[1] === "/" && isAlpha(rest[2])))) {
      while (cursor.at < html.length && !isSpace(html[cursor.at]) && html[cursor.at] !== ">") {
        cursor.at++;
      }
      while (attribute(html, cursor) !== null);
    } else if (rest.startsWith("<!") || rest.startsWith("</") || rest.startsWith("<?")) {
      const end = html.indexOf(">", at);
      cursor.at = end < 0 ? html.length : end;
    }
    cursor.at++;
  }
  return null;
}

function isAlpha(c) {
  return c !== undefined && /^[A-Za-z]$/.test(c);
}

/** ASCII letters lower-cased, and nothing else. */
function lower(c) {
  return c >= "A" && c <= "Z" ? c.toLowerCase() : c;
}

/** As `meta_charset`: what a `<meta>`'s attributes, read from `cursor`, declare; null for none. */
function metaCharset(html, cursor) {
  const seen = new Set();
  let pragma = false;
  // Whether the charset came from `content=` (and needs the pragma), or from `charset=`.
  let needsPragma = null;
  let charset = null;
  for (let attr = attribute(html, cursor); attr !== null; attr = attribute(html, cursor)) {
    const [name, value] = attr;
    if (seen.has(name)) continue;
    seen.add(name);
    if (name === "http-equiv" && value === "content-type") {
      pragma = true;
    } else if (name === "content" && charset === null) {
      const found = contentCharset(value);
      if (found !== null) {
        charset = found;
        needsPragma = true;
      }
    } else if (name === "charset") {
      charset = value;
      needsPragma = false;
    }
  }
  if (needsPragma === null || (needsPragma && !pragma) || charset === null) return null;
  if (!/^[A-Za-z0-9_.:-]+$/.test(charset)) return null;
  return isUtf16(charset) ? "utf-8" : charset;
}

/** As `is_utf16`: whether `label` names UTF-16 (`utf-16le` or `utf-16be`, by any label). */
function isUtf16(label) {
  return (
    label.startsWith("utf-16") ||
    [
      "unicode",
      "unicodefeff",
      "unicodefffe",
      "ucs-2",
      "csunicode",
      "iso-10646-ucs-2",
      "unicode-1-1",
    ].includes(label)
  );
}

/**
 * As `attribute`: the next attribute of a tag, read from `cursor` as the prescan reads one, its
 * name and value lower-cased; null at the tag's end, or the bytes'.
 */
function attribute(html, cursor) {
  const at = () => html[cursor.at];
  while (at() !== undefined && (isSpace(at()) || at() === "/")) cursor.at++;
  if (at() === undefined || at() === ">") return null;
  let name = "";
  for (;;) {
    const c = at();
    if (c === undefined) return null;
    if (c === "=" && name !== "") {
      cursor.at++;
      break;
    }
    if (isSpace(c)) {
      while (isSpace(at())) cursor.at++;
      if (at() === undefined) return null;
      if (at() !== "=") return [name, ""];
      cursor.at++;
      break;
    }
    if (c === "/" || c === ">") return [name, ""];
    name += lower(c);
    cursor.at++;
  }
  while (isSpace(at())) cursor.at++;
  const first = at();
  if (first === undefined) return null;
  let value = "";
  if (first === '"' || first === "'") {
    for (;;) {
      cursor.at++;
      const c = at();
      if (c === undefined) return null;
      if (c === first) {
        cursor.at++;
        return [name, value];
      }
      value += lower(c);
    }
  }
  if (first === ">") return [name, value];
  for (;;) {
    const c = at();
    if (c === undefined) return null;
    if (isSpace(c) || c === ">") return [name, value];
    value += lower(c);
    cursor.at++;
  }
}

/** As `content_charset`: the charset in a meta's `content` value (`text/html; charset=…`). */
function contentCharset(content) {
  let at = 0;
  for (;;) {
    const found = content.indexOf("charset", at);
    if (found < 0) return null;
    at = found + 7;
    while (isSpace(content[at])) at++;
    if (content[at] === "=") {
      at++;
      break;
    }
  }
  while (isSpace(content[at])) at++;
  const first = content[at];
  if (first === undefined) return null;
  if (first === '"' || first === "'") {
    const end = content.indexOf(first, at + 1);
    return end < 0 ? null : content.slice(at + 1, end);
  }
  let end = at;
  while (end < content.length && !isSpace(content[end]) && content[end] !== ";") end++;
  return end > at ? content.slice(at, end) : null;
}
