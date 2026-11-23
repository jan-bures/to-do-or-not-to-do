#!/usr/bin/env node
/**
 * Renders Status, Board & Timeline HTML mockups to 2x PNGs.
 * Uses Playwright from apps/web/e2e/node_modules if installed, or falls back
 * directly to Chromium/Chrome/Edge via headless CDP using Node 24's WebSocket.
 *
 * Usage (from repo root):
 *   node docs/mockups/status-board-timeline/render.mjs
 */
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..", "..");
const outDir = __dirname;

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
  `// Generated from assets/icons/lucide and assets/icons/glyphs by render.mjs.\nwindow.TODO_ICONS = ${JSON.stringify(iconsPayload)};\n`
);

const shots = [
  { file: "desktop-board.html", name: "desktop-board", width: 1440, height: 900, variants: ["light", "dark"] },
  { file: "desktop-timeline.html", name: "desktop-timeline", width: 1440, height: 900, variants: ["light"] },
  { file: "desktop-list-status.html", name: "desktop-list-status", width: 1440, height: 900, variants: ["light"] },
  { file: "desktop-list-settings.html", name: "desktop-list-settings", width: 1440, height: 900, variants: ["light"] },
  { file: "android-cover-board.html", name: "android-cover-board", width: 412, height: 960, variants: ["light"] },
  { file: "android-inner-board.html", name: "android-inner-board", width: 876, height: 970, variants: ["light"] },
  { file: "android-cover-timeline.html", name: "android-cover-timeline", width: 412, height: 960, variants: ["light"] },
];

// 2. Minimal local static server rooted at repoRoot so fonts & tokens.css load cleanly.
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
const baseUrl = `http://127.0.0.1:${port}/docs/mockups/status-board-timeline`;

function findBrowserExecutable() {
  const candidates = [];
  if (process.env.LOCALAPPDATA) {
    const pwRoot = path.join(process.env.LOCALAPPDATA, "ms-playwright");
    if (fs.existsSync(pwRoot)) {
      for (const entry of fs.readdirSync(pwRoot).sort().reverse()) {
        if (entry.startsWith("chromium-")) {
          candidates.push(path.join(pwRoot, entry, "chrome-win64", "chrome.exe"));
          candidates.push(path.join(pwRoot, entry, "chrome-win", "chrome.exe"));
        }
      }
    }
  }
  candidates.push(
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "/usr/bin/chromium",
    "/usr/bin/google-chrome"
  );
  return candidates.find((p) => fs.existsSync(p));
}

async function renderWithPlaywright() {
  const pwPath = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "playwright", "index.mjs");
  const pwFallback = path.join(repoRoot, "apps", "web", "e2e", "node_modules", "@playwright", "test", "index.mjs");
  const modPath = fs.existsSync(pwPath) ? pwPath : fs.existsSync(pwFallback) ? pwFallback : null;
  if (!modPath) return false;
  const pwModule = await import(pathToFileURL(modPath).href);
  const chromium = pwModule.chromium ?? pwModule.default?.chromium;
  if (!chromium) return false;

  const browser = await chromium.launch();
  try {
    for (const shot of shots) {
      const context = await browser.newContext({
        viewport: { width: shot.width, height: shot.height },
        deviceScaleFactor: 2,
      });
      const page = await context.newPage();
      for (const variant of shot.variants) {
        const query = `?capture=1&theme=${variant}`;
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
  }
  return true;
}

async function renderWithCdp() {
  const exe = findBrowserExecutable();
  if (!exe) {
    throw new Error("No Playwright package or Chromium/Chrome/Edge binary found.");
  }
  const userDataDir = path.join(outDir, ".cdp-profile");
  fs.rmSync(userDataDir, { recursive: true, force: true });
  fs.mkdirSync(userDataDir, { recursive: true });

  const proc = spawn(
    exe,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--hide-scrollbars",
      "--remote-debugging-port=0",
      `--user-data-dir=${userDataDir}`,
      "about:blank",
    ],
    { stdio: ["ignore", "pipe", "pipe"] }
  );

  const wsUrl = await new Promise((resolve, reject) => {
    let buf = "";
    const onData = (chunk) => {
      buf += chunk.toString();
      const m = buf.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (m) resolve(m[1]);
    };
    proc.stderr.on("data", onData);
    proc.stdout.on("data", onData);
    proc.on("exit", (code) => reject(new Error(`Browser exited early with code ${code}: ${buf}`)));
  });

  const debugOrigin = new URL(wsUrl).origin.replace(/^ws/, "http");

  async function cdpSession(pageWsUrl) {
    const ws = new WebSocket(pageWsUrl);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", reject, { once: true });
    });
    let nextId = 1;
    const pending = new Map();
    const listeners = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && pending.has(msg.id)) {
        const { resolve, reject } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      } else if (msg.method && listeners.has(msg.method)) {
        for (const fn of listeners.get(msg.method)) fn(msg.params);
      }
    });
    const send = (method, params = {}) =>
      new Promise((resolve, reject) => {
        const id = nextId++;
        pending.set(id, { resolve, reject });
        ws.send(JSON.stringify({ id, method, params }));
      });
    const waitEvent = (method) =>
      new Promise((resolve) => {
        const list = listeners.get(method) || [];
        const fn = (params) => {
          listeners.set(method, (listeners.get(method) || []).filter((x) => x !== fn));
          resolve(params);
        };
        list.push(fn);
        listeners.set(method, list);
      });
    return { send, waitEvent, close: () => ws.close() };
  }

  try {
    const targets = await fetch(`${debugOrigin}/json/list`).then((r) => r.json());
    const pageTarget = targets.find((t) => t.type === "page");
    const cdp = await cdpSession(pageTarget.webSocketDebuggerUrl);
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");

    for (const shot of shots) {
      await cdp.send("Emulation.setDeviceMetricsOverride", {
        width: shot.width,
        height: shot.height,
        deviceScaleFactor: 2,
        mobile: shot.width < 900,
      });
      for (const variant of shot.variants) {
        const loaded = cdp.waitEvent("Page.loadEventFired");
        await cdp.send("Page.navigate", { url: `${baseUrl}/${shot.file}?capture=1&theme=${variant}` });
        await loaded;
        await cdp.send("Runtime.evaluate", {
          expression: "document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))",
          awaitPromise: true,
        });
        const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
        const outPath = path.join(outDir, `${shot.name}-${variant}.png`);
        fs.writeFileSync(outPath, Buffer.from(data, "base64"));
        console.log(`Rendered ${path.relative(repoRoot, outPath)}`);
      }
    }
    cdp.close();
  } finally {
    proc.kill();
    await new Promise((r) => setTimeout(r, 250));
    fs.rmSync(userDataDir, { recursive: true, force: true });
  }
}

try {
  const usedPw = await renderWithPlaywright();
  if (!usedPw) await renderWithCdp();
} finally {
  server.close();
}
