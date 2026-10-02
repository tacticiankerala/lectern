import { describe, expect, it } from "vitest";
import { fuzzyFilter } from "../src/fuzzy";

declare const process: { env: Record<string, string | undefined> };

interface Item {
  rel: string;
}

const item = (rel: string): Item => ({ rel });
const key = (t: Item) => ({ name: t.rel.slice(t.rel.lastIndexOf("/") + 1), rel: t.rel });
const rels = (query: string, items: Item[], limit?: number) =>
  fuzzyFilter(query, items, key, limit).map((s) => s.item.rel);

const VAULT = [
  "statistics/overview.md",
  "work/slab/notes/2026-01-15-pr1-stats.md",
  "work/slab/README.md",
  "work/slab/plans/2026-09-30-rollout.md",
  "archive/old-stuff/startups.md",
  "reference/color-contrast-overview.md",
].map(item);

describe("fuzzyFilter", () => {
  it("prefers filename hits", () => {
    expect(rels("stats", VAULT)[0]).toBe("work/slab/notes/2026-01-15-pr1-stats.md");
    // A hit in the folder path still matches, below the filename hit.
    expect(rels("stats", VAULT)).toContain("statistics/overview.md");
    expect(rels("readme", VAULT)[0]).toBe("work/slab/README.md");
  });

  it("matches word starts across dated names", () => {
    expect(rels("pr1st", VAULT)[0]).toBe("work/slab/notes/2026-01-15-pr1-stats.md");
    expect(rels("cco", VAULT)[0]).toBe("reference/color-contrast-overview.md");
  });

  it("is case-insensitive and a subsequence match", () => {
    expect(rels("ROLLOUT", VAULT)).toEqual(["work/slab/plans/2026-09-30-rollout.md"]);
    expect(rels("sbplro", VAULT)).toEqual(["work/slab/plans/2026-09-30-rollout.md"]);
    expect(rels("zzz", VAULT)).toEqual([]);
  });

  it("rates a camel hump as a word start", () => {
    const items = ["notes/weeklyMeeting.md", "notes/somewhere-time.md"].map(item);
    expect(rels("wm", items)[0]).toBe("notes/weeklyMeeting.md");
  });

  it("returns positions for highlighting", () => {
    const [top] = fuzzyFilter("pr1st", VAULT, key);
    const rel = "work/slab/notes/2026-01-15-pr1-stats.md";
    expect(top?.item.rel).toBe(rel);
    // Indexes into `rel`, which ends with the name.
    const picked = (top?.positions ?? []).map((i) => rel[i]).join("");
    expect(picked).toBe("pr1st");
    expect(top?.positions).toEqual([27, 28, 29, 31, 32]);
    const [path] = fuzzyFilter("sbro", VAULT, key);
    expect(path?.positions.map((i) => path.item.rel[i]).join("")).toBe("sbro");
  });

  it("keeps the order and gives no positions for an empty query", () => {
    const all = fuzzyFilter("", VAULT, key);
    expect(all.map((s) => s.item)).toEqual(VAULT);
    expect(all.every((s) => s.positions.length === 0)).toBe(true);
    expect(fuzzyFilter("", VAULT, key, 2)).toHaveLength(2);
  });

  it("returns at most `limit` matches, best first", () => {
    const items = Array.from({ length: 300 }, (_, i) => item(`notes/plan-${String(i)}.md`));
    items.push(item("plan.md"));
    const top = fuzzyFilter("plan", items, key, 10);
    expect(top).toHaveLength(10);
    expect(top[0]?.item.rel).toBe("plan.md");
    for (let i = 1; i < top.length; i++) {
      expect(top[i - 1]?.score ?? 0).toBeGreaterThanOrEqual(top[i]?.score ?? 0);
    }
  });

  it("handles 10k items under 5ms", () => {
    const words = ["alpha", "beta", "gamma", "notes", "plans", "reference", "memory", "work"];
    const items: Item[] = [];
    for (let i = 0; i < 10_000; i++) {
      const a = words[i % words.length] ?? "";
      const b = words[(i * 7) % words.length] ?? "";
      const day = String((i % 28) + 1).padStart(2, "0");
      items.push(item(`${a}/project-${String(i)}/${b}/2026-09-${day}-feature-${String(i)}.md`));
    }
    // Warm up the JIT, then take the best of a few runs.
    for (let i = 0; i < 3; i++) fuzzyFilter("plan", items, key);
    let best = Infinity;
    for (let i = 0; i < 5; i++) {
      const started = performance.now();
      const found = fuzzyFilter("plan", items, key);
      best = Math.min(best, performance.now() - started);
      expect(found.length).toBeGreaterThan(0);
    }
    const budget = process.env.CI ? 20 : 5;
    expect(best).toBeLessThan(budget);
  });
});
