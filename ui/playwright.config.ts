import { defineConfig, devices } from "@playwright/test";

// The config runs in Node; the project has no Node types, as everything else runs in the page.
declare const process: { env: Record<string, string | undefined> };

// @playwright/test is pinned to 1.63.0, whose Chromium (revision 1243) is already in ~/.cache/ms-playwright.
// The specs run against the fake backend (dev/), served by `npm run serve:fake`.
export default defineConfig({
  testDir: "e2e",
  reporter: "list",
  use: { baseURL: "http://localhost:4517" },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1400, height: 900 } },
    },
  ],
  webServer: {
    command: "npm run serve:fake",
    port: 4517,
    reuseExistingServer: !process.env.CI,
    // The first run builds the fixture exporter.
    timeout: 180_000,
  },
});
