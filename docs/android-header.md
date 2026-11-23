# Android Daybreak Room Header Specification

This specification upgrades the Android room header (`apps/android/.../ui/Header.kt` and `design/Landscape.kt`) so it feels fuller, calmer, and closer to the desktop visual reference while keeping the strict **Sky above, hands below** rule from [`docs/android-thumb-zone.md`](android-thumb-zone.md) and the **Playful / Daybreak** language from [`docs/design-language.md`](design-language.md).

---

## 1. Diagnosis & design goals

Real-device testing on a Galaxy Z Fold7 cover screen (`~412 × 910–960 dp`, dark theme, a normal list with 15 open tasks) highlighted two visual gaps:

1. **The latched pull-down reach state looks empty.** Today, `Header.kt` inserts a bare `Spacer(collapse.reach)` above the text block while leaving the `36 dp` landscape strip pinned at the bottom. Pulling down to bring the first row into thumb reach opens up to `180 dp` of flat solid colour with nothing in it.
2. **The at-rest header reads thin and sparse.**
   - A `36 dp` landscape strip (`40 dp` inner) leaves only `13 dp` of vertical arc rise (`arcBase = 23`, `arcPeak = 10`). The sun's path looks almost flat, and at `0 of N done` the sun sits half-submerged at the far-left edge.
   - The two hills (`amp = 1.0`) rise only `6–8 dp` above the bottom hairline.
   - In dark mode, `horizon` is painted as a hard-edged rectangle (`drawRect`), slicing horizontally across the strip—especially stark in empty lists, Activity, and Trash.
   - Placing the `33 sp` room title and `0 of 15 done · 15 open` on one row crowds a `412 dp` cover screen and truncates titles longer than ~10 characters. On desktop, any room narrower than `556 px` (`desktop::LIST_MIN`) already stacks the summary cleanly under the title.

### Non-negotiable invariants

- **Non-interactive top sky:** Zero buttons, filters, search icons, or overflow menus in the header. Every interactive control remains in the bottom thumb dock.
- **No duplication with the bottom dock:** Never repeat room pills, filter controls, quick-add hints, or Trash's bottom-bar deletion count in the header.
- **Pure Compose `Canvas` geometry:** Single-colour paths, stroked polylines, circles, and one vertical linear gradient per header. **Zero bitmaps beyond what exists, zero blur shaders, zero idle animations.**
- **Strict WCAG contrast:** Every text token sits over `canvas` / `sky` (or `dusk.top` in finished dusk) at **≥ 4.5:1** across all 360° room hues in both themes; the remaining arc and travelled stroke keep **≥ 3:1**.

---

## 2. Unified full-bleed sky & layered landscape

Instead of stacking a flat `sky` box on top of a tiny `36 dp` strip, the entire header—from `y = 0` behind the status bar down to the `1 dp` `line` hairline—shares **one unified sky canvas**.

### 2.1 Smooth 3-stop sky gradient (replaces hard `horizon` band)

Every colour comes from the existing room `Palette` and `Dusk` tables (`crates/todo-design/src/palette.rs`); no new `Palette` keys are needed.

| Mode / Theme | `0%` (status bar & zenith) | Mid stop (behind title & summary) | Horizon stop (behind arc & hills) | Contrast guarantee |
|---|---|---|---|---|
| **Light — Daytime / Evening** | `sky` (`L=0.962`) | `sky` at `52%` (`42%` in reach; `44%` in evening) | `horizon` (`L=0.950`) at `100%` | All text sits inside `0..52%` strictly over `sky` (`ink`, `ink-2`, `ink-3`, `accent-strong` ≥ 4.5:1). |
| **Dark — Daytime / Night** | `canvas` (`L=0.168`) | `sky` (`L=0.232`) at `48%` (`44%` in reach) | `horizon` (`L=0.315`) at `86%..100%` (`78%` in reach) | Zenith `canvas` is darker than `sky`, raising contrast behind the status bar and eyebrow; `horizon` glows softly only behind the lower arc and hills. |
| **Finished My Day (Dusk, both themes)** | `dusk.top` (`#0f1727`) | `dusk.top` at `38%` | `dusk.low` (`#724a31`) at `100%` | Night-theme ink over `dusk.top` switches at `DUSK_NIGHT_AT` (`0.4`). |

### 2.2 Three layered paper-cut hills

Adding a calm **distant ridge** behind the existing `far_hill` and `near_hill` gives depth to every room—especially empty lists, Activity, and Trash where there is no sun. It reuses `hill-far` (or `dusk.silhouette-far`) at translucent opacity (explicitly permitted for landscape scenery in `docs/design-language.md`):

