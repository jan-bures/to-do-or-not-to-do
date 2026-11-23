#!/usr/bin/env node
/**
 * Renders the Android thumb-zone HTML mockups to 2x PNGs using Playwright
 * installed in apps/web/e2e/node_modules.
 *
 * Usage (from repo root):
 *   cd apps/web/e2e && npm ci && cd ../../..
 *   node docs/mockups/render.mjs
 */
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const outDir = path.join(__dirname, "thumb-zone");

// 1. Regenerate icons.js from authentic project SVGs (assets/icons/lucide & glyphs).
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
  `// Generated from assets/icons/lucide and assets/icons/glyphs by docs/mockups/render.mjs.\nwindow.TODO_ICONS = ${JSON.stringify(iconsPayload)};\n`
);

// 2. Load Playwright from apps/web/e2e/node_modules.
const pwPath = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "playwright", "index.mjs");
const pwFallback = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "@playwright", "test", "index.mjs");
const pwModule = await import(pathToFileURL(fs.existsSync(pwPath) ? pwPath : pwFallback).href);
const chromium = pwModule.chromium ?? pwModule.default?.chromium;
if (!chromium) {
  throw new Error("Could not load chromium from apps/web/e2e/node_modules. Run `npm ci` in apps/web/e2e first.");
}

// 3. Minimal static HTTP server rooted at repoRoot so fonts & tokens.css load cleanly.
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
const baseUrl = `http://127.0.0.1:${port}/docs/mockups/thumb-zone`;

const shots = [
  { file: "cover-room.html", name: "cover-room", width: 412, height: 960, variants: ["light", "dark", "zones-light"] },
  { file: "cover-switcher.html", name: "cover-switcher", width: 412, height: 960, variants: ["light", "dark", "zones-light"] },
  { file: "cover-detail.html", name: "cover-detail", width: 412, height: 960, variants: ["light", "dark", "zones-light"] },
  { file: "cover-quick-add.html", name: "cover-quick-add", width: 412, height: 960, variants: ["light", "dark", "zones-light"] },
  { file: "cover-bulk.html", name: "cover-bulk", width: 412, height: 960, variants: ["light", "dark", "zones-light"] },
  {
    file: "unfolded-workspace.html",
    name: "unfolded-workspace",
    width: 876,
    height: 970,
    variants: ["light", "dark", "zones-light", "zones-dark"],
  },
];

const browser = await chromium.launch();
try {
  for (const shot of shots) {
    const context = await browser.newContext({
      viewport: { width: shot.width, height: shot.height },
      deviceScaleFactor: 2,
    });
    const page = await context.newPage();
    for (const variant of shot.variants) {
      const isDark = variant.endsWith("dark");
      const hasZones = variant.startsWith("zones");
      const query = `?capture=1&theme=${isDark ? "dark" : "light"}&zones=${hasZones ? "1" : "0"}`;
      await page.goto(`${baseUrl}/${shot.file}${query}`, { waitUntil: "networkidle" });
      await page.evaluate(() => document.fonts.ready);
      const outPath = path.join(outDir, `${shot.name}-${variant}.png`);
      await page.screenshot({ path: outPath, fullPage: false });
      console.log(`Rendered ${path.relative(repoRoot, outPath)}`);
    }
    await context.close();
  }
} finally {
  await browser.close();
  server.close();
}
