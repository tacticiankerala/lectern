// Serves dist-fake/ on port 4517 for Playwright, plus the fixture vault's files at
// /__asset/<encoded path>, where the path is under C:\Fixtures\vault as `export_fixtures` wrote it,
// and at /__page/d/<drive>/<encoded segments>, as `lxpage` serves pages and PDFs into a frame: with
// its sandboxing CSP on everything but a PDF.
import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const dist = path.resolve(here, "../dist-fake");
const vault = path.resolve(here, "../../fixtures/vault");
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
/** As `lxpage`: a page's policy, and everything else's but a PDF's. */
const PAGE_POLICY = "default-src 'none'; sandbox";
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
  const urlPath = new URL(req.url ?? "/", "http://localhost").pathname;
  const file = resolve(urlPath);
  if (file === null) return void res.writeHead(403).end();
  if (!statSync(file, { throwIfNoEntry: false })?.isFile()) return void res.writeHead(404).end();
  const type = TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream";
  const headers = { "content-type": type };
  if (urlPath.startsWith("/__page/") && type !== "application/pdf") {
    headers["content-security-policy"] = type === "text/html" ? PAGE_POLICY : SANDBOXED;
  }
  res.writeHead(200, headers);
  createReadStream(file).pipe(res);
}).listen(4517, () => console.log("fake UI on http://localhost:4517"));