| Layer (back to front) | Fill token | Opacity | Cover at rest (`72 dp`) | Inner at rest (`80 dp`) | Latched reach (`207 dp`) | Finished dusk (`164 dp`) |
|---|---|---|---|---|---|---|
| **0. Distant ridge** | `hill-far` (`dusk.silhouette-far`) | `RIDGE_OPACITY_LIGHT` (`0.48`) / `RIDGE_OPACITY_DARK` (`0.50`) / `RIDGE_OPACITY_DUSK` (`0.45`) | `top = h − 34`, `amp = 1.5` | `top = h − 38`, `amp = 1.5` | `top = h − 84`, `amp = 2.8` | `top = h − 74`, `amp = 2.6` |
| **1. Far hill** | `hill-far` (`dusk.silhouette-far`) | `1.0` | `top = h − 24`, `amp = 1.6` | `top = h − 27`, `amp = 1.6` | `top = h − 58`, `amp = 2.6` | `top = h − 58`, `amp = 3.0` |
| **2. Near hill** | `hill-near` (`dusk.silhouette-near`) | `1.0` | `top = h − 12`, `amp = 1.4` | `top = h − 14`, `amp = 1.4` | `top = h − 28`, `amp = 2.2` | `top = h − 26`, `amp = 3.0` |

Distant ridge cubic-Bézier control points (same `(widthFraction, amplitudeOffset)` format as `todo_design::landscape::Landscape::hill`):

```text
[(0.14, -5), (0.28, -9), (0.42, -3),
 (0.56,  3), (0.70, -7), (0.84, -4),
 (0.92, -2), (0.97, -5), (1.00, -3)]
```

### 2.3 Sweeping sun arc & sunrise baseline (`0%` progress fix)

When a room has `0 of 15 done`, the sun should read as **daybreak just clearing the eastern hills**, not a clipped dot trapped in the corner.

- **Horizontal inset (`pad`):** `36 dp` on cover (`width ≤ 420 dp`), `40 dp` on inner (`width > 420 dp`), `56 dp` on finished dusk.
- **Arc vertical bounds (within scene height `h`):**
  - Cover at rest (`h = 72 dp`): `arcBase = h − 30 = 42 dp`, `arcPeak = 12 dp` → **`30 dp` of vertical rise** (more than 2.3× the old `13 dp`).
  - Inner at rest (`h = 80 dp`): `arcBase = h − 34 = 46 dp`, `arcPeak = 12 dp` → **`34 dp` of vertical rise**.
  - Latched reach (`h = 207 dp`): `arcBase = h − 60 = 147 dp`, `arcPeak = 44 dp` → **`103 dp` sweeping semicircle**.
  - Finished dusk (`h = 164 dp`): unchanged `arcBase = h − 64 = 100 dp`, `arcPeak = 46 dp`, setting at `(x1 − 6, arcBase + 10)` behind the hills.
- **Why `arcBase = h − 30` (`h − 34` inner) works at `0%`:** At `x = 36 dp`, `far_hill` crests around `h − 28 dp`. Placing `arcBase` at `h − 30 dp` lets the sun disc (`r = 7.0 dp`) and its upper rays shine cleanly just above the eastern ridge crest—reading unmistakably as daybreak while the full `arc` curve sweeps overhead to the right.
- **Sun layers (back to front, all pure circles/lines):**
  1. **Outer atmospheric glow:** `r = 24 dp × scale`, `sun` at `OUTER_GLOW_OPACITY` (`0.08`). Pure `drawCircle`, no blur.
  2. **Reach outer ring *(reach only, `reachFraction > 0.2`)*:** `r = 36 dp × scale`, `1 dp` stroke, `sun` at `0.14 × reachFade`.
  3. **Halo:** `r = 13.5 dp × scale`, `sun` at `HALO_OPACITY` (`0.22`).
  4. **Eight rays:** `9.5 → 13.0 dp × scale`, `RAY_STROKE` (`1.6 dp`), round caps.
  5. **Sun disc:** `r = 7.0 dp × scale`, solid `sun`.
- **Arc strokes:** Remaining arc in `arc` at `ARC_STROKE` (`1.0 dp`); travelled arc in `sun-ink` at `TRAVELLED_STROKE_PHONE` (`1.75 dp`, round caps).

### 2.4 Ambient sky details by theme & local time of day

Sky ambience adapts quietly to the local hour (`LocalTime`), recomputed only when the hour bucket changes—**never animated on an idle loop**, and **deterministic via fixture clock** for Paparazzi tests.

| Theme | Time bucket (local hour) | Ambient elements |
|---|---|---|
| **Light** | **Daytime** (`05:00–17:59`) | 2 calm flat-bottom horizon clouds (`star` `#fdfdfd`, zero stroke, opacities `CLOUD_OPACITY` `0.48` at `x = 0.78w` and `0.34` at `x = 0.20w`). Reach adds a 3rd high wisp (`0.30`) at `x = 0.54w`. |
| **Light** | **Evening / Twilight** (`18:00–04:59`) | Horizon gradient warms slightly higher (`midStop = 44%`), clouds quiet to `0.28`, and 4 faint upper-right zenith stars appear at `0.25` opacity. |
| **Dark** | **Daytime** (`06:00–19:59`) | 10 shared `STARS` in the landscape sky (`STAR_OPACITY = 0.70`, kept above the distant ridge) + 6 upper-right zenith stars (`ZENITH_STAR_OPACITY = 0.30`, clear of left-aligned text). |
| **Dark** | **Night** (`20:00–05:59`) | Same starfield + whisper-quiet ambient crescent moon (`r = 9.5 dp`, upper-right `x = 0.84w`) in `star` at `AMBIENT_MOON_OPACITY` (`0.30`)—intentionally subtler and opposite-side from Finished Dusk's hero left crescent (`r = 13 dp`, `MOON_OPACITY = 0.90`). |
| **Finished My Day** | Any hour (both themes) | Always the celebratory **Dusk** scene: 3 dusk silhouettes, setting sun (`scale = 2.2`) + 3 concentric rings, hero left crescent moon (`0.90`), staggered sky + zenith stars. |

