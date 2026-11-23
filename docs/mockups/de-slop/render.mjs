#!/usr/bin/env node
/**
 * Renders the de-slop design-audit before/after mockups to 2x PNGs using
 * Playwright installed in apps/web/e2e/node_modules, real tokens from
 * crates/todo-design/generated/tokens.css, and bundled fonts/icons.
 *
 * Usage (from repo root):
 *   node docs/mockups/de-slop/render.mjs
 */
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..", "..");
const outDir = __dirname;

function readSvgDir(dir) {
  const out = {};
  for (const name of fs.readdirSync(dir).sort()) {
    if (!name.endsWith(".svg")) continue;
    const raw = fs
      .readFileSync(path.join(dir, name), "utf8")
      .replace(/<!--[\s\S]*?-->/g, "")
      .replace(/\s+/g, " ")
      .trim();
    out[name.slice(0, -4)] = raw;
  }
  return out;
}

const iconsPayload = {
  lucide: readSvgDir(path.join(repoRoot, "assets", "icons", "lucide")),
  glyphs: readSvgDir(path.join(repoRoot, "assets", "icons", "glyphs")),
};
fs.writeFileSync(
  path.join(outDir, "icons.js"),
  `// Generated from assets/icons/lucide and assets/icons/glyphs by docs/mockups/de-slop/render.mjs.\nwindow.TODO_ICONS = ${JSON.stringify(iconsPayload)};\n`
);

const pwPath = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "playwright", "index.mjs");
const pwFallback = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "@playwright", "test", "index.mjs");
const pwModule = await import(pathToFileURL(fs.existsSync(pwPath) ? pwPath : pwFallback).href);
const chromium = pwModule.chromium ?? pwModule.default?.chromium;
if (!chromium) {
  throw new Error("Could not load chromium from apps/web/e2e/node_modules.");
}

const mimeTypes = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".ttf": "font/ttf",
  ".png": "image/png",
};

const server = http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || "/").split("?")[0]);
  const fullPath = path.resolve(repoRoot, "." + urlPath);
  if (!fullPath.startsWith(repoRoot) || !fs.existsSync(fullPath) || fs.statSync(fullPath).isDirectory()) {
    res.writeHead(404);
    res.end("Not found");
    return;
  }
  const ext = path.extname(fullPath).toLowerCase();
  res.writeHead(200, { "Content-Type": mimeTypes[ext] || "application/octet-stream" });
  fs.createReadStream(fullPath).pipe(res);
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const { port } = server.address();
const baseUrl = `http://127.0.0.1:${port}/docs/mockups/de-slop`;

const shots = [
  { file: "desktop-task-row.html", name: "desktop-task-row", width: 960 },
  { file: "assistant-settings-footer.html", name: "assistant-settings-footer", width: 1180 },
  { file: "sidebar-footer.html", name: "sidebar-footer", width: 920 },
  { file: "android-task-row.html", name: "android-task-row", width: 940 },
  { file: "board-card.html", name: "board-card", width: 1140 },
];

const browser = await chromium.launch();
try {
  for (const shot of shots) {
    const context = await browser.newContext({
      viewport: { width: shot.width, height: 800 },
      deviceScaleFactor: 2,
    });
    const page = await context.newPage();
    await page.goto(`${baseUrl}/${shot.file}`, { waitUntil: "networkidle" });
    await page.evaluate(() => document.fonts.ready);
    const height = await page.evaluate(() => {
      const el = document.querySelector(".sheet");
      return el ? Math.ceil(el.getBoundingClientRect().height) : document.documentElement.scrollHeight;
    });
    await page.setViewportSize({ width: shot.width, height });
    const outPath = path.join(outDir, `${shot.name}.png`);
    await page.screenshot({ path: outPath, fullPage: false });
    console.log(`Rendered ${path.relative(repoRoot, outPath)} (${shot.width}x${height} @2x)`);
    await context.close();
  }
} finally {
  await browser.close();
  server.close();
}
