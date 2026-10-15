import { describe, expect, it } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { CONVERT_LIMIT, PATH_TOKEN_VECTORS } from "../src/generated/path-tokens";
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
      const opened = await fake.openUserPath(path);
      expect(opened.folder, path).toBe(false);
      expect(opened.doc?.status, path).toBe("err");
      expect(opened.library.roots, path).toHaveLength(before);
    }
  });
});

describe("FakeBackend path forms", () => {
  it("follows core's rules for drives, /mnt, WSL shares and Linux paths, with a fixed distro", async () => {
    const fake = new FakeBackend(fixtures());
    const forms = (raw: string) => fake.pathForms(raw, false);
    const plan = { windows: "S:\\Notes\\My Vault\\plan.md", wsl: "/mnt/s/Notes/My Vault/plan.md" };
    expect(await forms("S:\\Notes\\My Vault\\plan.md")).toEqual(plan);
    expect(await forms("/mnt/s/Notes/My Vault/plan.md")).toEqual(plan);
    expect(await forms("\\\\wsl.localhost\\Ubuntu\\home\\me\\a.md")).toEqual({
      windows: "\\\\wsl.localhost\\Ubuntu\\home\\me\\a.md",
      wsl: "/home/me/a.md",
    });
    expect(await forms("\\\\wsl$\\Ubuntu\\home\\me\\a.md")).toEqual({
      windows: "\\\\wsl$\\Ubuntu\\home\\me\\a.md",
      wsl: "/home/me/a.md",
    });
    expect(await forms("/home/me/a.md")).toEqual({
      windows: "\\\\wsl.localhost\\Ubuntu\\home\\me\\a.md",
      wsl: "/home/me/a.md",
    });
    expect(await forms("\\\\nas\\share\\x.md")).toEqual({
      windows: "\\\\nas\\share\\x.md",
      wsl: null,
    });
  });

  it("keeps the line, anchor, quotes and backticks around a path, converting the path alone", async () => {
    const fake = new FakeBackend(fixtures());
    for (const [raw, windows, wsl] of [
      ["`S:\\Notes\\plan.md:12`", "`S:\\Notes\\plan.md:12`", "`/mnt/s/Notes/plan.md:12`"],
      ["/mnt/s/Notes/plan.md:12:5", "S:\\Notes\\plan.md:12:5", "/mnt/s/Notes/plan.md:12:5"],
      ["S:\\Notes\\plan.md#L12", "S:\\Notes\\plan.md#L12", "/mnt/s/Notes/plan.md#L12"],
      [' "S:\\Notes\\plan.md" ', ' "S:\\Notes\\plan.md" ', ' "/mnt/s/Notes/plan.md" '],
      ["spec/a_spec.rb:12", "spec\\a_spec.rb:12", "spec/a_spec.rb:12"],
      ['"app/x y.rb"', '"app\\x y.rb"', '"app/x y.rb"'],
    ] as const) {
      expect(await fake.pathForms(raw, false), raw).toEqual({ windows, wsl });
    }
    expect(await fake.pathForms("C:\\src\\C#\\app.cs", false)).toEqual({
      windows: "C:\\src\\C#\\app.cs",
      wsl: "/mnt/c/src/C#/app.cs",
    });
    // A `#` is an anchor only after a file name with an extension.
    const meeting = { windows: "S:\\Notes\\Meeting #3.md", wsl: "/mnt/s/Notes/Meeting #3.md" };
    expect(await fake.pathForms("S:\\Notes\\Meeting #3.md", false)).toEqual(meeting);
    expect(await fake.pathForms("/mnt/s/Notes/Meeting #3.md#agenda", false)).toEqual({
      windows: "S:\\Notes\\Meeting #3.md#agenda",
      wsl: "/mnt/s/Notes/Meeting #3.md#agenda",
    });
  });

  it("flips a relative path's separators, and gives a bare name or a URL neither form", async () => {
    const fake = new FakeBackend(fixtures());
    expect(await fake.pathForms("app/services/x/writing_type.rb", false)).toEqual({
      windows: "app\\services\\x\\writing_type.rb",
      wsl: "app/services/x/writing_type.rb",
    });
    expect(await fake.pathForms("`spec\\a\\b_spec.rb:12`", false)).toEqual({
      windows: "`spec\\a\\b_spec.rb:12`",
      wsl: "`spec/a/b_spec.rb:12`",
    });
    for (const raw of ["notes.md", "https://example.com/a/b", "C:notes\\plan.md", "\\notes\\x"]) {
      expect(await fake.pathForms(raw, false), raw).toEqual({ windows: null, wsl: null });
    }
    // An exact path is never relative.
    expect(await fake.pathForms("notes/plan.md", true)).toEqual({ windows: null, wsl: null });
  });

  it("follows a user mapping from a Linux folder both ways, as core does", async () => {
    const fake = new FakeBackend(fixtures());
    await fake.setSettings({
      pathMappings: [{ from: "/home/me/vault", to: "S:\\Notes\\My Vault" }],
    });
    const plan = { windows: "S:\\Notes\\My Vault\\plan.md", wsl: "/home/me/vault/plan.md" };
    expect(await fake.pathForms("/home/me/vault/plan.md", false)).toEqual(plan);
    expect(await fake.pathForms("S:\\Notes\\My Vault\\plan.md", false)).toEqual(plan);
    expect(await fake.pathForms("s:/notes/my vault/plan.md", true)).toEqual({
      windows: "s:/notes/my vault/plan.md",
      wsl: "/home/me/vault/plan.md",
    });
    // Only whole folders: `My Vaults` isn't under `My Vault`.
    expect((await fake.pathForms("S:\\Notes\\My Vaults\\x.md", false)).wsl).toBe(
      "/mnt/s/Notes/My Vaults/x.md",
    );
    expect((await fake.convertPaths("cat /home/me/vault/a.md", "windows")).text).toBe(
      "cat S:\\Notes\\My Vault\\a.md",
    );
  });

  it("takes a link's path as written, as core's written_forms does", async () => {
    const fake = new FakeBackend(fixtures());
    for (const [written, windows, wsl] of [
      ["../notes/v1.0#archive.md", "..\\notes\\v1.0#archive.md", "../notes/v1.0#archive.md"],
      ["docs/owners'", "docs\\owners'", "docs/owners'"],
      ["plan.md", "plan.md", "plan.md"],
      [
        "/home/me/notes/v1.0#archive.md",
        "\\\\wsl.localhost\\Ubuntu\\home\\me\\notes\\v1.0#archive.md",
        "/home/me/notes/v1.0#archive.md",
      ],
    ] as const) {
      expect(await fake.writtenForms(written), written).toEqual({ windows, wsl });
    }
    for (const written of ["", "\\notes\\plan.md", "C:notes\\plan.md"]) {
      expect(await fake.writtenForms(written), written).toEqual({ windows: null, wsl: null });
    }
  });

  it("converts the paths in a text as core does", async () => {
    const fake = new FakeBackend(fixtures());
    for (const vector of PATH_TOKEN_VECTORS) {
      const label = JSON.stringify(vector.text);
      expect(await fake.convertPaths(vector.text, "windows"), label).toEqual(vector.windows);
      expect(await fake.convertPaths(vector.text, "wsl"), label).toEqual(vector.wsl);
    }
  });

  it("refuses text past core's limit", async () => {
    const fake = new FakeBackend(fixtures());
    const long = "a/b ".repeat(CONVERT_LIMIT / 4) + "c";
    await expect(fake.convertPaths(long, "wsl")).rejects.toBe(
      "The selection is too long to convert.",
    );
    expect((await fake.convertPaths(long.slice(0, CONVERT_LIMIT), "wsl")).converted).toBe(
      CONVERT_LIMIT / 4,
    );
  });

  it("takes an exact path as it is: a `#` or `:12` there is part of a name", async () => {
    const fake = new FakeBackend(fixtures());
    for (const [path, wsl] of [
      ["S:\\Notes\\plan.md#draft.md", "/mnt/s/Notes/plan.md#draft.md"],
      ["S:\\Notes\\v1.0#archive", "/mnt/s/Notes/v1.0#archive"],
    ] as const) {
      expect(await fake.pathForms(path, true), path).toEqual({ windows: path, wsl });
    }
    expect(await fake.pathForms("/home/me/logs/run:12", true)).toEqual({
      windows: "\\\\wsl.localhost\\Ubuntu\\home\\me\\logs\\run:12",
      wsl: "/home/me/logs/run:12",
    });
    // As text, the same `#` is an anchor, kept after the path.
    expect((await fake.pathForms("S:\\Notes\\plan.md#draft.md", false)).windows).toBe(
      "S:\\Notes\\plan.md#draft.md",
    );
  });
});
