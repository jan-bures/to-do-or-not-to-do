// Browser tests and the screenshot gallery. They run against the fixtures build
// (`node ../scripts/build.mjs --fixtures`), served with the server's CSP by serve.mjs.
import { defineConfig } from "@playwright/test";

const port = Number(process.env.FIXTURE_PORT ?? 4173);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "tests",
  outputDir: "test-results",
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL,
    browserName: "chromium",
    locale: "en-GB",
    timezoneId: "Europe/Prague",
  },
  webServer: {
    command: `node serve.mjs ../dist-fixtures ${port}`,
    url: `${baseURL}/app`,
    reuseExistingServer: !process.env.CI,
  },
});