---

## 3. Layout, typography & glanceable content per state

### 3.1 Measurements on the 4 dp grid

| State | Posture | Status bar | Upper sky drift | Text block | Landscape scene | Total header (below status bar) | Visible `64 dp` rows (Fold7) |
|---|---|---|---|---|---|---|---|
| **At rest** | Cover (`412 × 960`) | `32–40 dp` | `0 dp` | `104 dp` | `72 dp` (`HEADER_SCENE_COVER`) | **`176 dp`** (`208 dp` incl. 32 dp status) | **~9.6 rows** |
| **At rest** | Inner left (`438 × 970`) | `32–40 dp` | `0 dp` | `104 dp` | `80 dp` (`HEADER_SCENE_INNER`) | **`184 dp`** (`216 dp` incl. 32 dp status) | **~9.6 rows** |
| **Latched reach** | Cover (`412 × 960`) | `32–40 dp` | `45 dp` (`0.25 × 180`) | `104 dp` | `207 dp` (`72 + 0.75 × 180`) | **`356 dp`** (`388 dp` incl. 32 dp status) | **~6.8 rows** (row 1 at thumb line) |
| **Finished My Day** | Cover (`412 × 960`) | `32–40 dp` | `0 dp` | `104 dp` | `164 dp` (`HEIGHT_FINISHED`) | **`268 dp`** (`300 dp` incl. 32 dp status) | Tomorrow hand-over + week |
| **Compact collapsed** | Cover / Inner | `32–40 dp` | `0 dp` | — | `2 dp` progress hairline | **`36 dp`** (`COMPACT_HEADER`) | **~11.8 rows** |
| **Keyboard / Short (`< 480 dp`)** | Cover / Inner | `32–40 dp` | `0 dp` | — | `2 dp` hairline (or hidden) | **`36 dp`** (or **`0 dp`** if `< 4` rows fit) | **≥ 4 rows** guaranteed |

### 3.2 Text block hierarchy (`104 dp` nominal at `1.0×` font scale)

Padding: `start = S5` (`20 dp`), `end = S5` (`20 dp`), `top = S3` (`12 dp`), `bottom = S2` (`8 dp`); vertical gap between rows = `S1` (`4 dp`).

```text
┌─────────────────────────────────────────────┐  y = 0 (status bar, transparent over sky)
│  10:35                            ▂▄▆█ 5G 🔋 │  32 dp
├ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┤  + 0..45 dp reach drift
│  Home  15 OPEN                  [Offline·2] │  20 dp  Eyebrow (EYEBROW_DAY_PHONE + META)
│  Groceries                                  │  36 dp  Title (ROOM_TITLE_PHONE, full width!)
│  0 of 15 done · 15 open         Next · 18:00│  20 dp  Summary + optional glanceable cue
│                                             │   8 dp  bottom text pad
│        ☁             · · ·          ☁       │
│           ╭───────────···───────────╮       │  72 dp  3-hill Daybreak landscape
│      ☀───╯     ╱╲         ╱╲        ╰       │  (80 dp inner / 207 dp reach / 164 dp dusk)
│  ╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱╲╱  │
└─────────────────────────────────────────────┘  1 dp `line` hairline → Task List (`pane`)
```

