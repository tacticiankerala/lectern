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
