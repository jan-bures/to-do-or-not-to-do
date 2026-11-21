// Starts a real todo-server for the live browser tests: a fresh data directory, loopback only, and the
// web app from ../dist. The binary is $TODO_SERVER_BIN, or the workspace's debug build.
//
//   cargo build -p todo-server && node ../scripts/build.mjs && npx playwright test -c live.config.mjs

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { DATA, PORT } from "./live-env.mjs";

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

rmSync(DATA, { recursive: true, force: true });
mkdirSync(DATA, { recursive: true });
const child = spawn(bin, ["serve"], {
  stdio: "inherit",
  env: {
    ...process.env,
    TODO_SERVER_BIND: `127.0.0.1:${PORT}`,
    TODO_SERVER_DATA_DIR: DATA,
    TODO_SERVER_PUBLIC_ORIGIN: `http://localhost:${PORT}`,
    TODO_SERVER_WEB_DIR: dist,
    RUST_LOG: process.env.RUST_LOG ?? "warn",
  },
});
const stop = () => child.kill();
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
process.on("exit", stop);
child.on("exit", (code) => process.exit(code ?? 0));