- **Row 1 — Eyebrow (`20 dp` min):** Exact shared `todo_client::room_header::Eyebrow` (`EYEBROW_DAY_PHONE` in `accent-strong` + `EYEBROW_META_PHONE` in `ink-3`, `9 dp × 1 dp` `line-strong` divider for dates). Trailing slot shows quiet sync status (`Offline`, `Offline · 2`, `3 pending`, `Sync error`) only when not cleanly synced.
- **Row 2 — Room title (`36 dp` min, full width):** `ROOM_TITLE_PHONE` (`Geist 600 33/36`, `-0.037em`, `ink`). Because the summary sits on Row 3 (matching desktop's `< 556 px` rule), titles like `"Weekly groceries"`, `"Agent activity"`, and `"Home maintenance"` never truncate prematurely.
- **Row 3 — Summary & optional glanceable cue (`20 dp` min):**
  - **Leading:** Exact shared `Summary` (`3 of 12 done · 9 open`, `0 of 15 done · 15 open`, `12 of 12 done · all clear`, `12 done`, `2 finished this month`, or `nothing yet`).
  - **Trailing (at most ONE, strictly non-interactive, omitted if neither applies):**
    1. **Overdue count** *(highest priority, task rooms except Completed/Activity/Trash)*: If `overdueCount > 0`, show the existing `OverdueBadge` (`◆ 2 overdue`, `20 dp` tall, `danger-bg` / `danger-ink`, already WCAG-guarded across all hues).
    2. **Next timed task today** *(only when `overdueCount == 0`)*: If any open task in the room is due today with a specific time (or due soon), show `Next · 18:00` (`SUMMARY_FIGURES`, weight `600`, `accent-strong`).
    3. **Otherwise:** Nothing—keep the right edge open and calm.
  - Under large font scale (`≥ 1.3×`) or narrow widths where both cannot fit with `S2` (`8 dp`) gap, Row 3 wraps cleanly into two lines (`FlowRow` with `verticalArrangement = spacedBy(S1)`), pushing the landscape down cleanly rather than clipping.

### 3.3 Room-type matrix

Every room hue (all 6 smart-list hues + all 12 swatches) uses the exact same sky gradient, 3-hill formula, and contrast-guarded inks.

| Room | Hue source | Eyebrow | Summary row | Glanceable cue | Landscape |
|---|---|---|---|---|---|
| **My Day** (open) | `my-day` (65°) | `Saturday 21 NOV 2026 \| W47` | `3 of 12 done · 9 open` | `◆ N overdue` or `Next · HH:MM` | 3 hills + sun on arc + clouds/stars |
| **My Day** (finished) | `my-day` → `Dusk` | `Saturday 21 NOV 2026 \| W47` *(night ink)* | `12 of 12 done · all clear` | None | `164 dp` Dusk: 3 silhouettes, setting sun + 3 rings, hero moon, stars |
| **Normal list** | List swatch | `<Collection> N OPEN` (or `List N OPEN`) | `0 of 15 done · 15 open` | `◆ N overdue` or `Next · HH:MM` | 3 hills + sun on arc (sunrise at 0%) + clouds/stars |
| **Collection** | Collection swatch | `Collection N LISTS` | `4 of 11 done · 7 open` | `◆ N overdue` or `Next · HH:MM` | 3 hills + sun on arc + clouds/stars |
| **Important / Planned / All** | Smart hue (28° / 150° / 95°) | `Saturday 21 NOV 2026 \| W47` | `done of total · open` | `◆ N overdue` or `Next · HH:MM` | 3 hills + sun on arc + clouds/stars |
| **Completed** | `completed` (135°) | `Saturday 21 NOV 2026 \| W47` | `12 done` | None | 3 hills + sun at `100%` arc end |
| **Agent activity** | `agent-activity` (262°) | `Saturday 21 NOV 2026 \| W47` | `3 of 7 done · 4 open` | None | 3 hills + sun on arc |
| **Empty task room** | Room hue | Same as room kind | `2 finished this month` or `nothing yet` | None | **3 hills + gradient + clouds/stars only (no sun, no arc)** |
| **Activity** | `agent-activity` (262°) | `History ALL CHANGES` / `AGENT: <Name>` | `20 shown · 4 today` *(when non-empty)* | None | **3 hills + gradient + clouds/stars only (no sun, no arc)** |
| **Trash** | `agent-activity` (262°) | `History DELETED ITEMS` | None *(count lives in bottom bar)* | None | **3 hills + gradient + clouds/stars only (no sun, no arc)** |

### 3.4 Compact collapsed header (`36 dp`)

When scrolled up (`collapse.fraction == 1f`) or forced by keyboard/short window:

- **Surface:** `sky` fill (or `dusk.top` on finished My Day; subtle `selection-wash` tint during bulk selection) with a `1 dp` bottom `line` hairline.
- **Horizon progress hairline (`COMPACT_PROGRESS_HEIGHT = 2 dp`):** Drawn along the bottom `2 dp` directly above the `1 dp` hairline when `total > 0`:
  - Full track: `arc` at `0.30` alpha.
  - Travelled bar (`width × progress`, animated with `SUN_TRAVEL` / `SUN`): `sun-ink`.
  - Omitted on empty lists, Activity, and Trash (`total == 0`).
- **Content (`paddingHorizontal = S5 = 20 dp`, `spacedBy(S2) = 8 dp`):**
  - Leading (`weight(1f)`): Room title in `ROOM_TITLE_BAR` (`18/20`, `ink`, 1-line ellipsis).
  - Trailing:
    - Optional compact overdue mark (`◆ 2`, `danger-ink`, `SUMMARY_FIGURES`, weight `600`, no pill container) when `overdueCount > 0`.
    - Miniature `8 dp` sun indicator (`total > 0`): `1.5 dp` stroked ring in `sun-ink` when `done == 0`; filled disc (`sun` in dark, `sun-ink` in light) when `done > 0`.
    - Compact count: `${done}/${total}` (`SUMMARY_FIGURES`, `ink-2`, tabular).
    - Quiet sync status (`Offline`, `Sync error`) if applicable.

---

## 4. Motion, pull-down reach & collapse choreography

### 4.1 Pull-down reach expansion (`cover` posture only)

Pull-down from list index `0`, offset `0` tracks the finger **1:1** via `HeaderCollapse.reach` (`0 .. reachLimit`, where `reachLimit = min(HEADER_REACH_EXPAND = 180 dp, viewportHeight × 0.28)`). Let `p = (reach / reachLimit).coerceIn(0f, 1f)`.

| Property | At rest (`p = 0`) | At full latch (`p = 1`, `reach = 180 dp`) | Formula / Parallax |
|---|---|---|---|
| **Upper sky drift above text** | `0 dp` | `45 dp` | `reach × HEADER_REACH_TEXT_SHARE` (`0.25`) |
| **Landscape scene height** | `72 dp` | `207 dp` | `HEADER_SCENE_COVER + reach × (1 − 0.25)` |
| **Distant ridge (layer 0)** | `top = h − 34`, `amp = 1.5` | `top = h − 84`, `amp = 2.8` | Parallax lag `0.35` (rises slowly to reveal horizon depth) |
| **Far hill (layer 1)** | `top = h − 24`, `amp = 1.6` | `top = h − 58`, `amp = 2.6` | Parallax lag `0.65` |
| **Near hill (layer 2)** | `top = h − 12`, `amp = 1.4` | `top = h − 28`, `amp = 2.2` | Grounded (`1.0` at bottom hairline) |
| **Sun arc rise (`arcBase − arcPeak`)** | `30 dp` | `103 dp` | Linear lerp of `arcBase` (`h−30 → h−60`) & `arcPeak` (`12 → 44`) |
| **Sun scale** | `1.0×` | `1.25×` | `1f + 0.25f * p` |
| **Extra reach stars / clouds / ambient moon** | `alpha = 0` | full token opacity | Fade in smoothly over `p ∈ [0.15, 1.0]` |

- **Latch & release:** Releasing past `HEADER_REACH_LATCH` (`96 dp`) settles to `reachLimit` over `PANEL` (`200 ms`, `STANDARD`). Tapping the sky, scrolling upward, tapping a task, opening any sheet, or entering bulk selection resets to `0` over `PANEL` (`200 ms`, `STANDARD`).
- **Finished My Day during reach:** Extends `dusk.top` zenith by `0.25 × reach` and grows the dusk panorama by `0.75 × reach`, revealing extra zenith stars and gently scaling the 3 dusk silhouettes while keeping the setting sun grounded behind the hills.
- **Unfolded inner posture:** Reach remains disabled (`reachLimit = 0f`), unchanged from `docs/android-thumb-zone.md` §3.2.

### 4.2 Scroll collapse into the `36 dp` compact header

As `collapse.fraction` moves `0.0 → 1.0` 1:1 with upward scroll (`onPreScroll`):

| Element | `fraction` window | Behaviour |
|---|---|---|
| **Eyebrow + Summary / Cue** | `0.00 → 0.40` | Linearly fades `alpha: 1 → 0`, drifts upward `0 → −8 dp`. |
| **Large Room Title** | `0.20 → 0.55` | Linearly fades `alpha: 1 → 0`, drifts upward `0 → −12 dp`. |
| **Landscape hills & sun** | `0.30 → 0.70` | Anchored to bottom of shrinking box; gently flattens vertically (`scaleY: 1.0 → 0.55` around bottom pivot) and fades `alpha: 1 → 0` into the `sky` surface. |
| **Compact `36 dp` header + `2 dp` progress bar** | `0.55 → 1.00` | Fades in (`alpha: 0 → 1`) cleanly at the bottom edge of the collapsing header. |

Releasing midway settles to `0.0` or `1.0` over `PANEL` (`200 ms`, `STANDARD`).

### 4.3 Reduced motion (`ANIMATOR_DURATION_SCALE == 0f`)

- **1:1 direct manipulation stays active:** Finger-driven pull-down reach and upward scroll still track height 1:1 (direct touch tracking is never turned off under reduced motion).
- **Zero parallax, zero scale:** During reach, all 3 hill layers move together (`parallax = 1.0`), sun stays at `1.0×` scale, and during scroll collapse the hills clip cleanly without vertical compression.
- **Instant settles, capped fades:** Fling/latch/collapse settles snap in `0 ms`; reach stars/clouds/moon and dusk transitions crossfade in `REDUCED_FADE` (`100 ms`).

---

## 5. Performance & Compose `Canvas` architecture

1. **Zero bitmaps & zero blur shaders:** Every layer (gradient, circles, lines, 3 hill paths, clouds, moon) is drawn with Compose `DrawScope` primitives (`drawRect` with `Brush.verticalGradient`, `drawCircle`, `drawLine`, `drawPath`).
2. **Zero per-frame `Path` allocations during 1:1 drag/scroll:**
   - Pre-build and cache (`remember(width, isDusk, dark)`) **two endpoint `Geometry` snapshots** per width: `restGeo` (`h = 72` or `80 dp`) and `reachGeo` (`h = 207 dp`), plus `duskGeo` (`h = 164 dp`) when finished/transitioning.
   - During a live pull-down drag (`0 < p < 1`), interpolate the 3 hill `Path`s by reusing a single pre-allocated mutable `Path` object (`path.rewind()` + cubic control point lerp between `restGeo` and `reachGeo` — zero GC allocations!) or drawing `restGeo` scaled/translated cleanly; sample the sun position from the interpolated half-sine formula directly in `drawScope` (only 1 `sin(πt)` call + 8 `drawLine` calls!).
   - Read `collapse.reach`, `collapse.fraction`, `sun()`, and `dusk()` **inside layout and draw lambdas only** so dragging or animating the sun never triggers recomposition of `HeaderBody` or the task list!
3. **Cache invalidation key:** Rebuild static paths only when `(widthDp, sceneKind, dark)` changes (window resize, fold/unfold, or theme toggle). Changing task progress (`done`/`total`) never rebuilds hill or cloud paths.

---

## 6. `todo-design` token mapping

### 6.1 Reused existing tokens

- **Palette (`palette.rs`):** `canvas`, `sky`, `horizon`, `tint`, `hill-far`, `hill-near`, `arc`, `sun`, `sun-ink`, `star`, `ink`, `ink-2`, `ink-3`, `ink-4`, `accent-strong`, `danger-bg`, `danger-ink`, `line`, `line-strong`, and `Dusk` (`top`, `low`, `silhouette-far`, `silhouette-near`, `star`, `sun`).
- **Layout (`layout.rs`):** `space::S1` (`4`), `S2` (`8`), `S3` (`12`), `S5` (`20`); `elevation::HAIRLINE` (`1`); `phone::COMPACT_HEADER` (`36`), `HEADER_REACH_EXPAND` (`180`), `HEADER_REACH_FRACTION` (`0.28`), `HEADER_REACH_LATCH` (`96`), `INNER_LIST` (`450`).
- **Landscape (`landscape.rs`):** `HEIGHT_FINISHED` (`164`), `ARC_STROKE` (`1.0`), `TRAVELLED_STROKE_FINISHED` (`1.2`), `RAY_STROKE` (`1.6`), `RING_STROKE` (`1.2`), `HALO_OPACITY` (`0.22`), `STAR_OPACITY` (`0.70`), `MOON_OPACITY` (`0.90`).
- **Typography (`typography.rs`):** `EYEBROW_DAY_PHONE`, `EYEBROW_META_PHONE`, `ROOM_TITLE_PHONE`, `ROOM_TITLE_BAR`, `SUMMARY`, `SUMMARY_FIGURES`, `OVERDUE_BADGE`.
- **Motion (`motion.rs`):** `PANEL` (`200`), `ROOM_HUE` (`360`), `SUN_TRAVEL` (`700`), `DUSK` (`1200`), `DUSK_NIGHT_AT` (`0.4`), `REDUCED_FADE` (`100`), `STANDARD`, `SUN`.

### 6.2 New additive tokens (added in `crates/todo-design`)

All 12 tokens below are additive (existing `LANDSCAPE_COVER` / `LANDSCAPE_INNER` and all `Palette` colour keys remain untouched for backwards compatibility across desktop, web, and existing Android snapshots until the implementation lands):

| Module | Rust constant | CSS / JSON key | Value | Purpose |
|---|---|---|---|---|
| `layout::phone` | `HEADER_SCENE_COVER` | `--phone-header-scene-cover` | `72.0` (dp) | At-rest Android Daybreak landscape height on cover screens (`412 dp`). |
| `layout::phone` | `HEADER_SCENE_INNER` | `--phone-header-scene-inner` | `80.0` (dp) | At-rest Android Daybreak landscape height on unfolded inner screens (`438–450 dp`). |
| `layout::phone` | `COMPACT_PROGRESS_HEIGHT` | `--phone-compact-progress-height` | `2.0` (dp) | Bottom horizon progress hairline height inside the `36 dp` compact header. |
| `layout::phone` | `HEADER_REACH_TEXT_SHARE` | `--phone-header-reach-text-share` | `0.25` | Share of pull-down reach added above the text block (`0.75` grows the landscape). |
| `landscape` | `TRAVELLED_STROKE_PHONE` | `--landscape-travelled-stroke-phone` | `1.75` (dp) | Travelled sun-arc stroke width on Android's taller horizon scene. |
| `landscape` | `OUTER_GLOW_OPACITY` | `--landscape-outer-glow-opacity` | `0.08` | Wide soft atmospheric circle behind the sun halo (pure circle, zero blur). |
| `landscape` | `RIDGE_OPACITY_LIGHT` | `--landscape-ridge-opacity-light` | `0.48` | Distant ridge layer opacity in light theme (`hill-far`). |
| `landscape` | `RIDGE_OPACITY_DARK` | `--landscape-ridge-opacity-dark` | `0.50` | Distant ridge layer opacity in dark theme (`hill-far` over `horizon` glow). |
| `landscape` | `RIDGE_OPACITY_DUSK` | `--landscape-ridge-opacity-dusk` | `0.45` | Distant ridge layer opacity in finished dusk (`dusk.silhouette-far`). |
| `landscape` | `CLOUD_OPACITY` | `--landscape-cloud-opacity` | `0.48` | Primary calm flat-bottom horizon cloud opacity in light daytime scenes (`star`). |
| `landscape` | `ZENITH_STAR_OPACITY` | `--landscape-zenith-star-opacity` | `0.30` | Subtle upper-sky zenith star opacity in dark theme and dusk headers. |
| `landscape` | `AMBIENT_MOON_OPACITY` | `--landscape-ambient-moon-opacity` | `0.30` | Subtle upper-right ambient crescent moon opacity in dark-theme night/reach sky. |

---

## 7. Accessibility & TalkBack

- **Semantic structure:**
  - The Room Title carries `Modifier.semantics { heading() }`.
  - The decorative landscape `Canvas` is hidden from accessibility (`clearAndSetSemantics {}` / non-focusable).
  - The full header announces one clean, scannable order: Eyebrow → Heading (Room title) → Progress summary → Optional glanceable cue (`"2 overdue"` or `"Next due at 18:00"`) → Optional sync issue.
  - When collapsed (`collapse.fraction > 0.5f`), the expanded header hides from accessibility and the `36 dp` compact bar exposes one merged `contentDescription`, e.g. `"My Day, 3 of 12 done, 2 overdue"`.
- **Latched pull-down reach escape:** When `collapse.reach > 0f`, the header remains tappable with `onClickLabel = "Return header to rest"` (no ripple/indication) so TalkBack users can collapse a latched sky in one activation without needing an upward drag.
- **Large font scale (`1.3× – 2.0×`):** The text container uses `heightIn(min = ...)` (never fixed `height`), Row 3 wraps cleanly (`FlowRow`) when summary + cue exceed the line width, and the landscape scene keeps its full `72 dp` / `80 dp` height below it without overlapping text.
- **Colour & contrast:** Overdue cue uses `OverdueBadge` (`danger-ink` on `danger-bg`, guarded ≥ 4.5:1) or `danger-ink` over `sky` in the compact bar; `Next · HH:MM` uses `accent-strong` over `sky` (guarded ≥ 4.5:1 across all 360° hues). All scenic hills, clouds, stars, and glows remain decorative (`1:1` informational per `docs/design-language.md`).

---

## 8. Visual mockups

All mockups are rendered at 2× from HTML using the real generated `crates/todo-design/generated/tokens.css`, bundled Geist / Geist Mono fonts, authentic project icons, and neutral sample data (`docs/mockups/thumb-zone/`):

| State | HTML mockup | Light PNG | Dark PNG |
|---|---|---|---|
| **Cover at rest — My Day** (`3/12`, `2 overdue`) | `docs/mockups/thumb-zone/header-cover-rest.html` | `header-cover-rest-light.png` | `header-cover-rest-dark.png` |
| **Cover at rest — Normal List** (`Groceries`, `0/15`, sunrise) | `docs/mockups/thumb-zone/header-cover-list.html` | `header-cover-list-light.png` | `header-cover-list-dark.png` |
| **Cover — Latched pull-down reach** (`207 dp` expanded panorama) | `docs/mockups/thumb-zone/header-cover-reach.html` | `header-cover-reach-light.png` | `header-cover-reach-dark.png` |
| **Cover — Finished My Day (Dusk)** (`12/12 all clear`) | `docs/mockups/thumb-zone/header-cover-dusk.html` | `header-cover-dusk-light.png` | `header-cover-dusk-dark.png` |
| **Inner two-pane at rest** (`876 × 970`, `Home` `80 dp` scene) | `docs/mockups/thumb-zone/header-inner-rest.html` | `header-inner-rest-light.png` | `header-inner-rest-dark.png` |
| **Compact collapsed header** (`36 dp` + `2 dp` horizon hairline) | `docs/mockups/thumb-zone/header-cover-compact.html` | `header-cover-compact-light.png` | `header-cover-compact-dark.png` |

Re-render all PNGs from the repo root:

```sh
cd apps/web/e2e && npm ci --ignore-scripts && cd ../../.. && node docs/mockups/render.mjs
```

---

## 9. Phased implementation checklist

### Phase 1 — Shared view-model cue & landscape geometry (`crates/todo-client`, `crates/todo-ffi`, `crates/todo-design`)

- [ ] Add optional glanceable header cue derivation to `todo_client::room_header` (or `crates/todo-ffi/src/view.rs` `RoomView`):
  - `overdue_count: u32` (open overdue tasks in the room; `0` in Completed, Activity, Trash).
  - `next_due_today: Option<String>` (earliest upcoming timed task today in `"HH:MM"` format when `overdue_count == 0`, else `None`).
  - `local_hour: u8` (0..23 from `LocalTime`, deterministic under test fixtures) for ambient day/evening/night sky selection.
- [ ] Add `ridge_hill()` and optional `clouds()` path helpers in `todo_design::landscape` (or keep hill control points shared in `todo_design::landscape` and expose via `todo-ffi`'s `LandscapeShapes` alongside `far_hill` and `near_hill`).
- [ ] Verify `cargo test --workspace`, wasm32, and Android target checks pass.

### Phase 2 — Unified Sky Canvas & 3-layer Landscape (`apps/android/.../design/Landscape.kt`)

- [ ] Replace the flat `sky` fill + hard-edged `drawRect(r.horizon)` band with the unified full-bleed 3-stop vertical gradient (`canvas → sky → horizon` in dark; `sky → sky → horizon` in light; `dusk.top → dusk.low` in dusk) extending behind `statusBarsPadding()`.
- [ ] Upgrade Android landscape height from `LANDSCAPE_COVER` / `LANDSCAPE_INNER` to `HEADER_SCENE_COVER` (`72 dp`) / `HEADER_SCENE_INNER` (`80 dp`).
- [ ] Render the 3-layer hills (`ridge` → `far` → `near`), sweeping arc (`arcBase = h − 30 / h − 34`, `arcPeak = 12`, `TRAVELLED_STROKE_PHONE = 1.75 dp`), outer glow circle (`OUTER_GLOW_OPACITY`), daytime flat-bottom clouds (`CLOUD_OPACITY`), dark zenith stars (`ZENITH_STAR_OPACITY`), and late-night subtle ambient moon (`AMBIENT_MOON_OPACITY`).
- [ ] Pre-allocate/cache `Path` instances per `(width, sceneKind, dark)` so `Canvas` draw frames perform zero path allocations.

### Phase 3 — Stacked Typography & Glanceable Cue (`apps/android/.../ui/Header.kt`)

- [ ] Move `Summary(room)` onto Row 3 directly below `ROOM_TITLE_PHONE`, giving the room title 100% width with `Modifier.semantics { heading() }`.
- [ ] Place the read-only glanceable cue (`OverdueBadge` or `Next · HH:MM`) at the trailing edge of Row 3 using `FlowRow` fallback for large font scales (`≥ 1.3×`).
- [ ] Ensure Trash keeps `Summary.None` (no duplication of its bottom-bar count) and Activity optionally shows `"N shown · M today"` when non-empty.
- [ ] Update the `36 dp` compact collapsed bar to include the `2 dp` `COMPACT_PROGRESS_HEIGHT` bottom horizon progress hairline (`sun-ink` over `arc` @ 30%), `8 dp` sun ring/disc indicator, and optional compact `◆ N` overdue count.

### Phase 4 — Live 1:1 Reach Parallax & Smooth Collapse (`Header.kt`, `Collapse.kt`, `Landscape.kt`)

- [ ] Remove `Spacer(Modifier.height(collapse.reach))` above the header text block.
- [ ] Split `collapse.reach` into `reach * HEADER_REACH_TEXT_SHARE` (`0.25`) upper sky drift above the text block and `reach * 0.75` landscape panorama expansion below it, driving 3-layer hill parallax (`0.35 / 0.65 / 1.0`), arc lift (`30 → 103 dp`), sun scale (`1.0 → 1.25×`), and reach stars/clouds/moon fade-in (`reachFraction ∈ [0.15, 1.0]`).
- [ ] Wire smooth scroll-collapse curves (`eyebrow/summary 0.0..0.4`, `title 0.2..0.55`, `hills 0.3..0.7`, `compact bar 0.55..1.0`) reading `collapse.fraction` and `collapse.reach` solely in layout/draw phases.
- [ ] Verify reduced motion (`LocalMotionDurationScale == 0f` / `ANIMATOR_DURATION_SCALE == 0f`): 1:1 height tracking preserved, parallax locked to `1.0`, sun scale locked to `1.0×`, `0 ms` settles, `REDUCED_FADE` (`100 ms`) opacity crossfades.

### Phase 5 — Paparazzi Gallery & Device Verification (`GalleryTest.kt`, `HeaderCollapseTest.kt`)

- [ ] Update existing `FOLD7_COVER` & `FOLD7_INNER` Paparazzi snapshots (`cover_my_day`, `cover_reach_pull`, `cover_finished`, `cover_collapsed`, `cover_empty`, `inner_personal`, `activity`, `trash`) in both light and dark themes.
- [ ] Add a dedicated zero-progress normal list snapshot (`cover_list_zero_progress_dark` / `_light`, e.g. `0 of 15 done · 15 open` in teal hue) and a large-font-scale (`1.5×`) header wrapping test.
- [ ] Verify unit tests in `HeaderCollapseTest.kt` for reach split math, reduced-motion clamping, and smooth collapse ranges, plus a manual Fold7 check in light/dark themes across cover, inner, IME-open, and latched reach.
