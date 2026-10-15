// Serves dist-fake/ on port 4517 for Playwright, plus the fixture vault's files at
// /__asset/<encoded path>, where the path is under C:\Fixtures\vault as `export_fixtures` wrote it,
// and at /__page/o/<token>/<encoded segments>, as `lxpage` serves pages and PDFs into a frame: each
// open's token reaches the folder of the page it opened and below, and nothing else; a script or a
// font it serves is shared with the page's opaque origin (`Access-Control-Allow-Origin: null`), by
// its extension alone, and nothing else is; and everything but a PDF carries the sandboxing CSP.
// The server keeps nothing: a token from the fake backend, `t<n>-<hex>`, carries its page's path
// in hex, and the open's key is worked out from it. The open's own page gets
// Lectern's bridge (crates/lectern-core/assets/page-bridge.js), with that key, inserted at its top
// under a fresh nonce, and the policy of the HTML pages setting its URL names (`lxmode`, from the
// fake backend), reporting to its open; any other page gets neither bridge nor reports, as
// lectern_core::page builds them (dev/page-serving.mjs, held to Rust's by
// test/page-serving.test.ts). The browser's reports go to
// /__csp-report, which takes them and does nothing more: the fake backend tells what a page loads
// itself. A file under an open asked for with `lxdelay=<ms>` comes that much later, for the e2e.
import { randomBytes } from "node:crypto";
import { createReadStream, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  bridgeFor,
  fakeBridgeKey,
  injectScript,
  pagePolicy,
  sharedWithPage,
} from "./page-serving.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const dist = path.resolve(here, "../dist-fake");
const vault = path.resolve(here, "../../fixtures/vault");
const bridge = readFileSync(
  path.resolve(here, "../../crates/lectern-core/assets/page-bridge.js"),
  "utf8",
);
const FAKE_ROOT = "c:\\fixtures\\vault\\";
const TYPES = {
  ".html": "text/html",
  ".htm": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".json": "application/json",
  ".css": "text/css",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".pdf": "application/pdf",
};
/** As `lxpage`: the policy of everything but a page or a PDF. */
const SANDBOXED = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/** The page a fake token (`t<n>-<hex>`) opened, or null for any other. */
function openedPage(token) {
  const hex = /^t\d+-((?:[0-9a-f]{2})+)$/.exec(token)?.[1];
  return hex === undefined ? null : Buffer.from(hex, "hex").toString("utf8");
}

/**
 * The fake Windows path `rest` names below the folder of `page`, each segment decoded once and
 * refused if it climbs or holds a separator, as core's `page_path`; null when refused.
 */
function pagePath(page, rest) {
  const folder = page.slice(0, page.lastIndexOf("\\"));
  const segments = [];
  for (const segment of rest.split("/")) {
    let decoded;
    try {
      decoded = decodeURIComponent(segment);
    } catch {
      return null;
    }
    if (/^\.{0,2}$|[\\/:]/.test(decoded)) return null;
    segments.push(decoded);
  }
  return `${folder}\\${segments.join("\\")}`;
}

/** The vault file a fake Windows path names, or null outside the vault. */
function inVault(fake) {
  if (!fake.toLowerCase().startsWith(FAKE_ROOT)) return null;
  const file = path.resolve(vault, ...fake.slice(FAKE_ROOT.length).split("\\"));
  return file.startsWith(vault + path.sep) ? file : null;
}

/**
 * What a request path names: the file, null when it may not be served, and, under an open, the
 * page it opened (`opened`) and whether this is that page. Undefined for a page URL of no open.
 */
function resolve(urlPath) {
  const open = /^\/__page\/o\/([^/]+)\/(.+)$/.exec(urlPath);
  if (open) {
    const page = openedPage(open[1] ?? "");
    if (page === null) return undefined;
    const fake = pagePath(page, open[2] ?? "");
    return {
      file: fake === null ? null : inVault(fake),
      opened: page,
      own: fake !== null && fake.toLowerCase() === page.toLowerCase(),
    };
  }
  if (urlPath.startsWith("/__page/")) return undefined;
  if (urlPath.startsWith("/__asset/")) {
    return { file: inVault(decodeURIComponent(urlPath.slice("/__asset/".length))), opened: null };
  }
  const file = path.resolve(
    dist,
    "." + decodeURIComponent(urlPath === "/" ? "/index.html" : urlPath),
  );
  return { file: file.startsWith(dist + path.sep) ? file : null, opened: null };
}

/** How long the server may hold back a file an open serves (`lxdelay`, for the e2e), in ms. */
const MAX_DELAY_MS = 10_000;

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  // A file under an open the e2e asks to come late, as a slow one does.
  const delay = Math.min(Number(url.searchParams.get("lxdelay")) || 0, MAX_DELAY_MS);
  if (delay > 0 && url.pathname.startsWith("/__page/o/")) {
    await new Promise((resolve) => setTimeout(resolve, delay));
  }
  if (url.pathname === "/__csp-report") {
    req.resume();
    return void res.writeHead(req.method === "POST" ? 204 : 405).end();
  }
  const found = resolve(url.pathname);
  if (found === undefined) return void res.writeHead(404).end();
  // Within an open, a script or a font is the page's to load, as its extension says (WebView2 says
  // nothing of what a request is for); nothing else is its to read.
  const { file } = found;
  if (file === null) return void res.writeHead(403).end();
  const shared =
    found.opened !== null && sharedWithPage(file) ? { "access-control-allow-origin": "null" } : {};
  if (!statSync(file, { throwIfNoEntry: false })?.isFile()) {
    return void res.writeHead(404, shared).end();
  }
  const type = TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream";
  const headers = { "content-type": type, ...shared };
  const framed = found.opened !== null;
  if (framed && type === "text/html") {
    const nonce = randomBytes(16).toString("hex");
    // Its bridge, with the key, and its reports are the open's own page's alone.
    const token = found.own ? (url.pathname.split("/")[3] ?? null) : null;
    const policy = pagePolicy(url.searchParams.get("lxmode"), nonce, token, "");
    headers["content-security-policy"] = policy.enforced;
    if (policy.reportOnly) headers["content-security-policy-report-only"] = policy.reportOnly;
    res.writeHead(200, headers);
    const html = readFileSync(file, "utf8");
    if (token === null) return void res.end(html);
    const script = `<script nonce="${nonce}">${bridgeFor(bridge, fakeBridgeKey(token))}</script>`;
    return void res.end(injectScript(html, script));
  }
  if (framed && type !== "application/pdf") {
    headers["content-security-policy"] = SANDBOXED;
  }
  res.writeHead(200, headers);
  createReadStream(file).pipe(res);
}).listen(4517, () => console.log("fake UI on http://localhost:4517"));
