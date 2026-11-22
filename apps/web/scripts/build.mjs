#!/usr/bin/env node
// Builds the web app into a directory that todo-server serves at /app ([web] dir in its config).
//
//   node apps/web/scripts/build.mjs              -> apps/web/dist           (the real app)
//   node apps/web/scripts/build.mjs --fixtures   -> apps/web/dist-fixtures  (the screenshot gallery's build:
//                                                   invented data, a fixed clock, never deployed)
//   --dev                                        an unoptimised build (faster)
//
// Needs the wasm32-unknown-unknown target and wasm-bindgen-cli at the version in apps/web/Cargo.lock:
//   cargo install wasm-bindgen-cli --version <that version> --locked
//
// No bundler and no Trunk: Trunk injects an inline bootstrap script, and the app's Content-Security-Policy
// allows none. The output is plain files: index.html, boot.js, the wasm-bindgen glue, the wasm module,
// app.css, tokens.css, the bundled fonts and the Daybreak app icon (`brand/`, from assets/brand/generated and
// its 16 px source drawing), plus gzipped copies of the larger ones (`<name>.gz`).

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(web, "../..");
const args = new Set(process.argv.slice(2));
const fixtures = args.has("--fixtures");
const dev = args.has("--dev");
const out = join(web, fixtures ? "dist-fixtures" : "dist");
const profile = dev ? "debug" : "release";

function run(cmd, argv, opts = {}) {
  console.log(`> ${cmd} ${argv.join(" ")}`);
  execFileSync(cmd, argv, { stdio: "inherit", cwd: web, ...opts });
}

// The CLI must match the wasm-bindgen crate exactly, or the glue and the module disagree.
const lock = readFileSync(join(web, "Cargo.lock"), "utf8");
const locked = /name = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock)?.[1];
let cli = "";
try {
  cli = execFileSync("wasm-bindgen", ["--version"], { encoding: "utf8" }).trim().split(/\s+/)[1];
} catch {
  console.error(`wasm-bindgen-cli is not installed. Run: cargo install wasm-bindgen-cli --version ${locked} --locked`);
  process.exit(1);
}
if (cli !== locked) {
  console.error(`wasm-bindgen-cli is ${cli}, Cargo.lock has ${locked}. Run: cargo install wasm-bindgen-cli --version ${locked} --locked`);
  process.exit(1);
}

const cargo = ["build", "--target", "wasm32-unknown-unknown", "--locked"];
if (!dev) cargo.push("--release");
if (fixtures) cargo.push("--features", "fixtures");
run("cargo", cargo);

rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "fonts"), { recursive: true });
mkdirSync(join(out, "brand"), { recursive: true });

// Cargo resolves a relative CARGO_TARGET_DIR from its working directory (web).
const target = resolve(web, process.env.CARGO_TARGET_DIR || "target");
const wasm = join(target, "wasm32-unknown-unknown", profile, "todo-web.wasm");
run("wasm-bindgen", ["--target", "web", "--no-typescript", "--out-dir", out, "--out-name", "todo-web", wasm]);

for (const f of readdirSync(join(web, "static"))) {
  copyFileSync(join(web, "static", f), join(out, f));
}
copyFileSync(join(root, "crates", "todo-design", "generated", "tokens.css"), join(out, "tokens.css"));
const fonts = join(root, "assets", "fonts");
for (const f of readdirSync(fonts)) {
  if (f.endsWith(".ttf") || f.startsWith("OFL-")) {
    copyFileSync(join(fonts, f), join(out, "fonts", f));
  }
}
// The app icon. The favicon is drawn at 16 CSS px, so its SVG is the 16 px drawing (never the master scaled
// down: see "The app icon" in docs/design-language.md); the PNGs are the generated 16 and 32 px rasters.
const brand = join(root, "assets", "brand");
for (const [from, to] of [
  [join(brand, "small-16.svg"), "icon-small.svg"],
  [join(brand, "generated", "icon-16.png"), "icon-16.png"],
  [join(brand, "generated", "icon-32.png"), "icon-32.png"],
  [join(brand, "generated", "apple-touch-icon.png"), "apple-touch-icon.png"],
]) {
  copyFileSync(from, join(out, "brand", to));
}
writeFileSync(
  join(out, "fonts", "README.txt"),
  "Bundled fonts (SIL Open Font License 1.1). Licenses: the OFL-*.txt files next to this one.\n",
);

const index = readFileSync(join(out, "index.html"), "utf8");
if (index.split("<!--todo-session-->").length !== 2) {
  console.error("index.html must contain the <!--todo-session--> marker exactly once");
  process.exit(1);
}

// Gzipped copies of the larger files: todo-server sends them to clients that accept gzip.
const compress = (d) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f);
    if (statSync(p).isDirectory()) compress(p);
    else if (/\.(wasm|js|css|ttf)$/.test(f) && statSync(p).size > 1024) {
      writeFileSync(`${p}.gz`, gzipSync(readFileSync(p), { level: 9 }));
    }
  }
};
compress(out);

let total = 0;
const walk = (d) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f);
    const s = statSync(p);
    if (s.isDirectory()) walk(p);
    else total += s.size;
  }
};
walk(out);
const kib = (f) => (statSync(join(out, f)).size / 1024).toFixed(0);
console.log(`built ${out}: ${(total / 1024).toFixed(0)} KiB on disk; wasm ${kib("todo-web_bg.wasm")} KiB, gzipped ${kib("todo-web_bg.wasm.gz")} KiB`);
