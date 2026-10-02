// Bundles the UI into dist/ (or dist-fake/ with --fake): app.js, the modules it loads on first use,
// app.css, index.html and fonts/.
// The fake build inlines dev/fixtures.json, which `npm run fixtures` writes, with each document's
// Markdown from fixtures/vault added for the fake's full-text search.
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
  const data = JSON.parse(await readFile(fixtures, "utf8"));
  const vault = path.join(root, "../fixtures/vault");
  data.sources = {};
  for (const doc of Object.keys(data.docs)) {
    const rel = doc.slice(data.root.length + 1).split("\\");
    data.sources[doc] = await readFile(path.join(vault, ...rel), "utf8");
  }
  define.__LX_FIXTURES__ = JSON.stringify(data);
}

await rm(outdir, { recursive: true, force: true });
await mkdir(outdir, { recursive: true });

// app.js loads before the first paint, so what is only needed later stays out of it: app.ts loads
// these modules with `import("./<name>.js")` on first use, and each is bundled on its own (a shared
// chunk would cost app.js a second request before it can run).
const lazy = ["quick-open", "preferences", "menu", "search-panel", "find", "update", "about"];
await esbuild.build({
  entryPoints: [entry],
  outfile: path.join(outdir, "app.js"),
  bundle: true,
  format: "esm",
  minify: true,
  target,
  define,
  external: lazy.map((name) => `./${name}.js`),
  logLevel: "warning",
});
await esbuild.build({
  entryPoints: Object.fromEntries(lazy.map((name) => [name, path.join(root, `src/${name}.ts`)])),
  outdir,
  bundle: true,
  format: "esm",
  minify: true,
  target,
  logLevel: "warning",
});

// One CSS bundle from every stylesheet, in file-name order, through a generated @import entry.
// Font urls stay as written: they point at fonts/, copied next to app.css below.
const styles = (await readdir(path.join(root, "styles"))).filter((f) => f.endsWith(".css")).sort();
await esbuild.build({
  stdin: {
    contents: styles.map((f) => `@import "./styles/${f}";`).join("\n"),
    resolveDir: root,
    sourcefile: "app.css",
    loader: "css",
  },
  outfile: path.join(outdir, "app.css"),
  external: ["*.woff2"],
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
