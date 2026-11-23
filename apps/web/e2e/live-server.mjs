// Starts a real todo-server for the live browser tests: a fresh data directory, loopback only, and the
// web app from ../dist. The binary is $TODO_SERVER_BIN, or the workspace's debug build.
//
//   cargo build -p todo-server && node ../scripts/build.mjs && npx playwright test -c live.config.mjs

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { DATA, GALLERY_DATA, GALLERY_PORT, PORT } from "./live-env.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");

const exe = process.platform === "win32" ? "todo-server.exe" : "todo-server";
const bin = process.env.TODO_SERVER_BIN ?? join(root, "target", "debug", exe);
if (!existsSync(bin)) {
  console.error(`${bin} not found: run \`cargo build -p todo-server\` first (or set TODO_SERVER_BIN)`);
  process.exit(1);
}
const dist = join(here, "..", "dist");
if (!existsSync(join(dist, "index.html"))) {
  console.error(`${dist} has no build: run \`node apps/web/scripts/build.mjs\` first`);
  process.exit(1);
}

const gallery = process.argv.includes("--admin-gallery");
const data = gallery ? GALLERY_DATA : DATA;
const port = gallery ? GALLERY_PORT : PORT;
rmSync(data, { recursive: true, force: true });
mkdirSync(data, { recursive: true });
const args = ["serve"];
if (gallery) {
  // The gallery visits all auth pages and pairs several invented devices in a few seconds.
  const config = join(data, "gallery.toml");
  writeFileSync(config, "[auth]\nauth_request_burst = 200\n");
  args.push("--config", config);
}
const child = spawn(bin, args, {
  stdio: "inherit",
  env: {
    ...process.env,
    TODO_SERVER_BIND: `127.0.0.1:${port}`,
    TODO_SERVER_DATA_DIR: data,
    TODO_SERVER_PUBLIC_ORIGIN: `http://localhost:${port}`,
    TODO_SERVER_WEB_DIR: dist,
    RUST_LOG: process.env.RUST_LOG ?? "warn",
  },
});
const stop = () => child.kill();
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
process.on("exit", stop);
child.on("exit", (code) => process.exit(code ?? 0));
