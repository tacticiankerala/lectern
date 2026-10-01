import { beforeEach, describe, expect, it } from "vitest";
import { renderProperties } from "../src/properties";
import type { Property } from "../src/generated/Property";
import { docPayload } from "./helpers";

// 1 October 2026, local noon.
const NOW = new Date(2026, 9, 1, 12, 0).getTime();

function parsed(entries: Property[]) {
  return docPayload({ kind: "parsed", entries });
}

function render(doc = parsed([]), now = NOW): HTMLElement {
  const host = document.createElement("section");
  document.body.replaceChildren(host);
  renderProperties(host, doc, now);
  return host;
}

describe("renderProperties", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("renders status badge and relative dates", () => {
    const host = render(
      parsed([
        { key: "status", value: { kind: "text", value: "active" } },
        { key: "updated", value: { kind: "date", value: "2026-09-29" } },
        { key: "started", value: { kind: "dateTime", value: "2026-10-01T09:00:00" } },
      ]),
    );
    expect(host.querySelector(".badge.status-active")?.textContent).toBe("active");
    const dates = [...host.querySelectorAll("time")];
    expect(dates.map((d) => d.textContent)).toEqual(["2 days ago", "3 hours ago"]);
    expect(dates[0]?.getAttribute("title")).toBe("2026-09-29");
    expect(dates[1]?.getAttribute("title")).toBe("2026-10-01T09:00:00");
  });

  it("renders prs as chips", () => {
    const host = render(
      parsed([
        {
          key: "prs",
          value: {
            kind: "list",
            value: [
              { kind: "number", value: "6257" },
              { kind: "number", value: "6261" },
            ],
          },
        },
      ]),
    );
    expect([...host.querySelectorAll(".chip")].map((c) => c.textContent)).toEqual([
      "#6257",
      "#6261",
    ]);
  });

  it("shows invalid frontmatter raw", () => {
    const host = render(
      docPayload({ kind: "invalid", raw: "status: [unclosed", error: "did not find ]" }),
    );
    expect(host.querySelector("pre")?.textContent).toBe("status: [unclosed");
    expect(host.querySelector(".props-warning")?.getAttribute("title")).toContain("did not find ]");
  });

  it("shows other keys as key and value, lists joined", () => {
    const host = render(
      parsed([
        { key: "metadata.type", value: { kind: "text", value: "feedback" } },
        { key: "draft", value: { kind: "bool", value: true } },
        {
          key: "aliases",
          value: {
            kind: "list",
            value: [
              { kind: "text", value: "one" },
              { kind: "text", value: "two" },
            ],
          },
        },
      ]),
    );
    const pairs = [...host.querySelectorAll(".prop")].map((p) => [
      p.querySelector(".prop-key")?.textContent,
      p.querySelector(".prop-value")?.textContent,
    ]);
    expect(pairs).toEqual([
      ["metadata.type", "feedback"],
      ["draft", "yes"],
      ["aliases", "one, two"],
    ]);
  });

  it("counts tasks, even without frontmatter", () => {
    const host = render(docPayload(null, { done: 10, total: 96 }));
    expect(host.querySelector(".props-tasks")?.textContent).toBe("10 / 96 tasks");
    expect(host.hidden).toBe(false);
  });

  it("hides itself with neither frontmatter nor tasks", () => {
    expect(render(docPayload(null)).hidden).toBe(true);
  });

  it("collapses from its header and remembers it", () => {
    const doc = parsed([{ key: "owner", value: { kind: "text", value: "sam" } }]);
    const host = render(doc);
    const head = host.querySelector<HTMLButtonElement>(".props-head");
    expect(host.classList.contains("collapsed")).toBe(false);
    head?.click();
    expect(host.classList.contains("collapsed")).toBe(true);
    expect(head?.getAttribute("aria-expanded")).toBe("false");
    expect(render(doc).classList.contains("collapsed")).toBe(true);
  });
});
