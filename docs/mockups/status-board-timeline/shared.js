/* Shared helpers for Status, Board & Timeline mockups:
 * - Exact Daybreak desktop slim landscape (48 px) & Android Living Daybreak Horizon (72/80 dp)
 * - Authentic Lucide & hand-drawn glyph injection + 4 geometric workflow status SVGs
 * - Theme (?theme=light|dark) & capture (?capture=1) toggles
 */
(function () {
  const params = new URLSearchParams(window.location.search);
  const isCapture = params.get("capture") === "1";
  let theme = params.get("theme") === "dark" ? "dark" : "light";

  // Proposed single-colour SVGs (24-unit grid, stroke 1.6, currentColor — Playful native)
  const EXTRA_ICONS = {
    "status-backlog":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" stroke-dasharray="3.2 3.2"/></svg>',
    "status-todo":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/></svg>',
    "status-progress":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/><path d="M12 5.5A6.5 6.5 0 0 1 18.5 12A6.5 6.5 0 0 1 12 18.5Z" fill="currentColor" stroke="none"/></svg>',
    "status-done":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="9.5" fill="currentColor"/><path d="M8.2 12.4l2.5 2.5 5.3-5.6" stroke="var(--color-on-accent)" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    "columns-3":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/><path d="M15 3v18"/></svg>',
    "gantt-chart":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h10"/><path d="M9 12h11"/><path d="M6 18h9"/><circle cx="4" cy="6" r="0.5" fill="currentColor"/></svg>',
    "calendar-range":
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M16 2v4"/><path d="M8 2v4"/><path d="M3 10h18"/><path d="M8 14h8"/></svg>',
  };

  function applyModes() {
    document.body.classList.toggle("capture-mode", isCapture);
    document.body.classList.toggle("preview-mode", !isCapture);
    const root = document.querySelector(".desktop-window, .device-screen");
    if (root) root.setAttribute("data-theme", theme);
    renderLandscapes();
    updateToolbar();
  }

  // Exact port of crates/todo-design/src/landscape.rs (Landscape::slim — desktop 48 px)
  function buildSlimLandscape(width, height, done, total, isDark) {
    const pad = 32.0;
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
      parts.push(`<polyline points="${polyline(0.0, progress, 36)}" fill="none" stroke="var(--color-sun-ink)" stroke-width="1.5" stroke-linecap="round"/>`);
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
    return `<svg viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" preserveAspectRatio="none" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
  }

  // Living Daybreak Horizon Header for Android (docs/android-header.md)
  function buildHorizonHeader(width, totalHeight, sceneHeight, done, total, isDark) {
    const gradId = `hgrad-${width}-${totalHeight}`;
    const sceneTop = totalHeight - sceneHeight;
    const parts = [];
    if (isDark) {
      parts.push(`<defs><linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="var(--color-canvas)"/><stop offset="48%" stop-color="var(--color-sky)"/><stop offset="86%" stop-color="var(--color-horizon)"/><stop offset="100%" stop-color="var(--color-horizon)"/></linearGradient></defs><rect width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>`);
    } else {
      parts.push(`<defs><linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="var(--color-sky)"/><stop offset="52%" stop-color="var(--color-sky)"/><stop offset="100%" stop-color="var(--color-horizon)"/></linearGradient></defs><rect width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>`);
    }

    if (!isDark) {
      function cloud(cx, y, w, h) {
        const x0 = cx - w * 0.5, x1 = cx + w * 0.5;
        return `M${x0.toFixed(1)} ${y.toFixed(1)}C${(x0 + w * 0.04).toFixed(1)} ${(y - h * 0.58).toFixed(1)} ${(x0 + w * 0.20).toFixed(1)} ${(y - h * 0.62).toFixed(1)} ${(x0 + w * 0.28).toFixed(1)} ${(y - h * 0.52).toFixed(1)}C${(x0 + w * 0.36).toFixed(1)} ${(y - h * 1.05).toFixed(1)} ${(x0 + w * 0.64).toFixed(1)} ${(y - h * 1.05).toFixed(1)} ${(x0 + w * 0.72).toFixed(1)} ${(y - h * 0.48).toFixed(1)}C${(x0 + w * 0.84).toFixed(1)} ${(y - h * 0.56).toFixed(1)} ${(x1 - w * 0.04).toFixed(1)} ${(y - h * 0.36).toFixed(1)} ${x1.toFixed(1)} ${y.toFixed(1)}Z`;
      }
      parts.push(`<path d="${cloud(width * 0.78, sceneTop + 24, 78, 16)}" fill="var(--color-star)" opacity="0.48"/>`);
      parts.push(`<path d="${cloud(width * 0.20, sceneTop + 20, 56, 12)}" fill="var(--color-star)" opacity="0.34"/>`);
    } else {
      const stars = [[0.06, 0.18, 1.1], [0.24, 0.12, 1.0], [0.47, 0.10, 1.2], [0.66, 0.16, 1.0], [0.88, 0.11, 1.1]];
      for (const [sx, sy, sr] of stars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(sceneTop + sceneHeight * sy * 0.46).toFixed(1)}" r="${sr}" fill="var(--color-star)" opacity="0.7"/>`);
      }
    }

    const pad = width > 420 ? 44.0 : 36.0;
    const x0 = pad, x1 = width - pad;
    const arcBase = sceneHeight >= 80 ? totalHeight - 34.0 : totalHeight - 30.0;
    const arcPeak = sceneTop + 12.0;
    function arcPt(t) {
      const tc = Math.max(0, Math.min(1, t));
      return [x0 + (x1 - x0) * tc, arcBase - (arcBase - arcPeak) * Math.sin(Math.PI * tc)];
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
      const p = Math.min(done, total) / total;
      const [cx, cy] = arcPt(p);
      parts.push(`<polyline points="${arcPoly(p, 1.0, 56)}" fill="none" stroke="var(--color-arc)" stroke-width="1" stroke-linecap="round"/>`);
      if (p > 0) parts.push(`<polyline points="${arcPoly(0, p, 40)}" fill="none" stroke="var(--color-sun-ink)" stroke-width="1.75" stroke-linecap="round"/>`);
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="24" fill="var(--color-sun)" opacity="0.08"/>`);
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="13.5" fill="var(--color-sun)" opacity="0.22"/>`);
      for (let i = 0; i < 8; i++) {
        const a = (i * 45 * Math.PI) / 180;
        parts.push(`<line x1="${(cx + 9.5 * Math.sin(a)).toFixed(1)}" y1="${(cy - 9.5 * Math.cos(a)).toFixed(1)}" x2="${(cx + 13 * Math.sin(a)).toFixed(1)}" y2="${(cy - 13 * Math.cos(a)).toFixed(1)}" stroke="var(--color-sun)" stroke-width="1.6" stroke-linecap="round"/>`);
      }
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="7" fill="var(--color-sun)"/>`);
    }

    function hillD(topY, amp, pts) {
      const p = ([fx, dy]) => `${(width * fx).toFixed(1)} ${(topY + dy * amp).toFixed(1)}`;
      let d = `M0.0 ${totalHeight.toFixed(1)}L0.0 ${topY.toFixed(1)}`;
      for (let i = 0; i < pts.length; i += 3) d += `C${p(pts[i])} ${p(pts[i + 1])} ${p(pts[i + 2])}`;
      d += `L${width.toFixed(1)} ${totalHeight.toFixed(1)}Z`;
      return d;
    }
    const ridgePts = [[0.14, -5], [0.28, -9], [0.42, -3], [0.56, 3], [0.70, -7], [0.84, -4], [0.92, -2], [0.97, -5], [1.00, -3]];
    const farPts = [[0.12, -6], [0.22, 3], [0.36, -2], [0.50, -7], [0.62, -8], [0.74, -1], [0.86, 6], [0.93, 2], [1.00, -4]];
    const nearPts = [[0.15, -5], [0.30, 3], [0.48, -1], [0.66, -5], [0.76, -5], [0.90, 0], [1.04, 5], [0.98, 1], [1.00, -2]];
    const ridgeTop = sceneHeight >= 80 ? totalHeight - 38 : totalHeight - 34;
    const farTop = sceneHeight >= 80 ? totalHeight - 27 : totalHeight - 24;
    const nearTop = sceneHeight >= 80 ? totalHeight - 14 : totalHeight - 12;
    parts.push(`<path d="${hillD(ridgeTop, 1.5, ridgePts)}" fill="var(--color-hill-far)" opacity="${isDark ? 0.5 : 0.48}"/>`);
    parts.push(`<path d="${hillD(farTop, 1.6, farPts)}" fill="var(--color-hill-far)"/>`);
    parts.push(`<path d="${hillD(nearTop, 1.4, nearPts)}" fill="var(--color-hill-near)"/>`);
    return `<svg viewBox="0 0 ${width} ${totalHeight}" width="${width}" height="${totalHeight}" preserveAspectRatio="none" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
  }

  function renderLandscapes() {
    const isDark = theme === "dark";
    document.querySelectorAll("[data-landscape]").forEach((el) => {
      const w = parseFloat(el.dataset.width || "1192");
      const h = parseFloat(el.dataset.height || "48");
      const done = parseInt(el.dataset.done || "2", 10);
      const total = parseInt(el.dataset.total || "10", 10);
      el.innerHTML = buildSlimLandscape(w, h, done, total, isDark);
    });
    document.querySelectorAll("[data-horizon-header]").forEach((el) => {
      const w = parseFloat(el.dataset.width || "412");
      const th = parseFloat(el.dataset.totalHeight || "208");
      const sh = parseFloat(el.dataset.sceneHeight || "72");
      const done = parseInt(el.dataset.done || "2", 10);
      const total = parseInt(el.dataset.total || "10", 10);
      el.innerHTML = buildHorizonHeader(w, th, sh, done, total, isDark);
    });
  }

  function injectIcons() {
    const icons = window.TODO_ICONS || { lucide: {}, glyphs: {} };
    document.querySelectorAll("[data-icon]").forEach((el) => {
      const name = el.dataset.icon;
      const svg = icons.lucide[name] || EXTRA_ICONS[name];
      if (svg) el.innerHTML = svg;
    });
    document.querySelectorAll("[data-glyph]").forEach((el) => {
      const svg = icons.glyphs[el.dataset.glyph];
      if (svg) el.innerHTML = svg;
    });
  }

  function injectToolbar() {
    if (isCapture) return;
    const root = document.querySelector(".desktop-window, .device-screen");
    if (!root) return;
    const bar = document.createElement("div");
    bar.className = "dev-toolbar";
    bar.innerHTML = `<span class="meta">${document.title}</span><button data-btn="light">Light</button><button data-btn="dark">Dark</button>`;
    bar.addEventListener("click", (e) => {
      const btn = e.target.closest("button");
      if (!btn) return;
      theme = btn.dataset.btn;
      applyModes();
    });
    document.body.insertBefore(bar, root);
  }

  function updateToolbar() {
    const bar = document.querySelector(".dev-toolbar");
    if (!bar) return;
    bar.querySelector('[data-btn="light"]').classList.toggle("active", theme === "light");
    bar.querySelector('[data-btn="dark"]').classList.toggle("active", theme === "dark");
  }

  document.addEventListener("DOMContentLoaded", () => {
    injectIcons();
    injectToolbar();
    applyModes();
  });
})();
