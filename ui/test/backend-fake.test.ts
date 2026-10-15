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

  it("drops line suffixes, anchors, quotes and backticks; a relative path has neither form", async () => {
    const fake = new FakeBackend(fixtures());
    const plan = { windows: "S:\\Notes\\plan.md", wsl: "/mnt/s/Notes/plan.md" };
    for (const raw of [
      "`S:\\Notes\\plan.md:12`",
      "S:\\Notes\\plan.md:12:5",
      "S:\\Notes\\plan.md#L12",
      "S:\\Notes\\plan.md#next-steps",
      ' "S:\\Notes\\plan.md" ',
    ]) {
      expect(await fake.pathForms(raw, false), raw).toEqual(plan);
    }
    expect(await fake.pathForms("C:\\src\\C#\\app.cs", false)).toEqual({
      windows: "C:\\src\\C#\\app.cs",
      wsl: "/mnt/c/src/C#/app.cs",
    });
    // A `#` is an anchor only after a file name with an extension.
    const meeting = { windows: "S:\\Notes\\Meeting #3.md", wsl: "/mnt/s/Notes/Meeting #3.md" };
    expect(await fake.pathForms("S:\\Notes\\Meeting #3.md", false)).toEqual(meeting);
    expect(await fake.pathForms("S:\\Notes\\Meeting #3.md#agenda", false)).toEqual(meeting);
    expect(await fake.pathForms("notes/plan.md", false)).toEqual({ windows: null, wsl: null });
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
    // As text, the same `#` is an anchor.
    expect((await fake.pathForms("S:\\Notes\\plan.md#draft.md", false)).windows).toBe(
      "S:\\Notes\\plan.md",
    );
  });
});
