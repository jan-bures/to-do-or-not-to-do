// Generates everything in `generated/` from the SVG sources next to this file.
//
//   node generate.mjs           write the outputs
//   node generate.mjs --check   write nothing; exit 1 if an output is missing or stale
//
// Needs `npm ci` first (a single dependency, the pure-Node rasteriser @resvg/resvg-js). The Rust workspace does
// not depend on it. See README.md for what each output is for.

import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "generated");
const checkOnly = process.argv.includes("--check");

const source = (name) => readFileSync(join(here, name), "utf8");

/** Rasterises an SVG to a PNG that is `size` px wide, on a transparent background. */
function png(svg, size) {
  const resvg = new Resvg(svg, {
    fitTo: { mode: "width", value: size },
    font: { loadSystemFonts: false },
  });
  return Buffer.from(resvg.render().asPng());
}

/** Raw straight-alpha RGBA pixels, for comparing renders. */
function rgba(svg, size) {
  const rendered = new Resvg(svg, {
    fitTo: { mode: "width", value: size },
    font: { loadSystemFonts: false },
  }).render();
  return rendered.pixels;
}

// ---------------------------------------------------------------------------------------------------------------
// Rasters

const master = source("master.svg");
const small = source("small-16.svg");

// 16 and 24 px use the simplified small-size artwork; everything larger uses the master.
const sizes = [16, 24, 32, 48, 64, 128, 192, 256, 512];
const pngFor = (size) => png(size <= 24 ? small : master, size);

// iOS rounds the apple-touch icon itself and fills transparent corners with black, so that one is full-bleed:
// the master without its rounded tile.
function fullBleed(svg) {
  const square = svg.replace('rx="17"', 'rx="0"');
  if (square === svg) throw new Error('master.svg: expected a tile clip with rx="17"');
  return square;
}

/** The inner markup of an SVG (everything between the <svg> tags). */
function inner(svg) {
  const match = svg.match(/<svg[^>]*>([\s\S]*)<\/svg>/);
  if (!match) throw new Error("not an SVG document");
  return match[1].trim();
}

// The maskable PWA icon: the Android adaptive layers stacked and cropped so that the 66-unit key-art safe circle
// becomes the 80 % safe circle that PWA maskable icons require (66 / 0.8 = 82.5 units of the 108-unit canvas).
const maskableSvg = (() => {
  const half = 82.5 / 2;
  const min = 54 - half;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${min} ${min} 82.5 82.5">\n` +
    `${inner(source("adaptive-background.svg"))}\n${inner(source("adaptive-foreground.svg"))}\n</svg>\n`
  );
})();

// ---------------------------------------------------------------------------------------------------------------
// The multi-size .ico. Every image is stored as PNG (Windows Vista and later read that at every size).

