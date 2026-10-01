// Shared helpers for the Playwright specs, which run against the fake backend (dev/).
import { expect, type Page } from "@playwright/test";

/** The fake library root that `export_fixtures` writes paths under. */
export const VAULT = "C:\\Fixtures\\vault";

/** The fake Windows path of a fixture, from its `/`-separated path in `fixtures/vault`. */
export function fixturePath(rel: string): string {
  return `${VAULT}\\${rel.replaceAll("/", "\\")}`;
}

/** Loads the app as if launched with `path` (or with nothing), and waits for startup. */
export async function launch(page: Page, path?: string): Promise<void> {
  await page.goto(path === undefined ? "/" : `/?open=${encodeURIComponent(path)}`);
  await page.locator("html[data-lx-ready]").waitFor({ state: "attached" });
}

/** Loads the app with a fixture open. */
export async function openFixture(page: Page, rel: string): Promise<void> {
  await launch(page, fixturePath(rel));
}

/** Where an anchor lands below the pane's top (doc-view's TOP_GAP), and the slack allowed. */
export const LANDING_PX = 12;
export const LANDING_SLACK_PX = 8;

/** How far heading `id` sits below the top of the document pane, or null if it's missing. */
export function headingOffset(page: Page, id: string): Promise<number | null> {
  return page.evaluate((id) => {
    const pane = document.getElementById("lx-doc-pane");
    const heading = document.querySelector(`#lx-doc [id="${id}"]`);
    if (!pane || !heading) return null;
    return Math.round(heading.getBoundingClientRect().top - pane.getBoundingClientRect().top);
  }, id);
}

/** Waits out lazy layout (about a second), then checks heading `id` is still where it landed. */
export async function expectLandedOn(page: Page, id: string): Promise<void> {
  await page.waitForTimeout(1000);
  const offset = await headingOffset(page, id);
  expect(offset, `#${id} below the pane's top`).not.toBeNull();
  expect(Math.abs((offset ?? Infinity) - LANDING_PX)).toBeLessThanOrEqual(LANDING_SLACK_PX);
}
