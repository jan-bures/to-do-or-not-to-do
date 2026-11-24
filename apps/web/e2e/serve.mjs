// Serves a build of the web app the way todo-server does: files under /app/, every other /app path gets
// index.html with the session marker replaced by a CSRF meta tag, and the HTML carries the server's
// exact Content-Security-Policy (read from crates/todo-server/src/web/spa.rs, so the two can't drift).
//
//   node serve.mjs [dir] [port]      (defaults: ../dist-fixtures, 4173)

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");

export function appCsp() {
  const src = readFileSync(join(root, "crates/todo-server/src/web/spa.rs"), "utf8").replaceAll("\r", "");
  const head = 'pub const APP_CSP: &str = "';
  const start = src.indexOf(head);
  if (start < 0) throw new Error("APP_CSP not found in spa.rs");
  const body = src.slice(start + head.length, src.indexOf('";', start));
  // A Rust string continued with a backslash at the end of a line drops the newline and the indent.
  return body
    .split("\\\n")
    .map((s) => s.trimStart())
    .join("");
}

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".ttf": "font/ttf",
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".txt": "text/plain; charset=utf-8",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
};

export function serve(dir, port) {
  const base = resolve(dir);
  const csp = appCsp();
  const index = readFileSync(join(base, "index.html"), "utf8").replace(
    "<!--todo-session-->",
    `<meta name="csrf-token" content="gallery-csrf"><meta name="todo-owner-scope" content="${"1".repeat(64)}">`,
  );
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    const path = decodeURIComponent(url.pathname);
    if (path === "/" || path === "/app/") {
      res.writeHead(302, { location: "/app" });
      return res.end();
    }
    if (path === "/api/v1/web-push" && req.method === "GET") {
      res.writeHead(204);
      return res.end();
    }
    if (!path.startsWith("/app")) {
      res.writeHead(404);
      return res.end("not found");
    }
    const rest = path.slice("/app".length).replace(/^\/+/, "");
    const file = normalize(join(base, rest));
    if (rest && file.startsWith(base + sep) && existsSync(file) && statSync(file).isFile() && !rest.endsWith(".html")) {
      res.writeHead(200, {
        "content-type": TYPES[extname(file)] ?? "application/octet-stream",
        "cache-control": "no-cache",
        "x-content-type-options": "nosniff",
        ...(rest === "reminder-worker.js" && { "service-worker-allowed": "/app" }),
      });
      return res.end(readFileSync(file));
    }
    if (rest.split("/").pop().includes(".")) {
      res.writeHead(404);
      return res.end("not found");
    }
    res.writeHead(200, {
      "content-type": TYPES[".html"],
      "content-security-policy": csp,
      "cache-control": "no-store",
      "referrer-policy": "same-origin",
      "x-content-type-options": "nosniff",
    });
    res.end(index);
  });
  return new Promise((ok) => server.listen(port, "127.0.0.1", () => ok(server)));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2] ?? join(here, "../dist-fixtures");
  const port = Number(process.argv[3] ?? 4173);
  await serve(dir, port);
  console.log(`serving ${dir} at http://127.0.0.1:${port}/app`);
}
