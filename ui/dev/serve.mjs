// Serves dist-fake/ on port 4517 for Playwright, plus the fixture vault's files at
// /__asset/<encoded path>, where the path is under C:\Fixtures\vault as `export_fixtures` wrote it,
// and at /__page/d/<drive>/<encoded segments>, as `lxpage` serves pages and PDFs into a frame: with
// its sandboxing CSP on everything but a PDF. An HTML page gets Lectern's bridge
// (crates/lectern-core/assets/page-bridge.js) inserted at its top under a fresh nonce, and the
// policy of the HTML pages setting its URL names (`lxmode`, from the fake backend), as
// lectern_core::page builds them (dev/page-serving.mjs, held to Rust's by
// test/page-serving.test.ts). The browser's reports go to /__csp-report, which takes them and does
// nothing more: the fake backend tells what a page loads itself.
import { randomBytes } from "node:crypto";
import { createReadStream, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { injectScript, pagePolicy } from "./page-serving.mjs";

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
  ".css": "text/css",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".pdf": "application/pdf",
};
/** As `lxpage`: the policy of everything but a page or a PDF. */
const SANDBOXED = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/** The fake Windows path a `/__page/d/<drive>/…` path names, or null. */
function pagePath(urlPath) {
  const [drive, ...rest] = urlPath.slice("/__page/d/".length).split("/");
  try {
    return `${drive}:\\${rest.map(decodeURIComponent).join("\\")}`;
  } catch {
    return null;
  }
}

/** The file a request path names, or null when it may not be served. */
function resolve(urlPath) {
  const page = urlPath.startsWith("/__page/d/");
  if (page || urlPath.startsWith("/__asset/")) {
    const fake = page ? pagePath(urlPath) : decodeURIComponent(urlPath.slice("/__asset/".length));
    if (fake === null || !fake.toLowerCase().startsWith(FAKE_ROOT)) return null;
    const file = path.resolve(vault, ...fake.slice(FAKE_ROOT.length).split("\\"));
    return file.startsWith(vault + path.sep) ? file : null;
  }
  const file = path.resolve(
    dist,
    "." + decodeURIComponent(urlPath === "/" ? "/index.html" : urlPath),
  );
  return file.startsWith(dist + path.sep) ? file : null;
}

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (url.pathname === "/__csp-report") {
    req.resume();
    return void res.writeHead(req.method === "POST" ? 204 : 405).end();
  }
  const file = resolve(url.pathname);
  if (file === null) return void res.writeHead(403).end();
  if (!statSync(file, { throwIfNoEntry: false })?.isFile()) return void res.writeHead(404).end();
  const type = TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream";
  const headers = { "content-type": type };
  const framed = url.pathname.startsWith("/__page/");
  if (framed && type === "text/html") {
    const nonce = randomBytes(16).toString("hex");
    const policy = pagePolicy(url.searchParams.get("lxmode"), nonce, url.searchParams.get("t"), "");
    headers["content-security-policy"] = policy.enforced;
    if (policy.reportOnly) headers["content-security-policy-report-only"] = policy.reportOnly;
    res.writeHead(200, headers);
    const script = `<script nonce="${nonce}">${bridge}</script>`;
    return void res.end(injectScript(readFileSync(file, "utf8"), script));
  }
  if (framed && type !== "application/pdf") {
    headers["content-security-policy"] = SANDBOXED;
  }
  res.writeHead(200, headers);
  createReadStream(file).pipe(res);
}).listen(4517, () => console.log("fake UI on http://localhost:4517"));
