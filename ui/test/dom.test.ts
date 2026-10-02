import { describe, expect, it } from "vitest";
import { MARKDOWN_PATH } from "../src/dom";

describe("MARKDOWN_PATH", () => {
  it("knows every extension the core treats as Markdown, in any case", () => {
    for (const path of ["C:\\n\\a.md", "a.MARKDOWN", "C:\\n\\old.mdown", "short.MKD"]) {
      expect(MARKDOWN_PATH.test(path), path).toBe(true);
    }
    for (const path of ["a.md.txt", "a.mdx", "readme", "C:\\n.md\\x.png"]) {
      expect(MARKDOWN_PATH.test(path), path).toBe(false);
    }
  });
});
