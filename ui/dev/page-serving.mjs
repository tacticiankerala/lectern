// How the fake's dev server serves an HTML page (dev/serve.mjs), as `lectern_core::page` does: its
// policies, and where Lectern's bridge goes in it, read as the browser's tokenizer reads a page's
// prologue. test/page-serving.test.ts holds both to what Rust gives (src/generated/page-vectors.ts).

const ALWAYS = "object-src 'none'; form-action 'none'; base-uri 'self'";

/** Where the bridge takes its open's key, as `lectern_core::page::BRIDGE_KEY_SLOT`. */
export const BRIDGE_KEY_SLOT = '"LECTERN_BRIDGE_KEY"';

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
