/* Shared helpers for AI Assistant mockups:
 * - Authentic Lucide & hand-drawn glyph injection from icons.js
 * - Desktop Daybreak Landscape::slim and Android Living Daybreak Horizon SVG rendering
 * - Theme (?theme=light|dark) and capture (?capture=1) modes
 */
(function () {
  const params = new URLSearchParams(window.location.search);
  const isCapture = params.get("capture") === "1";
  let theme = params.get("theme") === "dark" ? "dark" : "light";

  function applyModes() {
    document.body.classList.toggle("capture-mode", isCapture);
    document.body.classList.toggle("preview-mode", !isCapture);
    const root = document.querySelector("[data-room-hue]");
    if (root) root.setAttribute("data-theme", theme);
    renderLandscapes();
    updateToolbar();
  }

  // Exact port of crates/todo-design/src/landscape.rs (Landscape::slim) for desktop 48 px strip
  function buildDesktopLandscapeSvg(width, height, done, total, isDark) {
    const pad = 28.0;
    const x0 = pad;
    const x1 = width - pad;
    const arcBase = height - 13.0;
    const arcPeak = 10.0;
    const farTop = height - 15.0;
    const nearTop = height - 7.0;
    const horizonTop = arcBase - 12.0;

    function arcPoint(t) {
      const tc = Math.max(0, Math.min(1, t));
      return [
        x0 + (x1 - x0) * tc,
        arcBase - (arcBase - arcPeak) * Math.sin(Math.PI * tc),
      ];
    }
    function polyline(from, to, segs) {
      const pts = [];
      for (let i = 0; i <= segs; i++) {
        const [x, y] = arcPoint(from + ((to - from) * i) / segs);
        pts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
      }
      return pts.join(" ");
    }
    function hillPath(top, pts) {
      const p = ([fx, dy]) => `${(width * fx).toFixed(1)} ${(top + dy).toFixed(1)}`;
      let d = `M0.0 ${height.toFixed(1)}L0.0 ${top.toFixed(1)}`;
      for (let i = 0; i < pts.length; i += 3) {
        d += `C${p(pts[i])} ${p(pts[i + 1])} ${p(pts[i + 2])}`;
      }
      d += `L${width.toFixed(1)} ${height.toFixed(1)}Z`;
      return d;
    }
    const farPts = [
      [0.12, -6], [0.22, 3], [0.36, -2],
      [0.50, -7], [0.62, -8], [0.74, -1],
      [0.86, 6], [0.93, 2], [1.00, -4],
    ];
    const nearPts = [
      [0.15, -5], [0.30, 3], [0.48, -1],
      [0.66, -5], [0.76, -5], [0.90, 0],
      [1.04, 5], [0.98, 1], [1.00, -2],
    ];
    const stars = [
      [0.06, 0.18, 1.1], [0.15, 0.42, 0.8], [0.24, 0.12, 1.0],
      [0.37, 0.30, 0.8], [0.47, 0.10, 1.2], [0.58, 0.36, 0.8],
      [0.66, 0.16, 1.0], [0.79, 0.28, 0.9], [0.88, 0.10, 1.1],
      [0.95, 0.40, 0.8],
    ];

    const parts = [];
    parts.push(`<rect x="0" y="0" width="${width}" height="${height}" fill="var(--color-sky)"/>`);
    if (isDark) {
      parts.push(`<rect x="0" y="${horizonTop.toFixed(1)}" width="${width}" height="${(height - horizonTop).toFixed(1)}" fill="var(--color-horizon)"/>`);
      for (const [sx, sy, sr] of stars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(height * sy).toFixed(1)}" r="${sr}" fill="var(--color-star)" opacity="0.7"/>`);
      }
    }
    if (total > 0) {
      const progress = Math.min(done, total) / total;
      const [cx, cy] = arcPoint(progress);
      parts.push(`<polyline points="${polyline(progress, 1.0, 56)}" fill="none" stroke="var(--color-arc)" stroke-width="1" stroke-linecap="round"/>`);
      parts.push(`<polyline points="${polyline(0.0, progress, 40)}" fill="none" stroke="var(--color-sun-ink)" stroke-width="1.5" stroke-linecap="round"/>`);
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="11" fill="var(--color-sun)" opacity="0.22"/>`);
      for (let i = 0; i < 8; i++) {
        const a = (i * 45 * Math.PI) / 180;
        const xA = cx + 8.5 * Math.sin(a);
        const yA = cy - 8.5 * Math.cos(a);
        const xB = cx + 11.5 * Math.sin(a);
        const yB = cy - 11.5 * Math.cos(a);
        parts.push(`<line x1="${xA.toFixed(1)}" y1="${yA.toFixed(1)}" x2="${xB.toFixed(1)}" y2="${yB.toFixed(1)}" stroke="var(--color-sun)" stroke-width="1.6" stroke-linecap="round"/>`);
      }
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="6.2" fill="var(--color-sun)"/>`);
    }
    parts.push(`<path d="${hillPath(farTop, farPts)}" fill="var(--color-hill-far)"/>`);
    parts.push(`<path d="${hillPath(nearTop, nearPts)}" fill="var(--color-hill-near)"/>`);
    return `<svg viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
  }

  // Living Daybreak Horizon Header (docs/android-header.md)
  function buildHorizonHeaderSvg(opts) {
    const { width, totalHeight, sceneHeight, done, total, isDark } = opts;
    const gradId = `sky-grad-${Math.round(width)}-${Math.round(totalHeight)}`;
    const sceneTop = totalHeight - sceneHeight;
    const parts = [];

    if (isDark) {
      parts.push(`
        <defs>
          <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--color-canvas)"/>
            <stop offset="48%" stop-color="var(--color-sky)"/>
            <stop offset="86%" stop-color="var(--color-horizon)"/>
            <stop offset="100%" stop-color="var(--color-horizon)"/>
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>
      `);
      const zenithStars = [
        [0.64, 0.14, 0.9], [0.76, 0.09, 1.1], [0.85, 0.21, 0.8],
        [0.92, 0.12, 1.0], [0.56, 0.23, 0.8], [0.71, 0.27, 0.9],
      ];
      for (const [sx, sy, sr] of zenithStars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(Math.max(sceneTop + 12, 110) * sy + 26).toFixed(1)}" r="${sr}" fill="var(--color-star)" opacity="var(--landscape-zenith-star-opacity)"/>`);
      }
      const stars = [
        [0.06, 0.18, 1.1], [0.15, 0.40, 0.8], [0.24, 0.12, 1.0],
        [0.37, 0.28, 0.8], [0.47, 0.10, 1.2], [0.58, 0.34, 0.8],
        [0.66, 0.16, 1.0], [0.79, 0.26, 0.9], [0.88, 0.11, 1.1],
        [0.95, 0.36, 0.8],
      ];
      for (const [sx, sy, sr] of stars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(sceneTop + sceneHeight * sy * 0.46).toFixed(1)}" r="${sr}" fill="var(--color-star)" opacity="var(--landscape-star-opacity)"/>`);
      }
    } else {
      parts.push(`
        <defs>
          <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--color-sky)"/>
            <stop offset="52%" stop-color="var(--color-sky)"/>
            <stop offset="100%" stop-color="var(--color-horizon)"/>
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>
      `);
      function cloudPath(cx, baselineY, w, h) {
        const x0 = cx - w * 0.5;
        const x1 = cx + w * 0.5;
        return `M${x0.toFixed(1)} ${baselineY.toFixed(1)}` +
          `C${(x0 + w * 0.04).toFixed(1)} ${(baselineY - h * 0.58).toFixed(1)} ${(x0 + w * 0.20).toFixed(1)} ${(baselineY - h * 0.62).toFixed(1)} ${(x0 + w * 0.28).toFixed(1)} ${(baselineY - h * 0.52).toFixed(1)}` +
          `C${(x0 + w * 0.36).toFixed(1)} ${(baselineY - h * 1.05).toFixed(1)} ${(x0 + w * 0.64).toFixed(1)} ${(baselineY - h * 1.05).toFixed(1)} ${(x0 + w * 0.72).toFixed(1)} ${(baselineY - h * 0.48).toFixed(1)}` +
          `C${(x0 + w * 0.84).toFixed(1)} ${(baselineY - h * 0.56).toFixed(1)} ${(x1 - w * 0.04).toFixed(1)} ${(baselineY - h * 0.36).toFixed(1)} ${x1.toFixed(1)} ${baselineY.toFixed(1)}Z`;
      }
      parts.push(`<path d="${cloudPath(width * 0.78, sceneTop + 24, 78, 16)}" fill="var(--color-star)" opacity="var(--landscape-cloud-opacity)"/>`);
      parts.push(`<path d="${cloudPath(width * 0.20, sceneTop + 20, 56, 12)}" fill="var(--color-star)" opacity="0.34"/>`);
    }

    const pad = width > 420 ? 40.0 : 36.0;
    const x0 = pad;
    const x1 = width - pad;
    const arcBase = sceneHeight >= 80 ? totalHeight - 34.0 : totalHeight - 30.0;
    const arcPeak = sceneTop + 12.0;

    function arcPt(t) {
      const tc = Math.max(0, Math.min(1, t));
      return [
        x0 + (x1 - x0) * tc,
        arcBase - (arcBase - arcPeak) * Math.sin(Math.PI * tc),
      ];
    }
    function arcPoly(from, to, segs) {
      const pts = [];
      for (let i = 0; i <= segs; i++) {
        const [px, py] = arcPt(from + ((to - from) * i) / segs);
        pts.push(`${px.toFixed(1)},${py.toFixed(1)}`);
      }
      return pts.join(" ");
    }

    if (total > 0) {
      const progress = Math.min(done, total) / total;
      const [cx, cy] = arcPt(progress);
      parts.push(`<polyline points="${arcPoly(progress, 1.0, 56)}" fill="none" stroke="var(--color-arc)" stroke-width="1.0" stroke-linecap="round"/>`);
      if (progress > 0) {
        parts.push(`<polyline points="${arcPoly(0.0, progress, 44)}" fill="none" stroke="var(--color-sun-ink)" stroke-width="1.75" stroke-linecap="round"/>`);
      }
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="24" fill="var(--color-sun)" opacity="var(--landscape-outer-glow-opacity)"/>`);
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="13.5" fill="var(--color-sun)" opacity="var(--landscape-halo-opacity)"/>`);
      for (let i = 0; i < 8; i++) {
        const a = (i * 45 * Math.PI) / 180;
        const xA = cx + 9.5 * Math.sin(a);
        const yA = cy - 9.5 * Math.cos(a);
        const xB = cx + 13.0 * Math.sin(a);
        const yB = cy - 13.0 * Math.cos(a);
        parts.push(`<line x1="${xA.toFixed(1)}" y1="${yA.toFixed(1)}" x2="${xB.toFixed(1)}" y2="${yB.toFixed(1)}" stroke="var(--color-sun)" stroke-width="1.6" stroke-linecap="round"/>`);
      }
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="7.0" fill="var(--color-sun)"/>`);
    }

    function hillD(topY, amp, pts) {
      const p = ([fx, dy]) => `${(width * fx).toFixed(1)} ${(topY + dy * amp).toFixed(1)}`;
      let d = `M0.0 ${totalHeight.toFixed(1)}L0.0 ${topY.toFixed(1)}`;
      for (let i = 0; i < pts.length; i += 3) {
        d += `C${p(pts[i])} ${p(pts[i + 1])} ${p(pts[i + 2])}`;
      }
      d += `L${width.toFixed(1)} ${totalHeight.toFixed(1)}Z`;
      return d;
    }
    const ridgePts = [
      [0.14, -5], [0.28, -9], [0.42, -3],
      [0.56, 3], [0.70, -7], [0.84, -4],
      [0.92, -2], [0.97, -5], [1.00, -3],
    ];
    const farPts = [
      [0.12, -6], [0.22, 3], [0.36, -2],
      [0.50, -7], [0.62, -8], [0.74, -1],
      [0.86, 6], [0.93, 2], [1.00, -4],
    ];
    const nearPts = [
      [0.15, -5], [0.30, 3], [0.48, -1],
      [0.66, -5], [0.76, -5], [0.90, 0],
      [1.04, 5], [0.98, 1], [1.00, -2],
    ];
    const isInner = sceneHeight >= 80;
    const ridgeOp = isDark ? "var(--landscape-ridge-opacity-dark)" : "var(--landscape-ridge-opacity-light)";
    parts.push(`<path d="${hillD(totalHeight - (isInner ? 38 : 34), 1.5, ridgePts)}" fill="var(--color-hill-far)" opacity="${ridgeOp}"/>`);
    parts.push(`<path d="${hillD(totalHeight - (isInner ? 27 : 24), 1.6, farPts)}" fill="var(--color-hill-far)"/>`);
    parts.push(`<path d="${hillD(totalHeight - (isInner ? 14 : 12), 1.4, nearPts)}" fill="var(--color-hill-near)"/>`);
    return `<svg viewBox="0 0 ${width} ${totalHeight}" width="${width}" height="${totalHeight}" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
  }

  function renderLandscapes() {
    const isDark = theme === "dark";
    document.querySelectorAll("[data-landscape]").forEach((el) => {
      const w = parseFloat(el.dataset.width || "792");
      const h = parseFloat(el.dataset.height || "48");
      const done = parseInt(el.dataset.done || "0", 10);
      const total = parseInt(el.dataset.total || "0", 10);
      el.innerHTML = buildDesktopLandscapeSvg(w, h, done, total, isDark);
    });
    document.querySelectorAll("[data-horizon-header]").forEach((el) => {
      el.innerHTML = buildHorizonHeaderSvg({
        width: parseFloat(el.dataset.width || "412"),
        totalHeight: parseFloat(el.dataset.totalHeight || "208"),
        sceneHeight: parseFloat(el.dataset.sceneHeight || "72"),
        done: parseInt(el.dataset.done || "0", 10),
        total: parseInt(el.dataset.total || "0", 10),
        isDark,
      });
    });
  }

  function injectIcons() {
    const icons = window.TODO_ICONS;
    if (!icons) return;
    document.querySelectorAll("[data-icon]").forEach((el) => {
      const svg = icons.lucide[el.dataset.icon];
      if (svg) el.innerHTML = svg;
    });
    document.querySelectorAll("[data-glyph]").forEach((el) => {
      const svg = icons.glyphs[el.dataset.glyph];
      if (svg) el.innerHTML = svg;
    });
  }

  function mountToolbar() {
    if (isCapture) return;
    const bar = document.createElement("div");
    bar.className = "dev-toolbar";
    bar.innerHTML = `
      <span class="meta">${document.title}</span>
      <button type="button" data-btn="light">Light</button>
      <button type="button" data-btn="dark">Dark</button>
      <a href="index.html" style="color:#d6cdc4;text-decoration:none;margin-left:8px;font-size:12px;">← Gallery</a>
    `;
    bar.querySelector('[data-btn="light"]').addEventListener("click", () => {
      theme = "light";
      applyModes();
    });
    bar.querySelector('[data-btn="dark"]').addEventListener("click", () => {
      theme = "dark";
      applyModes();
    });
    document.body.prepend(bar);
  }

  function updateToolbar() {
    const l = document.querySelector('.dev-toolbar [data-btn="light"]');
    const d = document.querySelector('.dev-toolbar [data-btn="dark"]');
    if (l) l.classList.toggle("active", theme === "light");
    if (d) d.classList.toggle("active", theme === "dark");
  }

  window.addEventListener("DOMContentLoaded", () => {
    injectIcons();
    mountToolbar();
    applyModes();
  });
})();
