import { defineConfig } from "vitest/config";

export default defineConfig({
  // themes.test.ts reads src-tauri/src/app.rs, outside ui/.
  server: { fs: { allow: [".."] } },
  test: {
    environment: "jsdom",
    include: ["test/**/*.test.ts"],
    // Without this, Vitest hands tests an empty string for a stylesheet, even with `?raw`.
    css: { include: [/styles\/(themes|fonts)\.css/] },
  },
});
