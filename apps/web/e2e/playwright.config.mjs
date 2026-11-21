// Browser tests and the screenshot gallery. They run against the fixtures build
// (`node ../scripts/build.mjs --fixtures`), served with the server's CSP by serve.mjs.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  outputDir: "test-results",
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:4173",
    browserName: "chromium",
    locale: "en-GB",
    timezoneId: "Europe/Prague",
  },
  webServer: {
    command: "node serve.mjs ../dist-fixtures 4173",
    url: "http://127.0.0.1:4173/app",
    reuseExistingServer: !process.env.CI,
  },
});
