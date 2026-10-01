import { defineConfig, devices } from "@playwright/test";

// @playwright/test is pinned to 1.63.0, whose Chromium (revision 1243) is already in ~/.cache/ms-playwright.
export default defineConfig({
  testDir: "e2e",
  reporter: "list",
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
