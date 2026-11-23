// The server-rendered admin gallery uses the same real server harness as live.config.mjs,
// in its own run so that it can capture the pre-setup pages before registering a passkey.
import { defineConfig } from "@playwright/test";
import live from "./live.config.mjs";
import { GALLERY_PORT } from "./live-env.mjs";

const baseURL = `http://localhost:${GALLERY_PORT}`;

export default defineConfig({
  ...live,
  testDir: "admin",
  timeout: 180_000,
  use: { ...live.use, baseURL },
  webServer: { ...live.webServer, command: "node live-server.mjs --admin-gallery", url: `${baseURL}/login` },
});
