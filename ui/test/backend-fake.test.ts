import { describe, expect, it } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { ROOT, fixtures, rendered } from "./helpers";

const README = `${ROOT}\\README.md`;
const INDEX = `${ROOT}\\memory\\index.md`;

describe("FakeBackend search", () => {
  it("finds every file with a hit, after one whose name matched", async () => {
    const data = fixtures({
      [README]: rendered("Home", "<h1>Home</h1>"),
      [INDEX]: rendered("Memory index", "<ul><li>README</li></ul>"),
    });
    data.sources = {
      [README]: "# Home\n\nStart with this readme.",
      [INDEX]: "- [[README]]: the vault home",
    };
    const results = await new FakeBackend(data).search("readme");
    expect(results.map((r) => r.rel)).toEqual(["README.md", "memory/index.md"]);
    expect(results.map((r) => r.nameMatch)).toEqual([true, false]);
    expect(results[1]?.hits.map((h) => h.line)).toEqual([1]);
  });
});

describe("FakeBackend documents", () => {
  it("matches a text file's name without its extension, as core does", async () => {
    const readme = `${ROOT}\\notes\\readme.txt`;
    const data = fixtures({ [readme]: rendered("readme", "<p>Saved as readme.txt.</p>") });
    data.kinds = { [readme]: "text" };
    data.sources = { [readme]: "Saved as readme.txt beside the notes." };
    const results = await new FakeBackend(data).search("txt");
    expect(results.map((r) => [r.rel, r.nameMatch])).toEqual([["notes/readme.txt", false]]);
  });

  it("takes any document's name for a file, never a folder to add", async () => {
    const fake = new FakeBackend(fixtures());
    const before = (await fake.getLibrary()).roots.length;
    for (const path of ["C:\\Elsewhere\\brief.pdf", "C:\\Elsewhere\\page.HTML"]) {
      const opened = await fake.openUserPath(path, 1);
      expect(opened.folder, path).toBe(false);
      expect(opened.doc?.status, path).toBe("err");
      expect(opened.library.roots, path).toHaveLength(before);
    }
  });
});

describe("FakeBackend WSL distributions", () => {
  it("lists Windows' default and the installed ones, and takes `\"\"` for Windows' default", async () => {
    const fake = new FakeBackend(fixtures());
    expect(await fake.listWslDistros()).toEqual({
      default: "Ubuntu",
      installed: ["Debian", "Ubuntu"],
    });
    await fake.setSettings({ wslDistro: "Debian" });
    expect(fake.sharedSettings().wslDistro).toBe("Debian");
    // `""` asks for Windows' default, as core takes it.
    await fake.setSettings({ wslDistro: "" });
    expect(fake.sharedSettings().wslDistro).toBeNull();
  });
});
