// Serves dist-fake/ on port 4517 for Playwright, plus the fixture vault's files at
// /__asset/<encoded path>, where the path is under C:\Fixtures\vault as `export_fixtures` wrote it.
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
  ".js": "text/javascript",
  ".css": "text/css",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

/** The file a request path names, or null when it may not be served. */
function resolve(urlPath) {
  if (urlPath.startsWith("/__asset/")) {
    const fake = decodeURIComponent(urlPath.slice("/__asset/".length));
    if (!fake.toLowerCase().startsWith(FAKE_ROOT)) return null;
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
  const file = resolve(new URL(req.url ?? "/", "http://localhost").pathname);
  if (file === null) return void res.writeHead(403).end();
  if (!statSync(file, { throwIfNoEntry: false })?.isFile()) return void res.writeHead(404).end();
  res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(res);
}).listen(4517, () => console.log("fake UI on http://localhost:4517"));
