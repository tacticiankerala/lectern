import { afterEach, describe, expect, it, vi } from "vitest";
import { RightPanel } from "../src/right-panel";

function setup() {
  document.body.innerHTML = "<aside id='lx-outline'></aside>";
  const aside = document.getElementById("lx-outline");
  if (!aside) throw new Error("no aside");
  const panel = new RightPanel(aside);
  const tabs = [...aside.querySelectorAll<HTMLButtonElement>("[role=tablist] [role=tab]")];
  const [outlineTab, commentsTab] = tabs;
  if (!outlineTab || !commentsTab) throw new Error("no tabs");
  return { aside, panel, tabs, outlineTab, commentsTab };
}

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("RightPanel", () => {
  it("switches panes with its tabs", () => {
    const { aside, panel, tabs, outlineTab, commentsTab } = setup();
    expect(tabs.map((t) => t.textContent)).toEqual(["Outline", "Comments"]);
    expect(panel.outlinePane.id).toBe("lx-outline-pane");
    expect(panel.commentsPane.id).toBe("lx-comments-pane");
    expect(aside.contains(panel.outlinePane) && aside.contains(panel.commentsPane)).toBe(true);
    expect(panel.tab).toBe("outline");
    expect(outlineTab.getAttribute("aria-selected")).toBe("true");
    expect(panel.commentsPane.hidden).toBe(true);

    commentsTab.click();
    expect(panel.tab).toBe("comments");
    expect(commentsTab.getAttribute("aria-selected")).toBe("true");
    expect(outlineTab.getAttribute("aria-selected")).toBe("false");
    expect(panel.outlinePane.hidden).toBe(true);
    expect(panel.commentsPane.hidden).toBe(false);

    panel.show("outline");
    expect(panel.tab).toBe("outline");
    expect(panel.outlinePane.hidden).toBe(false);
    expect(panel.commentsPane.hidden).toBe(true);
  });

  it("moves between tabs with the arrow keys", () => {
    const { panel, outlineTab, commentsTab } = setup();
    outlineTab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(panel.tab).toBe("comments");
    expect(document.activeElement).toBe(commentsTab);
    commentsTab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(panel.tab).toBe("outline");
  });

  it("forces Outline and hides the strip while comments are off", () => {
    const { aside, panel } = setup();
    const strip = aside.querySelector<HTMLElement>("[role=tablist]");
    panel.show("comments");
    panel.setCommentsEnabled(false);
    expect(strip?.hidden).toBe(true);
    expect(panel.tab).toBe("outline");
    expect(panel.commentsPane.hidden).toBe(true);
    panel.show("comments");
    expect(panel.tab).toBe("outline");
    panel.setCommentsEnabled(true);
    expect(strip?.hidden).toBe(false);
    expect(panel.tab).toBe("outline");
    panel.show("comments");
    expect(panel.tab).toBe("comments");
  });

  it("counts open comments in the tab's label", () => {
    const { panel, commentsTab } = setup();
    panel.setCount(3);
    expect(commentsTab.textContent).toBe("Comments (3)");
    panel.setCount(0);
    expect(commentsTab.textContent).toBe("Comments");
  });

  it("flashes the Comments tab for a moment", () => {
    vi.useFakeTimers();
    const { panel, commentsTab } = setup();
    panel.flash();
    expect(commentsTab.classList.contains("flash")).toBe(true);
    vi.advanceTimersByTime(3000);
    expect(commentsTab.classList.contains("flash")).toBe(false);
  });
});
