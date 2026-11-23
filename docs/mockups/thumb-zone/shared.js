/* Shared helpers for Android thumb-zone mockups:
 * - Exact Daybreak landscape geometry from crates/todo-design/src/landscape.rs
 * - Authentic Lucide & hand-drawn glyph injection from icons.js
 * - Theme (?theme=light|dark), capture (?capture=1), and thumb-zone heat map (?zones=1)
 */
(function () {
  const params = new URLSearchParams(window.location.search);
  const isCapture = params.get("capture") === "1";
  let theme = params.get("theme") === "dark" ? "dark" : "light";
  let showZones = params.get("zones") === "1";

  function applyModes() {
    document.body.classList.toggle("capture-mode", isCapture);
    document.body.classList.toggle("preview-mode", !isCapture);
    document.body.classList.toggle("show-zones", showZones);
    const screen = document.querySelector(".device-screen");
    if (screen) screen.setAttribute("data-theme", theme);
    renderLandscapes();
    updateToolbar();
  }

  // Exact port of crates/todo-design/src/landscape.rs (Landscape::slim)
  function buildLandscapeSvg(width, height, done, total, isDark) {
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
      parts.push(`<polyline points="${polyline(progress, 1.0, 48)}" fill="none" stroke="var(--color-arc)" stroke-width="1" stroke-linecap="round"/>`);
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

    return `<svg viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
  }

  function renderLandscapes() {
    const isDark = theme === "dark";
    document.querySelectorAll("[data-landscape]").forEach((el) => {
      const w = parseFloat(el.dataset.width || "412");
      const h = parseFloat(el.dataset.height || "36");
      const done = parseInt(el.dataset.done || "3", 10);
      const total = parseInt(el.dataset.total || "12", 10);
      el.innerHTML = buildLandscapeSvg(w, h, done, total, isDark);
    });
  }

  function injectIcons() {
    if (!window.TODO_ICONS) return;
    document.querySelectorAll("[data-icon]").forEach((el) => {
      const svg = window.TODO_ICONS.lucide[el.dataset.icon];
      if (svg) el.innerHTML = svg;
    });
    document.querySelectorAll("[data-glyph]").forEach((el) => {
      const svg = window.TODO_ICONS.glyphs[el.dataset.glyph];
      if (svg) el.innerHTML = svg;
    });
  }

  // Ergonomic thumb-zone heat map overlay (Cover 412x960 or Unfolded 876x970)
  function buildCoverZonesSvg(labelBottom) {
    const W = 412, H = 960;
    const note = labelBottom || "All primary controls sit in the Comfortable zone";
    return `
    <svg viewBox="0 0 ${W} ${H}" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <clipPath id="screen-clip"><rect x="0" y="0" width="${W}" height="${H}"/></clipPath>
      </defs>
      <g clip-path="url(#screen-clip)">
        <!-- Unreachable / Forget about it (top ~34%) -->
        <path d="M0,0 L${W},0 L${W},310 Q206,255 0,310 Z" fill="rgba(220, 56, 46, 0.24)"/>
        <!-- Difficult (upper-mid) -->
        <path d="M0,310 Q206,255 ${W},310 L${W},455 Q206,400 0,455 Z" fill="rgba(238, 128, 36, 0.20)"/>
        <!-- OK / Stretch (lower-mid) -->
        <path d="M0,455 Q206,400 ${W},455 L${W},610 Q206,555 0,610 Z" fill="rgba(234, 184, 30, 0.20)"/>
        <!-- Comfortable (two-thumb natural arc, bottom ~38%) -->
        <path d="M0,610 Q206,555 ${W},610 L${W},${H} L0,${H} Z" fill="rgba(42, 166, 78, 0.22)"/>

        <!-- Crisp arc boundaries -->
        <path d="M0,310 Q206,255 ${W},310" fill="none" stroke="rgba(200, 44, 36, 0.65)" stroke-width="1.5" stroke-dasharray="5 4"/>
        <path d="M0,455 Q206,400 ${W},455" fill="none" stroke="rgba(210, 108, 22, 0.65)" stroke-width="1.5" stroke-dasharray="5 4"/>
        <path d="M0,610 Q206,555 ${W},610" fill="none" stroke="rgba(30, 146, 66, 0.78)" stroke-width="2" stroke-dasharray="6 4"/>

        <!-- Zone badges -->
        <g font-family="Geist Mono, monospace" font-size="10.5" font-weight="600" letter-spacing="0.06em">
          <rect x="112" y="232" width="188" height="22" rx="6" fill="rgba(166, 34, 28, 0.88)"/>
          <text x="206" y="246.5" fill="#fff" text-anchor="middle">UNREACHABLE · SKY ONLY</text>

          <rect x="150" y="366" width="112" height="22" rx="6" fill="rgba(186, 92, 14, 0.86)"/>
          <text x="206" y="380.5" fill="#fff" text-anchor="middle">DIFFICULT</text>

          <rect x="148" y="514" width="116" height="22" rx="6" fill="rgba(156, 118, 6, 0.88)"/>
          <text x="206" y="528.5" fill="#fff" text-anchor="middle">STRETCH / OK</text>

          <rect x="52" y="592" width="308" height="24" rx="6" fill="rgba(22, 126, 56, 0.92)"/>
          <text x="206" y="607.5" fill="#fff" text-anchor="middle">COMFORTABLE THUMB ZONE (2 THUMBS)</text>
        </g>

        <!-- Clean summary note placed directly under Comfortable arc (never covers bottom controls) -->
        <g font-family="Geist, sans-serif" font-size="10.5" font-weight="600">
          <rect x="16" y="622" width="380" height="22" rx="6" fill="rgba(20, 26, 22, 0.86)"/>
          <text x="206" y="636.5" fill="#b4f5c6" text-anchor="middle">${note}</text>
        </g>
      </g>
    </svg>`;
  }

  function buildInnerZonesSvg() {
    const W = 876, H = 970, mid = 438;
    return `
    <svg viewBox="0 0 ${W} ${H}" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <clipPath id="inner-clip"><rect x="0" y="0" width="${W}" height="${H}"/></clipPath>
      </defs>
      <g clip-path="url(#inner-clip)">
        <!-- Unreachable top-center & center dead zone -->
        <rect x="0" y="0" width="${W}" height="${H}" fill="rgba(220, 56, 46, 0.14)"/>

        <!-- Left thumb stretch band -->
        <ellipse cx="0" cy="${H - 120}" rx="420" ry="520" fill="rgba(234, 184, 30, 0.16)" stroke="rgba(196, 148, 12, 0.65)" stroke-width="1.5" stroke-dasharray="5 4"/>
        <!-- Right thumb stretch band -->
        <ellipse cx="${W}" cy="${H - 120}" rx="420" ry="520" fill="rgba(234, 184, 30, 0.16)" stroke="rgba(196, 148, 12, 0.65)" stroke-width="1.5" stroke-dasharray="5 4"/>

        <!-- Left thumb comfortable arc (~60 mm real reach on 8" Fold7 inner display) -->
        <ellipse cx="0" cy="${H - 76}" rx="368" ry="370" fill="rgba(42, 166, 78, 0.24)" stroke="rgba(28, 146, 66, 0.85)" stroke-width="2" stroke-dasharray="6 4"/>
        <!-- Right thumb comfortable arc -->
        <ellipse cx="${W}" cy="${H - 76}" rx="368" ry="370" fill="rgba(42, 166, 78, 0.24)" stroke="rgba(28, 146, 66, 0.85)" stroke-width="2" stroke-dasharray="6 4"/>

        <!-- Foldable crease indicator -->
        <line x1="${mid}" y1="0" x2="${mid}" y2="${H}" stroke="rgba(255,255,255,0.65)" stroke-width="2" stroke-dasharray="4 6"/>
        <line x1="${mid}" y1="0" x2="${mid}" y2="${H}" stroke="rgba(40,40,40,0.45)" stroke-width="1" stroke-dasharray="4 6"/>

        <!-- Labels -->
        <g font-family="Geist Mono, monospace" font-size="11" font-weight="600" letter-spacing="0.06em">
          <rect x="304" y="138" width="268" height="24" rx="6" fill="rgba(166, 34, 28, 0.90)"/>
          <text x="438" y="154" fill="#fff" text-anchor="middle">TOP-CENTRE DEAD ZONE · GLANCEABLE</text>

          <rect x="378" y="458" width="120" height="22" rx="6" fill="rgba(30, 28, 26, 0.86)"/>
          <text x="438" y="472.5" fill="#fff" text-anchor="middle">FOLD / HINGE</text>

          <rect x="24" y="696" width="234" height="24" rx="6" fill="rgba(22, 126, 56, 0.92)"/>
          <text x="141" y="711.5" fill="#fff" text-anchor="middle">LEFT THUMB · LIST &amp; ROOMS</text>

          <rect x="612" y="696" width="240" height="24" rx="6" fill="rgba(22, 126, 56, 0.92)"/>
          <text x="732" y="711.5" fill="#fff" text-anchor="middle">RIGHT THUMB · DETAIL ACTIONS</text>
        </g>
      </g>
    </svg>`;
  }

  function injectZones() {
    const overlay = document.querySelector(".zones-overlay");
    if (!overlay) return;
    const kind = overlay.dataset.zonesKind || "cover";
    const note = overlay.dataset.zonesNote || "";
    overlay.innerHTML = kind === "inner" ? buildInnerZonesSvg() : buildCoverZonesSvg(note);
  }

  function injectToolbar() {
    if (isCapture) return;
    const screen = document.querySelector(".device-screen");
    if (!screen) return;
    const bar = document.createElement("div");
    bar.className = "dev-toolbar";
    const title = document.title.replace(" — Thumb-Zone Mockup", "");
    bar.innerHTML = `
      <span class="meta">${title}</span>
      <button type="button" data-btn="light">Light</button>
      <button type="button" data-btn="dark">Dark</button>
      <button type="button" data-btn="zones">Thumb Zones</button>
    `;
    bar.addEventListener("click", (e) => {
      const btn = e.target.closest("button");
      if (!btn) return;
      if (btn.dataset.btn === "light") theme = "light";
      if (btn.dataset.btn === "dark") theme = "dark";
      if (btn.dataset.btn === "zones") showZones = !showZones;
      applyModes();
    });
    document.body.insertBefore(bar, screen);
  }

  function updateToolbar() {
    const bar = document.querySelector(".dev-toolbar");
    if (!bar) return;
    bar.querySelector('[data-btn="light"]').classList.toggle("active", theme === "light");
    bar.querySelector('[data-btn="dark"]').classList.toggle("active", theme === "dark");
    bar.querySelector('[data-btn="zones"]').classList.toggle("active", showZones);
  }

  document.addEventListener("DOMContentLoaded", () => {
    injectIcons();
    injectZones();
    injectToolbar();
    applyModes();
  });
})();
