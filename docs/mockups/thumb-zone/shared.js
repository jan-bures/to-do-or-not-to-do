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

  // Living Daybreak Horizon Header (docs/android-header.md)
  // Renders the unified full-bleed sky gradient, zenith stars, clouds, ambient moon,
  // sweeping sun arc, and 3 layered hills (distant ridge, far hill, near hill).
  function buildHorizonHeaderSvg(opts) {
    const { width, totalHeight, sceneHeight, done, total, mode, isDark } = opts;
    const isDusk = mode === "dusk";
    const isReach = mode === "reach";
    const gradId = `sky-grad-${Math.round(width)}-${Math.round(totalHeight)}-${mode}`;
    const sceneTop = totalHeight - sceneHeight;
    const parts = [];

    // 1. Unified 3-stop vertical sky gradient (zero harsh horizon rectangle)
    if (isDusk) {
      parts.push(`
        <defs>
          <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--dusk-top)"/>
            <stop offset="38%" stop-color="var(--dusk-top)"/>
            <stop offset="100%" stop-color="var(--dusk-low)"/>
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>
      `);
    } else if (isDark) {
      const midStop = isReach ? "44%" : "48%";
      const glowStop = isReach ? "78%" : "86%";
      parts.push(`
        <defs>
          <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--color-canvas)"/>
            <stop offset="${midStop}" stop-color="var(--color-sky)"/>
            <stop offset="${glowStop}" stop-color="var(--color-horizon)"/>
            <stop offset="100%" stop-color="var(--color-horizon)"/>
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>
      `);
    } else {
      const midStop = isReach ? "42%" : "52%";
      parts.push(`
        <defs>
          <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="var(--color-sky)"/>
            <stop offset="${midStop}" stop-color="var(--color-sky)"/>
            <stop offset="100%" stop-color="var(--color-horizon)"/>
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="${width}" height="${totalHeight}" fill="url(#${gradId})"/>
      `);
    }

    // 2. Zenith stars (upper sky in dark & dusk; kept clear of left title text)
    if (isDark || isDusk) {
      const starFill = isDusk ? "var(--dusk-star)" : "var(--color-star)";
      const zenithStars = [
        [0.64, 0.14, 0.9], [0.76, 0.09, 1.1], [0.85, 0.21, 0.8],
        [0.92, 0.12, 1.0], [0.56, 0.23, 0.8], [0.71, 0.27, 0.9],
      ];
      const upperBandH = Math.max(sceneTop + 12, 110);
      for (const [sx, sy, sr] of zenithStars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(upperBandH * sy + 26).toFixed(1)}" r="${sr}" fill="${starFill}" opacity="var(--landscape-zenith-star-opacity)"/>`);
      }

      // Extra high-altitude reach stars in the opened zenith when latched
      if (isReach) {
        const reachStars = [
          [0.10, 0.16, 1.0], [0.22, 0.11, 0.8], [0.35, 0.18, 1.1],
          [0.48, 0.12, 0.9], [0.60, 0.17, 1.0], [0.74, 0.13, 0.8],
          [0.86, 0.19, 1.1], [0.94, 0.14, 0.9],
        ];
        for (const [sx, sy, sr] of reachStars) {
          parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(sceneTop + sceneHeight * sy).toFixed(1)}" r="${sr}" fill="${starFill}" opacity="0.58"/>`);
        }
        // Whisper-quiet ambient crescent moon in upper-right of dark reach sky
        const mx = width * 0.84;
        const my = sceneTop + 34;
        const mStartX = mx + 4.5;
        const mStartY = my - 9.5;
        const moonD = `M${mStartX.toFixed(1)} ${mStartY.toFixed(1)}A9.5 9.5 0 1 0 ${(mStartX + 4.5).toFixed(1)} ${(mStartY + 17.0).toFixed(1)}A7.8 7.8 0 0 1 ${mStartX.toFixed(1)} ${mStartY.toFixed(1)}Z`;
        parts.push(`<path d="${moonD}" fill="${starFill}" opacity="var(--landscape-ambient-moon-opacity)"/>`);
      }

      // Lower scene stars (shared 10-star constellation above the distant ridge)
      const stars = [
        [0.06, 0.18, 1.1], [0.15, 0.40, 0.8], [0.24, 0.12, 1.0],
        [0.37, 0.28, 0.8], [0.47, 0.10, 1.2], [0.58, 0.34, 0.8],
        [0.66, 0.16, 1.0], [0.79, 0.26, 0.9], [0.88, 0.11, 1.1],
        [0.95, 0.36, 0.8],
      ];
      const yScale = isDusk ? 0.56 : (isReach ? 0.54 : 0.46);
      const rScale = isDusk ? 1.35 : (isReach ? 1.15 : 1.0);
      for (const [sx, sy, sr] of stars) {
        parts.push(`<circle cx="${(width * sx).toFixed(1)}" cy="${(sceneTop + sceneHeight * sy * yScale).toFixed(1)}" r="${(sr * rScale).toFixed(1)}" fill="${starFill}" opacity="var(--landscape-star-opacity)"/>`);
      }
    }

    // 3. Calm flat-bottom daytime clouds (light theme only, zero strokes)
    if (!isDark && !isDusk) {
      function cloudPath(cx, baselineY, w, h) {
        const x0 = cx - w * 0.5;
        const x1 = cx + w * 0.5;
        return `M${x0.toFixed(1)} ${baselineY.toFixed(1)}` +
          `C${(x0 + w * 0.04).toFixed(1)} ${(baselineY - h * 0.58).toFixed(1)} ${(x0 + w * 0.20).toFixed(1)} ${(baselineY - h * 0.62).toFixed(1)} ${(x0 + w * 0.28).toFixed(1)} ${(baselineY - h * 0.52).toFixed(1)}` +
          `C${(x0 + w * 0.36).toFixed(1)} ${(baselineY - h * 1.05).toFixed(1)} ${(x0 + w * 0.64).toFixed(1)} ${(baselineY - h * 1.05).toFixed(1)} ${(x0 + w * 0.72).toFixed(1)} ${(baselineY - h * 0.48).toFixed(1)}` +
          `C${(x0 + w * 0.84).toFixed(1)} ${(baselineY - h * 0.56).toFixed(1)} ${(x1 - w * 0.04).toFixed(1)} ${(baselineY - h * 0.36).toFixed(1)} ${x1.toFixed(1)} ${baselineY.toFixed(1)}Z`;
      }
      if (isReach) {
        parts.push(`<path d="${cloudPath(width * 0.76, sceneTop + 46, 98, 22)}" fill="var(--color-star)" opacity="var(--landscape-cloud-opacity)"/>`);
        parts.push(`<path d="${cloudPath(width * 0.23, sceneTop + 68, 76, 17)}" fill="var(--color-star)" opacity="0.38"/>`);
        parts.push(`<path d="${cloudPath(width * 0.54, sceneTop + 26, 62, 13)}" fill="var(--color-star)" opacity="0.30"/>`);
      } else {
        parts.push(`<path d="${cloudPath(width * 0.78, sceneTop + 24, 78, 16)}" fill="var(--color-star)" opacity="var(--landscape-cloud-opacity)"/>`);
        parts.push(`<path d="${cloudPath(width * 0.20, sceneTop + 20, 56, 12)}" fill="var(--color-star)" opacity="0.34"/>`);
      }
    }

    // 4. Finished-day hero crescent moon (left side, MOON_OPACITY 0.9)
    if (isDusk) {
      const mx = width * 0.16;
      const my = sceneTop + 56;
      const sx = mx + 6.0;
      const sy = my - 13.0;
      const moonD = `M${sx.toFixed(1)} ${sy.toFixed(1)}A13 13 0 1 0 ${(sx + 6.0).toFixed(1)} ${(sy + 23.0).toFixed(1)}A10.5 10.5 0 0 1 ${sx.toFixed(1)} ${sy.toFixed(1)}Z`;
      parts.push(`<path d="${moonD}" fill="var(--dusk-star)" opacity="var(--landscape-moon-opacity)"/>`);
    }

    // 5. Sweeping sun arc & sun (when total > 0 or dusk)
    const pad = isDusk ? 56.0 : (width > 420 ? 40.0 : 36.0);
    const x0 = pad;
    const x1 = width - pad;
    const arcBase = isDusk
      ? totalHeight - 64.0
      : (isReach ? totalHeight - 60.0 : (sceneHeight >= 80 ? totalHeight - 34.0 : totalHeight - 30.0));
    const arcPeak = isDusk
      ? sceneTop + 46.0
      : (isReach ? sceneTop + 44.0 : sceneTop + 12.0);

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

    if (total > 0 || isDusk) {
      const progress = isDusk ? 1.0 : Math.min(done, total) / total;
      let [cx, cy] = arcPt(progress);
      if (isDusk) {
        cx = x1 - 6.0;
        cy = arcBase + 10.0;
      }
      const scale = isDusk ? 2.2 : (isReach ? 1.25 : 1.0);
      const sunFill = isDusk ? "var(--dusk-sun)" : "var(--color-sun)";

      if (isDusk) {
        parts.push(`<polyline points="${arcPoly(0.0, 1.0, 56)}" fill="none" stroke="var(--dusk-sun)" opacity="0.5" stroke-width="1.2" stroke-linecap="round"/>`);
        for (const [rr, ro] of [[44, 0.32], [64, 0.18], [86, 0.09]]) {
          parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${rr}" fill="none" stroke="var(--dusk-sun)" stroke-width="1.2" opacity="${ro}"/>`);
        }
      } else {
        parts.push(`<polyline points="${arcPoly(progress, 1.0, 56)}" fill="none" stroke="var(--color-arc)" stroke-width="1.0" stroke-linecap="round"/>`);
        if (progress > 0) {
          parts.push(`<polyline points="${arcPoly(0.0, progress, 44)}" fill="none" stroke="var(--color-sun-ink)" stroke-width="1.75" stroke-linecap="round"/>`);
        }
        // Pure-circle atmospheric outer glow (zero blur shader)
        parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${(24 * scale).toFixed(1)}" fill="${sunFill}" opacity="var(--landscape-outer-glow-opacity)"/>`);
        if (isReach) {
          parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${(36 * scale).toFixed(1)}" fill="none" stroke="${sunFill}" stroke-width="1.0" opacity="0.14"/>`);
        }
      }

      const haloR = (isDusk ? 11.0 : 13.5) * scale;
      const discR = (isDusk ? 6.2 : 7.0) * scale;
      const rayIn = (isDusk ? 8.5 : 9.5) * scale;
      const rayOut = (isDusk ? 11.5 : 13.0) * scale;

      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${haloR.toFixed(1)}" fill="${sunFill}" opacity="var(--landscape-halo-opacity)"/>`);
      for (let i = 0; i < 8; i++) {
        const a = (i * 45 * Math.PI) / 180;
        const xA = cx + rayIn * Math.sin(a);
        const yA = cy - rayIn * Math.cos(a);
        const xB = cx + rayOut * Math.sin(a);
        const yB = cy - rayOut * Math.cos(a);
        parts.push(`<line x1="${xA.toFixed(1)}" y1="${yA.toFixed(1)}" x2="${xB.toFixed(1)}" y2="${yB.toFixed(1)}" stroke="${sunFill}" stroke-width="1.6" stroke-linecap="round"/>`);
      }
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${discR.toFixed(1)}" fill="${sunFill}"/>`);
    }

    // 6. Three layered paper-cut hills (distant ridge, far hill, near hill)
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

    const ridgeTop = isDusk
      ? totalHeight - 74.0
      : (isReach ? totalHeight - 84.0 : (sceneHeight >= 80 ? totalHeight - 38.0 : totalHeight - 34.0));
    const farTop = isDusk
      ? totalHeight - 58.0
      : (isReach ? totalHeight - 58.0 : (sceneHeight >= 80 ? totalHeight - 27.0 : totalHeight - 24.0));
    const nearTop = isDusk
      ? totalHeight - 26.0
      : (isReach ? totalHeight - 28.0 : (sceneHeight >= 80 ? totalHeight - 14.0 : totalHeight - 12.0));

    const ridgeAmp = isDusk ? 2.6 : (isReach ? 2.8 : 1.5);
    const farAmp = isDusk ? 3.0 : (isReach ? 2.6 : 1.6);
    const nearAmp = isDusk ? 3.0 : (isReach ? 2.2 : 1.4);

    const ridgeColor = isDusk ? "var(--dusk-silhouette-far)" : "var(--color-hill-far)";
    const ridgeOpacity = isDusk
      ? "var(--landscape-ridge-opacity-dusk)"
      : (isDark ? "var(--landscape-ridge-opacity-dark)" : "var(--landscape-ridge-opacity-light)");
    const farColor = isDusk ? "var(--dusk-silhouette-far)" : "var(--color-hill-far)";
    const nearColor = isDusk ? "var(--dusk-silhouette-near)" : "var(--color-hill-near)";

    parts.push(`<path d="${hillD(ridgeTop, ridgeAmp, ridgePts)}" fill="${ridgeColor}" opacity="${ridgeOpacity}"/>`);
    parts.push(`<path d="${hillD(farTop, farAmp, farPts)}" fill="${farColor}"/>`);
    parts.push(`<path d="${hillD(nearTop, nearAmp, nearPts)}" fill="${nearColor}"/>`);

    return `<svg viewBox="0 0 ${width} ${totalHeight}" width="${width}" height="${totalHeight}" preserveAspectRatio="none" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
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
    document.querySelectorAll("[data-horizon-header]").forEach((el) => {
      const header = el.closest(".horizon-header");
      const w = parseFloat(el.dataset.width || (header ? header.offsetWidth : 412) || "412");
      const totalHeight = parseFloat(el.dataset.totalHeight || (header ? header.offsetHeight : 208) || "208");
      const sceneHeight = parseFloat(el.dataset.sceneHeight || "72");
      const done = parseInt(el.dataset.done || "3", 10);
      const total = parseInt(el.dataset.total || "12", 10);
      const mode = el.dataset.mode || "day";
      el.innerHTML = buildHorizonHeaderSvg({ width: w, totalHeight, sceneHeight, done, total, mode, isDark });
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