function ico(images) {
  const header = Buffer.alloc(6 + 16 * images.length);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(images.length, 4);
  let offset = header.length;
  images.forEach(({ size, data }, i) => {
    const at = 6 + 16 * i;
    header.writeUInt8(size >= 256 ? 0 : size, at); // width, 0 means 256
    header.writeUInt8(size >= 256 ? 0 : size, at + 1); // height
    header.writeUInt8(0, at + 2); // palette colours
    header.writeUInt8(0, at + 3); // reserved
    header.writeUInt16LE(1, at + 4); // colour planes
    header.writeUInt16LE(32, at + 6); // bits per pixel
    header.writeUInt32LE(data.length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += data.length;
  });
  return Buffer.concat([header, ...images.map((image) => image.data)]);
}

const icoSizes = [16, 24, 32, 48, 64, 256];

// ---------------------------------------------------------------------------------------------------------------
// The monochrome layer, flattened to plain paths.
//
// adaptive-monochrome.svg uses a mask (the tick cut out of the sun) and a clip path (the sun is cut off at the
// hill). Android VectorDrawable supports neither, so the same shape is computed here as plain filled paths:
//   * the sun: a circle cut off at the clip line, so a chord and a large arc;
//   * the tick: the stroked polyline with round caps and round joins, turned into its outline and added to the
//     sun's path with the opposite winding, so that it is a hole under the non-zero fill rule;
//   * the hill line: a stroked line with round caps, turned into a stadium.
// The geometry is read from the source file, so the two cannot drift apart.

const fmt = (n) => String(Math.round(n * 1000) / 1000);
const pt = (x, y) => `${fmt(x)},${fmt(y)}`;

/** The first number list of an SVG attribute. */
const numbers = (text) => text.match(/-?\d+(?:\.\d+)?/g).map(Number);

function attr(tag, name) {
  const match = tag.match(new RegExp(`\\s${name}="([^"]*)"`));
  if (!match) throw new Error(`adaptive-monochrome.svg: ${tag.slice(0, 40)}... has no ${name}`);
  return match[1];
}

/** Outline of an open polyline stroked with round caps and round joins, as an SVG path (one closed subpath). */
function strokeOutline(points, radius) {
  const normal = ([dx, dy]) => [-dy, dx];
  const at = (p, n) => [p[0] + radius * n[0], p[1] + radius * n[1]];

  /** One side of the stroke, walking the polyline forward. Returns path commands and the vertices it visits. */
  function side(pts) {
    const ds = [];
    for (let i = 0; i + 1 < pts.length; i++) {
      const dx = pts[i + 1][0] - pts[i][0];
      const dy = pts[i + 1][1] - pts[i][1];
      const len = Math.hypot(dx, dy);
      ds.push([dx / len, dy / len]);
    }
    const start = at(pts[0], normal(ds[0]));
    const commands = [];
    const vertices = [start];
    for (let k = 1; k + 1 < pts.length; k++) {
      const n1 = normal(ds[k - 1]);
      const n2 = normal(ds[k]);
      const cross = ds[k - 1][0] * ds[k][1] - ds[k - 1][1] * ds[k][0];
      if (cross > 0) {
        // This side is the inside of the turn: the two offset lines meet.
        const scale = radius / (1 + n1[0] * n2[0] + n1[1] * n2[1]);
        const p = [pts[k][0] + (n1[0] + n2[0]) * scale, pts[k][1] + (n1[1] + n2[1]) * scale];
        commands.push(`L${pt(...p)}`);
        vertices.push(p);
      } else if (cross < 0) {
        // The outside of the turn: a round join, an arc around the vertex.
        const a = at(pts[k], n1);
        const b = at(pts[k], n2);
        commands.push(`L${pt(...a)}`, `A${fmt(radius)},${fmt(radius)} 0 0 0 ${pt(...b)}`);
        vertices.push(a, b);
      }
    }
    const last = pts.length - 1;
    const end = at(pts[last], normal(ds[ds.length - 1]));
    commands.push(`L${pt(...end)}`);
    vertices.push(end);
    return { start, end, commands, vertices, lastPoint: pts[last] };
  }

  const forward = side(points);
  const back = side([...points].reverse());
  const cap = (to) => `A${fmt(radius)},${fmt(radius)} 0 0 0 ${pt(...to)}`;
  const d =
    `M${pt(...forward.start)}${forward.commands.join("")}` +
    `${cap(back.start)}${back.commands.join("")}${cap(forward.start)}Z`;
  const vertices = [...forward.vertices, ...back.vertices];
  // Shoelace sum over the visited vertices; positive is clockwise on screen (y points down).
  let area = 0;
  vertices.forEach((p, i) => {
    const q = vertices[(i + 1) % vertices.length];
    area += p[0] * q[1] - q[0] * p[1];
  });
  return { d, clockwise: area > 0 };
}

function flattenMonochrome() {
  const svg = source("adaptive-monochrome.svg");
  const tags = svg.match(/<(?:circle|path|rect)\b[^>]*>/g) ?? [];
  const circle = tags.find((tag) => tag.startsWith("<circle"));
  const tick = tags.find((tag) => tag.startsWith("<path") && attr(tag, "stroke") === "#000");
  const line = tags.find((tag) => tag.startsWith("<path") && attr(tag, "stroke") === "#fff");
  const clip = svg.match(/<clipPath[^>]*>\s*<rect[^>]*height="([\d.]+)"/);
  if (!circle || !tick || !line || !clip) throw new Error("adaptive-monochrome.svg has an unexpected shape");

  const [cx, cy, r] = [attr(circle, "cx"), attr(circle, "cy"), attr(circle, "r")].map(Number);
  const clipY = Number(clip[1]);
  const tickPoints = numbers(attr(tick, "d"));
  const tickPts = [0, 2, 4].map((i) => [tickPoints[i], tickPoints[i + 1]]);
  const tickRadius = Number(attr(tick, "stroke-width")) / 2;
  const linePts = numbers(attr(line, "d")); // M x1,y H x2
  const lineRadius = Number(attr(line, "stroke-width")) / 2;
  if (attr(tick, "stroke-linejoin") !== "round" || attr(tick, "stroke-linecap") !== "round") {
    throw new Error("the tick must have round joins and caps");
  }
  if (attr(line, "stroke-linecap") !== "round") throw new Error("the hill line must have round caps");

  // The tick must lie inside the clipped sun, or subtracting its outline would be wrong.
  const farthest = Math.max(...tickPts.map(([x, y]) => Math.hypot(x - cx, y - cy))) + tickRadius;
  const lowest = Math.max(...tickPts.map(([, y]) => y)) + tickRadius;
  if (farthest >= r || lowest >= clipY) throw new Error("the tick no longer lies inside the clipped sun");

  // The sun: a circle cut off at y = clipY. The centre is above the line, so the kept part is the large arc.
  if (cy >= clipY) throw new Error("the sun's centre must be above the clip line");
  const half = Math.sqrt(r * r - (clipY - cy) ** 2);
  const sun = `M${pt(cx - half, clipY)}A${fmt(r)},${fmt(r)} 0 1 1 ${pt(cx + half, clipY)}Z`;

  // The tick's outline must wind opposite to the sun's (clockwise) for the non-zero rule to cut it out.
  let outline = strokeOutline(tickPts, tickRadius);
  if (outline.clockwise) {
    outline = strokeOutline(tickPts.slice().reverse(), tickRadius);
    if (outline.clockwise) throw new Error("could not wind the tick outline counter-clockwise");
  }

  const [lx1, ly, lx2] = [linePts[0], linePts[1], linePts[2]];
  const hill =
    `M${pt(lx1, ly - lineRadius)}H${fmt(lx2)}A${fmt(lineRadius)},${fmt(lineRadius)} 0 0 1 ${pt(lx2, ly + lineRadius)}` +
    `H${fmt(lx1)}A${fmt(lineRadius)},${fmt(lineRadius)} 0 0 1 ${pt(lx1, ly - lineRadius)}Z`;

  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 108 108">\n` +
    `<path fill="#fff" fill-rule="nonzero" d="${sun}${outline.d}"/>\n` +
    `<path fill="#fff" d="${hill}"/>\n` +
    `</svg>\n`
  );
}

