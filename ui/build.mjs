// Bundles the UI into dist/ (or dist-fake/ with --fake): app.js, app.css, index.html and fonts/.
// The fake build inlines dev/fixtures.json, which `npm run fixtures` writes.
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, readdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const root = path.dirname(fileURLToPath(import.meta.url));
const fake = process.argv.includes("--fake");
const outdir = path.join(root, fake ? "dist-fake" : "dist");
const entry = path.join(root, fake ? "dev/main-fake.ts" : "src/main.ts");
const target = "chrome120";

const define = {};
if (fake) {
  const fixtures = path.join(root, "dev/fixtures.json");
  if (!existsSync(fixtures)) {
    throw new Error("dev/fixtures.json is missing: run `npm run fixtures` first");
  }
  define.__LX_FIXTURES__ = await readFile(fixtures, "utf8");
}

await rm(outdir, { recursive: true, force: true });
await mkdir(outdir, { recursive: true });

await esbuild.build({
  entryPoints: [entry],
  outfile: path.join(outdir, "app.js"),
  bundle: true,
  format: "esm",
  minify: true,
  target,
  define,
  logLevel: "warning",
});

// One CSS bundle from every stylesheet, in file-name order, through a generated @import entry.
const styles = (await readdir(path.join(root, "styles"))).filter((f) => f.endsWith(".css")).sort();
await esbuild.build({
  stdin: {
    contents: styles.map((f) => `@import "./styles/${f}";`).join("\n"),
    resolveDir: root,
    sourcefile: "app.css",
    loader: "css",
  },
  outfile: path.join(outdir, "app.css"),
  bundle: true,
  minify: true,
  target,
  logLevel: "warning",
});

await cp(path.join(root, "index.html"), path.join(outdir, "index.html"));
const fonts = path.join(root, "fonts");
if (existsSync(fonts)) {
  await cp(fonts, path.join(outdir, "fonts"), { recursive: true });
}
