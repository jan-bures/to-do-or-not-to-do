// The live browser tests: the real web app against a real todo-server (see live-server.mjs).
import { defineConfig } from "@playwright/test";
import { PORT as port } from "./live-env.mjs";

export default defineConfig({
  testDir: "live",
  outputDir: "test-results",
  workers: 1,
  reporter: [["list"]],
  timeout: 60_000,
  use: {
    baseURL: `http://localhost:${port}`,
    browserName: "chromium",
    viewport: { width: 1440, height: 900 },
  },
  webServer: {
    command: "node live-server.mjs",
    url: `http://localhost:${port}/login`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