/** Fails if the flattened layer does not draw the same picture as the masked source. */
function verifyFlat(flat) {
  const size = 432; // 4 px per unit
  const a = rgba(source("adaptive-monochrome.svg"), size);
  const b = rgba(flat, size);
  let worst = 0;
  let total = 0;
  for (let i = 3; i < a.length; i += 4) {
    const diff = Math.abs(a[i] - b[i]);
    worst = Math.max(worst, diff);
    total += diff;
  }
  const mean = total / (a.length / 4);
  if (worst > 128 || mean > 0.1) {
    throw new Error(`the flattened monochrome layer differs from the source (worst ${worst}, mean ${mean.toFixed(3)})`);
  }
  if (/mask|clip/i.test(flat)) throw new Error("the flattened layer still has a mask or clip path");
}

// ---------------------------------------------------------------------------------------------------------------
// Outputs

const flat = flattenMonochrome();
verifyFlat(flat);

/** @type {Map<string, Buffer | string>} */
const outputs = new Map();
for (const size of sizes) outputs.set(`icon-${size}.png`, pngFor(size));
outputs.set("apple-touch-icon.png", png(fullBleed(master), 180));
outputs.set("maskable-512.png", png(maskableSvg, 512));
outputs.set("todo.ico", ico(icoSizes.map((size) => ({ size, data: pngFor(size) }))));
outputs.set("adaptive-monochrome-flat.svg", flat);

const stale = [];
for (const [name, content] of outputs) {
  const path = join(out, name);
  const bytes = Buffer.from(content);
  if (checkOnly) {
    if (!existsSync(path) || !readFileSync(path).equals(bytes)) stale.push(name);
  } else {
    mkdirSync(out, { recursive: true });
    writeFileSync(path, bytes);
  }
}

if (checkOnly) {
  const extra = existsSync(out) ? readdirSync(out).filter((name) => !outputs.has(name)) : [];
  if (stale.length || extra.length) {
    for (const name of stale) console.error(`stale or missing: generated/${name}`);
    for (const name of extra) console.error(`not generated any more: generated/${name}`);
    console.error("Run `node generate.mjs` in assets/brand and commit the result.");
    process.exit(1);
  }
  console.log(`brand assets are up to date (${outputs.size} files)`);
} else {
  console.log(`wrote ${outputs.size} files to ${out}`);
}
