# Android thumb-zone layout specification

This specification redesigns the Android client (`apps/android`) for effortless thumb reach on tall foldables and large phones while preserving the **Playful / Daybreak** design language (`docs/design-language.md`). Every layout number comes from `todo-design` (`crates/todo-design`), and every interaction keeps a visible, accessible alternative.

## 1. Ergonomic problem & target postures

On modern tall devices, traditional top app bars, top-aligned drawers, and mid-screen popovers place almost every navigation and triage control outside natural thumb reach.

We design explicitly for two primary postures (using the **Galaxy Z Fold7** as the reference foldable alongside standard phones and short/landscape windows):

| Posture | Reference viewport (dp) | Aspect | Grip model | Comfortable zone (green) | Stretch / OK zone (yellow) | Unreachable zone (red) |
|---|---|---|---|---|---|---|
| **Folded (cover)** | `412 × 960 dp` (`21:9`, ~6.5")<br>*(also `412 × 920 dp` baseline)* | Very tall portrait | Two thumbs low, or single hand (right or left) | Bottom ~38% (`y = 595..960 dp`, ~365 dp from bottom) | Middle ~20% (`y = 400..595 dp`) | Top ~42% (`y = 0..400 dp`) |
| **Unfolded (inner)** | `876 × 970 dp` (~8.0" portrait)<br>*(also `900 × 1000 dp` baseline)* | Near-square two-pane | Both hands holding left & right edges; two thumbs | Lower-left arc (`x = 0..270 dp`, `y = 540..970 dp`) & lower-right arc (`x = 606..876 dp`, `y = 540..970 dp`) | Side-edge upper bands (`y = 340..540 dp`, outer 220 dp each side) | Top-centre (`x = 200..676 dp`, `y = 0..340 dp`) & centre-middle across the hinge |
| **Short / landscape** | `maxHeight < 480 dp` (e.g. `960 × 412 dp`) | Landscape cover | Both thumbs on sides | Bottom strip & side edges | Entire list viewport | Top chrome collapsed |

### Core principle: **Sky above, hands below**

The Playful design language already splits the world into a **sky** and **ground**:

1. **The top of the screen is glanceable, non-interactive orientation.** The Daybreak sky, eyebrow, room title, progress summary, and sun arc live at the top where the eye looks first—never cluttered with top-left hamburger buttons or top-right overflow menus. In Task Detail, the top strip holds only the calm breadcrumb (`Personal / Admin`).
2. **Every interactive control anchors to the bottom thumb zone.** Room switching, sibling room pills, filtering, room actions, quick-add, bulk selection status and tools, task detail actions, menus, pickers, and confirmation dialogs all live at the bottom edge (or aligned to the bottom-left / bottom-right pane corners when unfolded).
3. **Nothing crosses the hinge when unfolded.** The left pane (`List`) belongs to the **left thumb**; the right pane (`Detail`) belongs to the **right thumb**. Sheets triggered from either pane dock to the bottom of *that* pane instead of floating in the unreachable centre.
4. **Zero design-language compromises.** Flat surfaces separated by colour steps and `1 dp` hairlines, opaque overlays, zero blur, zero pill bloat, strict `4 dp` grid, WCAG 4.5:1 text / 3:1 control contrast, and calm `todo-design` motion.

---

## 2. Handedness & mirroring

Two-thumb use on both cover and inner screens is the primary assumption, so the layout is **symmetric-first**: high-frequency controls span the bottom edge cleanly where both thumbs rest naturally.

For one-handed cover use, the Settings sheet offers a local-only preference:

- **Setting:** `Thumb-zone hand` — `Right-handed` *(default)* | `Left-handed`, stored in app-local `SharedPreferences` (never synced to the server; device-specific ergonomics).
- **What mirrors on Cover (compact width) when set to `Left-handed`:**
  1. **Bottom Utility & Room Bar:** Default `[≡ Rooms]` · `[Room Pills…]` · `[🔍]` `[⋯]` mirrors to `[⋯]` `[🔍]` · `[Room Pills…]` · `[≡ Rooms]`.
  2. **Task Detail Bottom Bar:** Default `[← Back]` · *(spacer)* · `[○/✓]` `[☀]` `[★]` `[⋯]` mirrors to `[⋯]` `[★]` `[☀]` `[○/✓]` · *(spacer)* · `[← Back]`.
  3. **Room Switcher Footer:** Default `[+ New]` · `• 20:18` · `[⚙]` mirrors to `[⚙]` · `• 20:18` · `[+ New]`, and collection fold chevrons move to the leading edge if desired—or remain right-aligned while the whole row is a `48 dp` target. *(Recommendation: keep collection chevrons on the trailing edge of rows so hierarchy indentation stays readable; mirror only the bottom bar and footer actions.)*
  4. **Quick-Add Calendar / Priority popovers:** Keep alignment pinned to their button edge (`Calendar` and `Flag` stay at the trailing edge of the text field per `docs/design-language.md` so typing direction and LTR text flow remain natural).
- **What does NOT mirror:**
  - **Unfolded two-pane layout:** List always stays on the left (left thumb) and Detail on the right (right thumb), matching desktop and web mental models.
  - **Task row columns & swipe semantics:** Checkbox stays on the left, Star on the right (both thumbs hit them effortlessly when two-handed, and swipe gestures make both actions reachable one-handed from anywhere on the row). Swipe right is always Complete/Reopen; swipe left is always My Day—keeping muscle memory identical across postures and handedness.

---

## 3. Scroll, collapse & reachability behaviour

> **Header visual redesign:** See [`docs/android-header.md`](android-header.md) for the updated **Living Daybreak Horizon** room header specification (unified full-bleed sky gradient, `72 dp` / `80 dp` 3-hill landscape, stacked summary + glanceable urgency cue, scene-expanding parallax pull-down reach, and the `2 dp` compact progress hairline).

### 3.1 Scroll-linked header collapse (`HeaderCollapse`)

- **At rest (top of list):** Full non-interactive Daybreak sky header below `statusBarsPadding()` — eyebrow (`EYEBROW_DAY_PHONE` + `EYEBROW_META_PHONE`), room title (`ROOM_TITLE_PHONE`, 33/36), progress summary (`3 of 12 done · 9 open`), and the three-hill panorama (`HEADER_SCENE_COVER` 72 dp / `HEADER_SCENE_INNER` 80 dp, or `HEIGHT_FINISHED` 164 dp on finished My Day).
- **Scrolling up (reading down the list):** The sky body and landscape collapse **1:1 with scroll offset** (`NestedScrollConnection.onPreScroll`) into a sleek **`36 dp` glanceable title bar** (`phone::COMPACT_HEADER`, down from the old `48 dp` app bar because it holds zero touch targets!).
  - Inside the `36 dp` compact header (over `sky` background with a `1 dp` bottom `line` hairline):
    - Leading (`start = S5` 20 dp): Room title in `ROOM_TITLE_BAR` (18/20, `ink`), fading in over `collapse.fraction` `0.5..1.0`.
    - Trailing (`end = S5` 20 dp): Overdue mark when needed, an `8 dp` sun ring or disc, compact progress figure (`3/12`), and quiet sync status. A `2 dp` progress line runs above the bottom hairline.
  - Releasing midway settles to expanded or collapsed over `PANEL` (`200 ms`, `STANDARD`; snaps under reduced motion).
- **Keyboard open or short window (`maxHeight < 480 dp`):** Header forces collapsed (`forced = 1f`, `PANEL` 200 ms). The Bottom Utility Bar hides while the keyboard is open so the Quick-Add Composer (or Filter Field) docks directly above the IME and at least **4 full task rows** (`4 × 64 dp = 256 dp`) remain visible above it. If even the `36 dp` compact bar would leave fewer than 4 rows, it collapses away as before (`RoomPane.kt`).

### 3.2 Pull-down reachability expansion (cover posture)

When a list has few tasks (e.g. 1–4 open tasks), the top row sits near `y ≈ 160 dp`—still high for a one-handed thumb on a `960 dp` cover screen.

- **Gesture:** When the task list is already at index 0, offset 0, pulling downward expands the Daybreak sky smoothly **1:1** up to `phone::HEADER_REACH_EXPAND` (`180 dp`, clamped to `28%` of viewport height).
- **Visuals:** Instead of stretching a flat empty sky above the text, pull-down expands the Daybreak sky and landscape panorama smoothly (`0.25 × reach` upper sky drift above the text, `0.75 × reach` growing the 3-hill landscape, lifting the sun arc, and revealing high-altitude stars/clouds with parallax). See [`docs/android-header.md`](android-header.md) §4.1 for exact measurements and reduced-motion fallbacks.
- **Latch & release:** Releasing past `96 dp` (`53%` of expansion) gently latches the expanded sky (`PANEL`, `STANDARD`) until the user taps a task, starts scrolling upward, opens a sheet, or taps the sky area—whereupon it settles back to normal height over `PANEL` (`200 ms`, `STANDARD`). Under reduced motion, pull-down still tracks the finger 1:1 and snaps on release.
- **Unfolded posture:** Pull-down reachability is disabled (not needed with a two-hand side grip).

---

## 4. Screen-by-screen & flow-by-flow design

### 4.1 Room view (My Day, Smart Lists, Collections, Lists)

#### Cover screen (`412 × 960 dp`)

```
┌─────────────────────────────────────────┐  y = 0
│  [Status bar inset ~40 dp]              │
│  ┌───────────────────────────────────┐  │
│  │ Saturday  21 NOV 2026 | W47       │  │  GLANCEABLE SKY HEADER
│  │ My Day          3 of 12 done · 9  │  │  (Non-interactive; collapses
│  │ ~~~~~☀~~~~~/\~~~~/\~~~~~~~~~~~~~~ │  │   to 36 dp strip on scroll)
│  └───────────────────────────────────┘  │  y ≈ 156 dp
│  ○  Pay quarterly VAT filing    !!!  ★  │
│     ◆ 2d overdue  • Finance             │
│  ─────────────────────────────────────  │
│  ○  Send venue booking confirmation  ☆  │  SCROLLABLE TASK LIST
│     in 1h 25m     • Events  ✉          │  (~620–680 dp viewport:
│  ─────────────────────────────────────  │   ~10 full 64 dp rows!)
│  ... (rows 3–9) ...                     │
│  › COMPLETED 3                          │
│  ─────────────────────────────────────  │  y ≈ 796 dp
│  [Optional: Error / Signed-out Banner]  │  ← Thumb-reachable banner
│  [Optional: 56 dp Undo Toast Strip]     │  ← Never covers a task row
│  ┌───────────────────────────────────┐  │  y ≈ 824 dp
│  │ + Add a task…             📅   ⚑ │  │  UPPER DOCK: Composer (48 dp)
│  └───────────────────────────────────┘  │  (Swaps to Filter when active)
│  ─────────────────────────────────────  │  y ≈ 884 dp
│  [≡] [☀ My Day][• Important][• Plann]🔍⋯│  LOWER DOCK: Bottom Bar (52 dp)
│          [Gesture nav inset ~24 dp]     │  y = 960 dp
└─────────────────────────────────────────┘
```

- **Top Sky Header:** Non-interactive. Uses one full-bleed gradient, stacked eyebrow, title and summary/cue, and a `72 dp` panorama. When offline or pending sync, a quiet `SyncChip` may appear at the trailing edge of the eyebrow for glanceability, while its interactive nudge lives in the Room Switcher footer and Bottom Bar dot.
- **Banners (Failed change / Signed out):** Relocated from top-of-list to **directly above the Bottom Dock** (above `ToastStrip` if both are present), so **Dismiss** and **Pair again** sit squarely in the Comfortable thumb zone.
- **Undo Toast Strip (`phone::TOAST_STRIP` 56 dp):** Unchanged visual design (`44 dp` `toast` card, `10 dp` radius, `2 dp` linear `UNDO_WINDOW` countdown bar), docked directly above the Upper Dock so it never covers any task row.
- **Upper Dock — Quick-Add Composer (`56 dp` slot: `4 dp` top + `48 dp` `COMPOSER` + `4 dp` bottom, `pane` bg, `1 dp` top `line` hairline):**
  - Full-width `raised` field (`radius::COMPOSER` 12 dp, `1 dp` `control` border, `accent` + `3 dp` `tint` focus ring when focused).
  - Leading `+` (`accent-strong`), input (`+ Add a task…`), trailing `Calendar` (`48 dp`) and `Flag` (`48 dp`) buttons.
  - When date or priority chips are active, the second `48 dp` chip row animates open inside the composer card (`OVERLAY_IN`/`OVERLAY_OUT`).
- **Lower Dock — Bottom Utility & Room Bar (`phone::BOTTOM_BAR` 52 dp, `canvas` bg, `1 dp` top `line` hairline, plus `navigationBarsPadding()`):**
  - **Rooms button (`48 dp`, `menu` icon):** Opens the **Bottom Room Switcher Sheet**. Displays a `6 dp` status dot on its upper-right corner *only* when offline (`ink-4`), syncing pending writes (`accent`), or sync error (`danger`).
  - **Sibling Room Pills (`weight(1f)`, horizontally scrollable `LazyRow`):** Renders all sibling rooms (`My Day`, `Important`, `Planned`, `All`, `Completed`, `Agent activity` for smart lists; sibling collections for a collection; sibling lists for a list). Each pill is `32 dp` tall (`radius::ROOM_PILL` 16 dp) inside a `48 dp` touch target, using `ListColors` (`strong`/`onStrong` when active, `tint`/`ink` when inactive). Automatically scrolls the active pill into view (`PANEL`, `STANDARD`). Replaces the old top `+N ▾` truncation—every sibling is a thumb-flick away, and `[≡]` sits right beside the pills for the full hierarchy.
  - **Filter button (`48 dp`, `search` icon):** Toggles the Bottom Filter Field in the Upper Dock slot; shows `wash` pill highlight + `accent-strong` icon when active (`expanded = true`).
  - **Room menu button (`48 dp`, `ellipsis` icon):** Opens the **Room Actions Bottom Sheet**; omitted in Completed, Agent activity, and Trash (exact parity with `roomMenu`).
  - **Swipe-up gesture on Lower Dock:** Dragging upward from the `52 dp` Bottom Bar tracks 1:1 to pull open the **Bottom Room Switcher Sheet** (`[≡]` tap is the visible accessible alternative).
  - **Keyboard open:** The `52 dp` Lower Dock animates away (`PANEL`, `EXIT`) so the Composer (or Filter Field) rests cleanly on top of the IME.

#### Unfolded inner screen (`876 × 970 dp`)

- **Two-pane split:** Left pane (`List`, `438 dp` or `phone::INNER_LIST` 450 dp capped to `50%` of width, respecting `FoldingFeature` hinge bounds if a physical hinge separation exists) + `1 dp` `line` vertical divider + Right pane (`Detail`, `weight(1f)`).
- **Left Pane (`List`, left-thumb domain):** Uses the **exact same layout as Cover** — non-interactive Daybreak sky header (`80 dp` panorama) at top, scrollable task list in the middle, and the full 2-row **Bottom Thumb Dock** (Composer + `[≡]` Room Pills `[🔍]` `[⋯]`) at bottom-left!
  - Left thumb naturally reaches `[≡ Rooms]`, room pills, checkboxes, quick-add input, and left-docked sheets.
- **Right Pane (`Detail`, right-thumb domain):**
  - Top: `40 dp` non-interactive glanceable breadcrumb strip (`[• Personal] / Admin`).
  - Middle: Scrollable Task Detail body.
  - Bottom: `52 dp` **Detail Bottom Action Bar** (aligned to the exact baseline of the Left Pane's Bottom Utility Bar!) holding `[○/✓ Complete]` on the inner edge and `[☀ My Day]` `[★ Important]` `[⋯ Task actions]` `[× Close]` clustered at the bottom-right directly under the **right thumb**.
  - When no task is selected (`NoDetail`): calm centred message (`"Pick a task to see it here."`) or, during bulk selection, selection summary in the lower third.

---

### 4.2 Bottom-anchored Room Switcher Sheet (replacing the side drawer)

Replaces `ModalNavigationDrawer` completely on Android.

#### Structure (bottom-to-top, closest to thumb first)

```
┌─────────────────────────────────────────┐
│  [Dimmed sky & top rows via scrim]      │  ~28% visible at default open
│╭───────────────────────────────────────╮│  y ≈ 268 dp (72% default height)
││                ━━━━━                  ││  Drag handle (32 × 4 dp)
││  ┌─────────────────────────────────┐  ││
││  │ 💼 Work                   5   ▾ │  ││  SCROLLABLE ZONE (weight = 1f)
││  │     🪙 Finance                1 │  ││  • Bottom gravity (Arrangement.Bottom)
││  │     📅 Events                 2 │  ││    when shorter than viewport
││  │     ⚑  Planning               2 │  ││  • Auto-scrolls active list into
││  │ 🏠 Personal               9   ▾ │  ││    the lower third on open
││  │     🧺 Errands                2 │  ││  • Preserves natural top-to-bottom
││  │     📄 Admin                  1 │  ││    user order within collections
││  │  ...                            │  ││
││  │ ─────────────────────────────── │  ││
││  │ 🕒 Activity                     │  ││  Activity + active agents today
││  │     (C) Claude   3 edited today │  ││  + Trash right above Smart Grid
││  │     (X) Codex    2 edited today │  ││
││  │     (π) Pi       2 edited today │  ││
││  │ 🗑 Trash                        │  ││
││  └─────────────────────────────────┘  ││
│├───────────────────────────────────────┤│  1 dp hairline
││ [☀ My Day  9] [★ Impor… 3] [📅 Plan 15]│  PINNED SMART ROOMS GRID (2 × 3)
││ [☰ All    16] [✓ Done    ] [✦ Agent  7]│  Each tile 48 dp tall, radius::MD
││ ┌───────────────────────────────────┐ ││
││ │ 🔍 Find a room…                   │ ││  PINNED QUICK-FILTER (44/48 dp)
││ └───────────────────────────────────┘ ││
│├───────────────────────────────────────┤│  1 dp hairline
││  + New         • 20:18            ⚙  ││  PINNED FOOTER (52 dp)
││         [Gesture nav inset]           ││
│╰───────────────────────────────────────╯│
└─────────────────────────────────────────┘
```

- **Surface & geometry:** Opaque `canvas` background, `radius::XL` (`14 dp`) top corners, `1 dp` `line-strong` top border in dark mode, `elevation::scrim` backdrop (no blur). Drag handle `32 × 4 dp` (`phone::SHEET_HANDLE_WIDTH × space::S1`) in `line-strong`, centred with `8 dp` top/bottom breathing room.
- **Heights:** Opens at `phone::SHEET_DEFAULT_FRACTION` (`0.72` of viewport height, ~690 dp on Fold7 cover). Draggable up to `0.92` (`phone::SHEET_MAX_FRACTION`) or down to dismiss.
- **Unfolded placement:** Anchors to the **bottom-left** matching `phone::INNER_LIST` (`438–450 dp`), rising directly from the Left Pane's Bottom Dock under the left thumb while the Right Pane stays visible behind the scrim.
- **Pinned Footer (`52 dp`, exact parity with desktop/web):**
  - `+ New` (`48 dp` target): Opens compact upward popover directly above `+ New` offering **List** (`plus`) and **Collection** (`folder-plus`). Choosing **List** reveals the inline `NewList` text field directly above the footer (riding `imePadding()` above the keyboard); choosing **Collection** opens the bottom-anchored `RenameCard("New collection")` sheet.
  - Centre **Sync status** (`6 dp` coloured dot + short mono label `20:18`, `Syncing…`, `Offline · 3`): tapping nudges sync immediately (`actions::nudge`)!
  - `⚙ Settings` (`48 dp` target): Opens the **Settings Bottom Sheet** in place (`CONTENT_SWAP`, with a back chevron to return to the switcher or drag-down to dismiss).
- **Pinned Quick-Filter (`48 dp` target, `44 dp` visual `raised` field):** `"Find a room…"`. Typing immediately filters smart lists, collections, lists, Activity, and Trash by name (case- and accent-insensitive). While focused with the keyboard open, the 2×3 Smart Rooms Grid collapses cleanly so matching rows sit directly above the input and keyboard!
- **Pinned Smart Rooms 2×3 Grid (`2 × 48 dp` tiles, `space::S2` 8 dp gap, `radius::MD` 6 dp):**
  - Row 1: **My Day**, **Important**, **Planned**
  - Row 2: **All**, **Completed**, **Agent activity**
  - Each tile shows the room's `ListColors` icon/glyph (`16 dp`), compact label (`NAV_SUB`), and trailing mono open count (`COUNT`, hidden when 0 or Completed). Active tile takes `wash` fill, `3 dp` leading `accent-strong` selection bar, and `BUTTON` weight label.
- **Scrollable Collections, Lists, Activity & Trash (`weight(1f)`):**
  - Preserves exact natural top-to-bottom order for collections (with collapsible `48 dp` chevron targets), their lists, and loose lists, followed by a hairline and **Activity** + today's active agent rows (`Claude`, `Codex`, `Pi` with `20 dp` letter marks and `"3 edited today"` status) + **Trash**.
  - Uses **bottom gravity (`Arrangement.Bottom`)** when content fits without scrolling, and on open scrolls automatically so the **currently active list** rests in the lower third of the scrollable area—immediately above the Smart Rooms Grid.
  - **Long-press (or trailing swipe / TalkBack action)** on any list or collection opens its **List/Collection Actions Bottom Sheet** (`Rename`, `Colour…`, `Move to…`, `Move up`, `Move down`, `Delete list…` / `Delete collection…`) instead of a small anchored popup that could appear near the top.
- **Motion:** Opens/closes over `PANEL` (`200 ms`, `ENTER` / `EXIT`); drag tracks 1:1.

---

### 4.3 Filter / Search

- **Trigger:** Tap `[🔍]` in the Bottom Utility Bar (either posture).
- **Placement:** Smoothly swaps (`CONTENT_SWAP`, `120 ms`) the **Upper Dock** slot (normally the Quick-Add Composer) into the **Bottom Filter Field** (`48 dp` min height, `wash` fill, `accent-strong` border while focused, `radius::MD` 6 dp, placeholder `"Filter: words, or tag:docs"`, trailing `48 dp` `[×]` clear button).
- **Why here:** The input sits directly on top of the keyboard (zero vertical jump when keyboard opens/closes) and right under both thumbs!
- **Active filter when keyboard is closed:** If `state.filter` is non-empty and the user dismisses the keyboard, the Bottom Filter Field remains visible in the Upper Dock (showing the active query and `[×]` clear button) alongside a compact `48 dp` `[+]` button on its leading side if the user still wants to jump to Quick-Add without clearing the filter. Opening any room (even the current one) clears the filter and restores the Composer, matching `docs/android.md`.
- **Unfolded:** Filter field swaps inside the **Left Pane's** Upper Dock only, keeping the Right Pane untouched.

---

### 4.4 Room menu (`⋯`)

- **Trigger:** Tap `[⋯]` in the Bottom Utility Bar.
- **Presentation:** **Bottom Action Sheet** (`radius::XL` 14 dp top corners, `raised` fill, drag handle, optional room title header in `CAPS_LABEL`).
- **Items (unchanged `roomMenu` parity):**
  - *List:* `Rename list`, `Colour…`, `Move to…`, `Move up`, `Move down`, `Show/Hide completed`, separator, `Delete list…` (`dangerInk`).
  - *Collection:* `Rename collection`, `Colour…`, `Move up`, `Move down`, `Show/Hide completed`, separator, `Delete collection…`.
  - *Smart lists (My Day, Important, Planned, All):* `Show/Hide completed`.
  - *Completed, Agent activity, Trash:* No menu (no `⋯` button shown).
- **Sub-flows:** Choosing `Colour…` or `Move to…` transitions **in place** inside the same bottom sheet (`CONTENT_SWAP`, `120 ms`) to `SwatchSheet` or `MoveListSheet`. Choosing `Rename` or `Delete…` transitions to the bottom-anchored `RenameCard` (riding `imePadding()`) or `DeleteListCard`.
- **Unfolded:** Docks to the bottom of the **Left Pane** (`width = INNER_LIST`).

---

### 4.5 Quick-add & its Calendar / Priority pickers

- **Composer (`QuickAdd.kt`):** Remains anchored at the bottom (Upper Dock). Keeps the exact shared syntax parsing (`tomorrow`, `zítra`, `+3d`, `!!`, `!!!`, `\escape`), removable preview chips (`Sun 22 Nov ×`, `!! ×`) in a second `48 dp` touch row inside the composer card, and `Calendar` (`📅`) + `Flag` (`⚑`) trailing buttons.
- **Thumb-first Quick-Add Calendar Popover (`📅`):**
  - Opens directly **above** the composer card (`overlay-shadow`, `radius::LG` 10 dp, `raised` surface, width `min(paneWidth - 24 dp, 388 dp)`).
  - **Key thumb-zone improvement — Inverted vertical hierarchy:**
    - Top: Month navigation (`< November 2026 >`, `48 dp` arrows) + Monday-first weekday labels + `7 × 5/6` day grid (`48 dp` min cell targets; today outlined in `control`, selected filled in `wash` with `accent-strong` bold date).
    - `1 dp` `line` hairline.
    - **Bottom (closest to thumb, right above the Calendar button!):** The 2×2 **Quick Presets** (`Today`, `Tomorrow` / `Next week`, `No date`, each `48 dp` tall, current preset tinted with `wash` and trailing `✓`).
  - Result: Tapping `📅` and choosing `Tomorrow` requires less than `24 dp` of thumb travel!
- **Quick-Add Priority Popover (`⚑`):**
  - Opens upward directly above the Flag button (`240 dp` wide, `48 dp` rows, highest first: `High !!!`, `Normal !!`, `Low !`, `None`, current ticked). Because the Flag button is already at the bottom-right corner, every priority row sits strictly inside the Comfortable thumb zone (`y ≈ 640..840 dp`).
- **Unfolded:** Both popovers anchor upward above the **Left Pane's** composer, never crossing the hinge.

---

### 4.6 Task rows, swipe gestures & drag-and-drop

- **Row geometry (`Rows.kt`):** Preserves `phone::ROW` (`64 dp` min, grows with font scale), `22 | 1fr | 20 | 24` columns + `48 dp` Important star target, `100 dp` due slot on dated rows, flush chips on undated rows, and the exact completion choreography (`CHECK_FILL` → `CHECK_TICK` → `COMPLETE_HOLD` → `ROW_COLLAPSE` → `SUN_TRAVEL`).
- **Row swipe gestures (new thumb-zone ergonomic accelerator):**
  - **Swipe Right (`dx ≥ phone::SWIPE_THRESHOLD` = 72 dp):** **Complete / Reopen** the task. Reveals `wash` strip from leading edge with `circle-check` (or `undo-2` on completed rows) in `accent-strong`, triggers `HapticFeedbackType.Confirm` upon crossing threshold, snaps back (`SWIPE_SETTLE` 160 ms), and plays the standard completion choreography + Undo toast. Makes completing effortless one-handed with a right thumb without stretching across to the left checkbox.
  - **Swipe Left (`dx ≤ -72 dp`):** **Add to / Remove from My Day**. Reveals warm `wash` strip from trailing edge with `sun-filled` / `sun` in `sun-ink`, triggers subtle tick haptic on crossing threshold, toggles My Day, and shows the Undo toast.
  - **Edge dead-zone guard (`phone::EDGE_GESTURE_INSET` = 24 dp):** Horizontal drags originating within `24 dp` of the left or right screen edge are ignored by row swipe handlers so Android system edge-back is never intercepted.
  - **Disabled during bulk selection and drag:** Swipe gestures are inactive when `state.bulk != null` or a drag is in progress.
  - **Visible & accessible alternatives:** Checkbox tap (row + detail bottom bar), Star tap (row + detail bottom bar), My Day toggle (detail bottom bar + task actions sheet + bulk bar), and TalkBack `customActions` (`"Complete"`, `"Add to My Day"`, `"Select task"`).
- **Long-press & Drag-and-drop (`TaskDrag.kt`):**
  - Long-press enters multi-selection and displays the `48 dp` 6-dot `DragGrip` on selected rows (exact existing behaviour).
  - **Thumb-zone upgrade for cross-list drag:** Instead of dragging to the **left screen edge** (which conflicts with Android's system back gesture and requires reaching across the screen):
    1. Dragging a lifted selection **down onto any visible Room Pill** in the Bottom Utility Bar highlights that pill (`selection-wash` + `accent-strong` outline) and drops the tasks directly into that list (or adds them to **My Day**)!
    2. Dragging **onto the `[≡ Rooms]` button** (or dwelling `250 ms` over the Bottom Utility Bar's `≡` target) slides open the **Bottom Room Switcher Sheet**, where the user can drop onto any list or My Day with automatic top/bottom edge-scrolling inside the sheet.
    3. Releasing anywhere invalid cancels cleanly.
  - **Accessible alternative:** Task Actions (`Move up`, `Move down`, `Move to…`) and Bulk Bar (`Move`, `My Day`).

---

### 4.7 Multi-selection & Bottom Bulk Dock

Replaces the split top-header + bottom-tools pattern (`BulkSelectionHeader` at top + `BulkSelectionTools` at bottom) with one unified **Bottom Bulk Dock**.

- **Top Sky Header:** Remains calm and non-interactive (no `[×]` button at the top!). Optional subtle `selection-wash` tint on the `36 dp` compact header if scrolled.
- **Selected rows:** Unchanged (`selection-wash`, `3 dp` `accent-strong` leading bar, `selection-check`, `ink-2` metadata, `DragGrip`).
- **Bottom Bulk Dock (replaces Composer + Utility Bar in place over `CONTENT_SWAP` 120 ms):**
  - **Row 1 — Selection Status Strip (`phone::BULK_STATUS_BAR` 44 dp, `wash` bg, `1 dp` top `line` hairline):**
    - Leading (`48 dp` target): `[×]` (`Clear selection`, `accent-strong`).
    - Middle (`weight(1f)`): **`3 tasks selected`** (`ROOM_TITLE_BAR`, `accent-strong`, `liveRegion = Polite`), or `"1 task selected · tap more or drag"` when 1 task is selected.
    - *(Left-handed cover mirror: `[×]` moves to the trailing edge.)*
  - **Row 2 — Bulk Tools Strip (`56 dp` min, `wash` bg, `1 dp` top `line` hairline, above `navigationBarsPadding()`):**
    - Horizontally scrollable centred row of `48 dp+` icon+label buttons (`Complete`/`Reopen`, `Star`, `My Day`, `Move`, `Due`, `Delete` in `dangerInk`).
    - `Move` opens `MoveTaskSheet` (bottom sheet); `Due` opens `DueDateSheet` (bottom sheet).
- **Unfolded:** Bulk Dock anchors to the bottom of the **Left Pane**. The **Right Pane** (`NoDetail`) displays the selection count (`"3 tasks selected"`) and optional quick action buttons in its **lower third** (under the right thumb) so two-thumb bulk triage feels effortless.

---

### 4.8 Task Detail & all its editors

#### Cover screen (`412 × 960 dp`, standalone)

```
┌─────────────────────────────────────────┐  y = 0
│  [Status bar inset ~40 dp]              │
│  [• Personal] / Admin                   │  GLANCEABLE BREADCRUMB (40 dp,
│  ─────────────────────────────────────  │  non-interactive; zero top buttons!)
│  ○  Renew passport appointment          │
│     Created 12 Nov by Pi / Edited 09:50 │
│                                         │
│  STEPS                            1/4   │  SCROLLABLE DETAIL BODY
│  ✓  Check which documents…     01  ⋯   │  (Steps, Notes, Fields, Sources,
│  ○  Book a slot on town hall…  02  ⋯   │   Attachments, Activity)
│  ○  Get a passport photo       03  ⋯   │
│  ○  Fill in application form   04  ⋯   │
│  +  Add step                            │
│                                         │
│  NOTES                                  │
│  Bring the old passport and proof of    │
│  address. Slots open Mondays at 08:00.  │
│                                         │
│  📅 Due         Sun 22 Nov           ›  │  ← Property rows land naturally
│  ⚑  Priority    Normal               ›  │    in the lower Comfortable zone!
│  🔔 Reminder    Today, 18:00         ›  │
│  🔁 Repeat      Does not repeat      ›  │
│  #  Tags        • #admin             ›  │
│  ... (Source, Add link, Attachments,    │
│       Activity)                         │
│  ─────────────────────────────────────  │
│  [Optional: 56 dp Undo Toast Strip]     │
│  [←]              [○] [☀] [★] [⋯]      │  PINNED DETAIL BOTTOM BAR (52 dp)
│          [Gesture nav inset ~24 dp]     │  y = 960 dp
└─────────────────────────────────────────┘
```

- **Top Glanceable Breadcrumb Bar (`40 dp`, `pane` bg, `1 dp` bottom `line` hairline, below `statusBarsPadding()`):**
  - Displays `[ListChip(collection)] / List name` only. Zero interactive buttons in the top bar.
- **Pinned Detail Bottom Bar (`phone::BOTTOM_BAR` 52 dp, `canvas` bg, `1 dp` top `line` hairline, above `navigationBarsPadding()`):**
  - **Cover (default Right-handed):**
    - Leading (`48 dp`): `[← Back]` (`chevron-left` — paired with Android predictive edge-back).
    - Trailing cluster (`spacedBy(space::S1)` 4 dp, each `48 dp`):
      - `[○ / ✓ Complete]` (mirrors the task's checkbox state—allows completing or reopening without reaching the top title!).
      - `[☀ My Day]` (`TaskToggle.MY_DAY` — outline `ink-3` off, filled `sun-ink` disc on, `POP` fill).
      - `[★ Important]` (`TaskToggle.IMPORTANT` — outline `ink-3` off, filled `important-fill` gold on).
      - `[⋯ Task actions]` (opens **Task Actions Bottom Sheet**).
  - **Cover (`Left-handed`):** Mirrors to `[⋯]` `[★]` `[☀]` `[○/✓]` on the left, `[← Back]` on the right.
  - **Unfolded Right Pane (`Detail`, always right-thumb optimised):**
    - Leading (inner edge, `48 dp`): `[○ / ✓ Complete]`.
    - Trailing cluster (bottom-right under right thumb, `spacedBy(4 dp)`): `[☀ My Day]` `[★ Important]` `[⋯ Task actions]` `[× Close]`.
  - **Keyboard open:** Detail Bottom Bar hides smoothly (`PANEL`) while editing title, notes, or adding/renaming a step so inline editors (`Save notes` / `Cancel`) sit directly above the keyboard.
- **All Task Detail Editors (every single one bottom-anchored!):**
  - **Due date (`DueDateSheet`):** Bottom sheet (`radius::XL` 14 dp top corners, `raised`). Top compact header (`Due` + formatted date + keyboard mode icon), **Quick Presets strip** (`Today`, `Tomorrow`, `Next week` — `48 dp` chips), Material month calendar grid (in `todo-design` colours/type), repeat warning note if repeating, pinned bottom buttons (`Clear` | `Cancel` | `Save`, `48 dp` each). In short windows (`< 480 dp`), drops headline and scrolls calendar above pinned buttons.
  - **Priority (`PrioritySheet`):** Compact bottom sheet (`High !!!`, `Normal !!`, `Low !`, `None`, `48 dp` rows, current ticked with `wash` + `✓`).
  - **Reminder (`ReminderSheet`):** Bottom sheet. Month calendar above, **24-hour Time Input** positioned directly below the calendar (closest to the thumb!) with validation / DST clock-change notes directly above the pinned bottom buttons (`Remove` | `Cancel` | `Save`). In short windows, Time Input comes first above scrollable calendar and pinned buttons. Permission rationale (`ReminderAccessCard`) renders as a bottom-anchored dialog card (`Alignment.BottomCenter`, `12 dp` inset).
  - **Repeat (`RepeatSheet`):** Bottom sheet (`Daily`, `Weekdays`, `Weekly`, `Monthly`, `Yearly`, `Custom…`, `Don't repeat`, undated repeat note, `Cancel`). Choosing `Custom…` transitions in-place (`CONTENT_SWAP`) to interval input, unit segments (`Day`/`Week`/`Month`/`Year`), weekday toggles (`Mo..Su` `48 dp` segments), live preview text, and pinned `Apply repeat` / `Cancel` buttons at bottom.
  - **Tags (`TagsSheet`):** Bottom sheet with **bottom-anchored filter/create input** (`"Type to find or create a tag"`), so matching tags and `"Create tag …"` sit directly above the input and keyboard! Trailing chevron on each tag transitions in-place (`CONTENT_SWAP`) to tag options (`Add/Remove`, `Rename tag`, `Colour…`, `Delete tag…`) and sub-pages (`Rename`, `SwatchSheet`, `ConfirmOrganize`) inside the same bottom sheet!
  - **Task Actions (`TaskActionsSheet`):** Bottom sheet offering Complete/Reopen, My Day, Important, Priority…, Move to…, Merge into…, Move up/down (in manual lists), separator, Delete task (`dangerInk`).
    - **Move to… (`MoveTaskSheet`):** Bottom sheet with scrollable list groups above, **filter input pinned near the bottom** above `Cancel` (rides `imePadding()` above keyboard).
    - **Merge into… (`MergeTaskSheet`):** Bottom sheet with scrollable open-task candidates above, **filter input pinned near the bottom** above `Cancel`; selecting a target transitions in-place (`CONTENT_SWAP`) to the confirmation view (`Cancel` focused first, `Merge tasks` primary button below).
  - **Steps (`StepActionsSheet`):** Tapping a step's `⋯` opens a compact bottom sheet (`Rename`, `Move up`, `Move down`, separator, `Delete step`). Tapping a step's title still edits inline with keyboard.
  - **Links & Attachments:** Source cards, `Add link…` (opens bottom-docked `RenameCard(webAddress = true)` above IME), attachment rows (`Open`, `Save`, `Remove` `48 dp` buttons), and `Add file` stay in the scrollable detail column; on tall screens they naturally sit in the lower Comfortable zone.
  - **Task Activity:** Read-only summary below Attachments, unchanged visual language.
  - **Unfolded sheet docking:** Every Detail sheet anchors to the **bottom of the Right Pane** (`width = rightPaneWidth`, `Alignment.BottomEnd`), staying strictly under the right thumb and never crossing the fold.

---

### 4.9 Activity & Trash rooms

- **Activity (`Activity.kt`):**
  - **Top:** Non-interactive Daybreak sky header in the `agent-activity` hue (no sun, eyebrow `History ALL CHANGES` or `History AGENT: CLAUDE`, title `Activity`), collapsing to `36 dp` compact header on scroll.
  - **Middle:** Day-grouped event feed with trailing `48 dp` `Undo` buttons.
  - **Bottom Dock:**
    - *(Optional `Refresh activity •` pill strip directly above Bottom Bar when new server events arrive — right in the thumb zone!)*
    - *(Reserved `56 dp` Undo Toast Strip when active)*
    - **Bottom Utility Bar (`52 dp`, `canvas`):** `[≡ Rooms]` on left, horizontally scrollable **Agent filter pills** (`All`, `C Claude`, `X Codex`, `π Pi`) in centre, and `[↻ Refresh]` (`48 dp`, disabled while refreshing) on right.
- **Trash (`Trash.kt`):**
  - **Top:** Non-interactive Daybreak sky header in `agent-activity` hue (`History DELETED ITEMS` / `Trash`, no sun), short explanation line below.
  - **Middle:** Deleted items list with trailing `48 dp` `Restore` buttons (or parent-restore hint).
  - **Bottom Dock:**
    - *(Reserved `56 dp` Undo Toast Strip when active)*
    - **Bottom Utility Bar (`52 dp`, `canvas`):** `[≡ Rooms]` on left, quiet count (`4 deleted items`) in centre, and `SyncChip` (only when offline/pending) on right.

---

### 4.10 Settings, Device Pairing, Pairing & Can't-Open screens

- **Settings (`SettingsSheet`, triggered from `⚙` in Room Switcher footer):**
  - Full bottom sheet (`radius::XL` 14 dp top corners, `raised`):
    - Header: `PAIRED WITH` (`CAPS_LABEL`) + `todo.example.com` (`DUE` mono), then the read-only push status and last wake (`NAV_SUB`).
    - `Open admin pages` (`external-link`)
    - `Sync now` / `Syncing…` (`refresh-cw`)
    - **Thumb-zone hand** (`Right` | `Left` two-segment toggle, `48 dp` height)
    - `Devices…` (`link`) — transitions in-place (`CONTENT_SWAP`) to Device Pairing (`Pair a new device` / `Approve a device`, preserving `FLAG_SECURE` when an invite QR is visible, with `Close` / `Cancel invite` / `Approve` pinned at the bottom!)
    - Hairline separator
    - `Unpair this phone` (`log-out`, `dangerInk`) — opens bottom-aligned `UnpairCard`.
- **Pairing (`PairingScreen.kt`) & Can't-Open (`CantOpenScreen`):**
  - **Top:** Glanceable Daybreak sky header (`Welcome` / `Pair this phone`, hills only, no sun).
  - **Middle (visual viewing zone):** Explanatory copy, and in `Waiting`, the crisp `224 dp` QR card + `WXYZ-2345` code + expiry timer—positioned comfortably for another device's camera to scan!
  - **Bottom (thumb action zone, bottom-weighted via `Arrangement.Bottom` inside scrollable column + `imePadding()`):**
    - `Entering`: Server address input, `Get a pairing code` primary button, `Or use an invite` input + `Use invite` + `Scan invite QR code` sit in the lower half of the screen.
    - `Waiting`: `[Open link]` + `[Copy link]` row and `[Cancel]` button pinned at the bottom.
    - `ConfirmInvite` / `SaveFailed` / `CantOpenScreen`: Status explanation above, primary & secondary buttons pinned at the bottom.

---

### 4.11 Overlay taxonomy (Sheets vs. Popovers vs. Dialogs)

| Overlay type | Used for | Anchor & geometry | Elevation & corners | Motion (`todo-design`) |
|---|---|---|---|---|
| **Bottom Sheet** (`BottomSheet`) | Room Switcher, Room Menu, Task Actions, Step Actions, List/Collection Menu, Settings, Devices, Due, Priority, Reminder, Repeat, Tags, Move Task, Merge Task, Swatches, Move List | Full width on Cover; pane-aligned (`INNER_LIST` or right pane width) on Unfolded. Drag handle `32 × 4 dp`. | Opaque `raised` (or `canvas` for Switcher), `radius::XL` (14 dp) top corners, `1 dp` `line-strong` top border in dark mode, `elevation::scrim`, **no blur**. | Slide up from bottom (`PANEL` 200 ms, `ENTER` / `EXIT`); drag tracks 1:1. Sub-pages swap in place (`CONTENT_SWAP` 120 ms). |
| **Upward Popover** (`AnchoredPopup`) | Quick-Add Calendar, Quick-Add Priority, Switcher `+ New` menu | Anchored directly above its bottom-dock trigger button (`4 dp` gap). | Opaque `raised`, `radius::LG` (10 dp), `shadow-overlay` + `1 dp` `line-strong` dark border. | `OVERLAY_IN` (150 ms, `ENTER`, scale `0.97→1`, `4 dp` upward shift) / `OVERLAY_OUT` (120 ms, `EXIT`). |
| **Bottom-Aligned Dialog Card** (`OverlayDialog`) | Delete list/collection/tag confirm, Unpair confirm, Reminder permission rationale, Rename / New Collection / Add Link | `Alignment.BottomCenter` (`padding = phone::DIALOG_INSET` 12 dp around sides & bottom + `navigationBarsPadding()` + `imePadding()`); `maxWidth = INNER_LIST` (450 dp). | Opaque `pane` (or `raised`), `radius::XL` (14 dp) all corners, `shadow-overlay`, `elevation::scrim`. | `OVERLAY_IN` (150 ms, `ENTER`, `DIALOG_RISE` 6 dp) / `OVERLAY_OUT` (120 ms, `EXIT`, `DIALOG_DROP` 4 dp). |
| **Undo Toast Strip** (`ToastStrip`) | All undoable completions, deletions, moves, merges, restores, swipe actions | Reserved `56 dp` strip directly above the Bottom Dock (or Detail Bottom Bar). Never overlaps rows. | Opaque `toast`, `radius::TOAST` (10 dp), `2 dp` linear `UNDO_WINDOW` countdown bar. | `TOAST_IN` (180 ms, `ENTER`) / `TOAST_OUT` (140 ms, `EXIT`). |

---

## 5. `todo-design` token mapping & proposed new tokens

### 5.1 Reused existing tokens

All existing tokens from `crates/todo-design/src/layout.rs` and `motion.rs` remain authoritative:

- **Spacing (`space::`):** `S1` (4), `S2` (8), `S3` (12), `S4` (16), `S5` (20), `S6` (24), `S8` (32), `S10` (40).
- **Radii (`radius::`):** `SM` (4), `MD` (6), `LG` (10), `XL` (14 — bottom sheets & bottom dialog cards), `CHIP` (10), `ROOM_PILL` (16), `TOAST` (10), `BUTTON` (6), `COMPOSER` (12), `CHECKBOX` (`circle`).
- **Phone metrics (`phone::`):** `ROW` (64), `ROW_PAD` (11, 12, 20), `COL_CHECK` (22), `COL_AGENT` (20), `COL_PRIORITY` (24), `DUE_SLOT` (100), `CHIP_MAX` (120), `TOUCH` (48), `NAV_ROW` (40), `ROOM_PILL` (32), `COMPOSER` (48), `TOAST_STRIP` (56), `TOAST` (44), `LANDSCAPE_COVER` (36), `LANDSCAPE_INNER` (40), `INNER_LIST` (450).
- **Elevation & motion:** `HAIRLINE` (1), `FOCUS_RING` (3), `SELECTION_BAR` (3), `scrim()`, `OVERLAY_LIGHT`, `OVERLAY_DARK`; `HOVER` (90), `CONTENT_SWAP` (120), `OVERLAY_OUT` (120), `OVERLAY_IN` (150), `TOAST_IN` (180), `TOAST_OUT` (140), `PANEL` (200), `ROOM_HUE` (360), `SUN_TRAVEL` (700), `DUSK` (1200), `REDUCED_FADE` (100), `UNDO_WINDOW` (6000).

### 5.2 Proposed new tokens

Add these 12 tokens to `crates/todo-design/src/layout.rs` (`pub mod phone`) and `crates/todo-design/src/motion.rs`, then regenerate `tokens.css` and `tokens.json` via `TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated`:

| Module | Rust constant | CSS / JSON key | Value | Purpose |
|---|---|---|---|---|
| `layout::phone` | `COMPACT_HEADER` | `--phone-compact-header` | `36.0` (dp) | Glanceable collapsed room header & non-interactive detail breadcrumb bar. |
| `layout::phone` | `BOTTOM_BAR` | `--phone-bottom-bar` | `52.0` (dp) | Height of the bottom utility/navigation bar and detail bottom action bar (`48 dp` `TOUCH` targets + `2 dp` top/bottom). |
| `layout::phone` | `BULK_STATUS_BAR` | `--phone-bulk-status-bar` | `44.0` (dp) | Selection count & Clear (`×`) strip directly above bulk tools in the Bottom Bulk Dock. |
| `layout::phone` | `HEADER_REACH_EXPAND` | `--phone-header-reach-expand` | `180.0` (dp) | Max extra Daybreak sky expansion on overscroll pull-down reachability (clamped to `28%` of viewport height). |
| `layout::phone` | `SWIPE_THRESHOLD` | `--phone-swipe-threshold` | `72.0` (dp) | Horizontal drag threshold to trigger task-row swipe right (Complete/Reopen) or swipe left (My Day). |
| `layout::phone` | `EDGE_GESTURE_INSET` | `--phone-edge-gesture-inset` | `24.0` (dp) | Left/right screen-edge dead zone where row swipes do not capture, protecting Android system edge-back. |
| `layout::phone` | `SHEET_HANDLE_WIDTH` | `--phone-sheet-handle-width` | `32.0` (dp) | Width of bottom sheet drag handle (`4 dp` tall = `space::S1`). |
| `layout::phone` | `SMART_TILE` | `--phone-smart-tile` | `48.0` (dp) | Height of each tile in the Room Switcher's pinned 2×3 Smart Rooms Grid (`TOUCH` height, `radius::MD`). |
| `layout::phone` | `DIALOG_INSET` | `--phone-dialog-inset` | `12.0` (dp) | Outer horizontal and bottom margin of bottom-anchored confirmation dialog cards (`space::S3`). |
| `layout::phone::FRACTIONS` | `SHEET_DEFAULT_FRACTION` | `--phone-sheet-default-fraction` | `0.72` | Default open height of the Bottom Room Switcher Sheet as a share of viewport height. |
| `layout::phone::FRACTIONS` | `SHEET_MAX_FRACTION` | `--phone-sheet-max-fraction` | `0.92` | Maximum expanded height of a tall bottom sheet (keeps status bar & sky sliver visible). |
| `motion` | `SWIPE_SETTLE` | `--duration-swipe-settle` | `160` (ms) | Task row snap-back after a cancelled or committed horizontal swipe (`STANDARD`; `0 ms` reduced motion). |

---

## 6. Visual mockups

Interactive HTML mockups (built directly on `crates/todo-design/generated/tokens.css`, bundled Geist / Geist Mono fonts, authentic project SVGs, and invented fixture data from `crates/todo-ffi/src/fixtures.rs`) and their 2× Playwright-rendered PNGs live in `docs/mockups/thumb-zone/`. Open `docs/mockups/thumb-zone/index.html` for the full gallery, or toggle `?theme=light|dark&zones=0|1` on any page:

| Screen | HTML mockup | Light | Dark | Thumb-zone heat map |
|---|---|---|---|---|
| **Cover — Room (My Day)** | `docs/mockups/thumb-zone/cover-room.html` | `cover-room-light.png` | `cover-room-dark.png` | `cover-room-zones-light.png` |
| **Cover — Bottom Room Switcher** | `docs/mockups/thumb-zone/cover-switcher.html` | `cover-switcher-light.png` | `cover-switcher-dark.png` | `cover-switcher-zones-light.png` |
| **Cover — Task Detail** | `docs/mockups/thumb-zone/cover-detail.html` | `cover-detail-light.png` | `cover-detail-dark.png` | `cover-detail-zones-light.png` |
| **Cover — Quick-Add + Calendar** | `docs/mockups/thumb-zone/cover-quick-add.html` | `cover-quick-add-light.png` | `cover-quick-add-dark.png` | `cover-quick-add-zones-light.png` |
| **Cover — Multi-Selection Bulk Dock** | `docs/mockups/thumb-zone/cover-bulk.html` | `cover-bulk-light.png` | `cover-bulk-dark.png` | `cover-bulk-zones-light.png` |
| **Unfolded — Personal + Detail** | `docs/mockups/thumb-zone/unfolded-workspace.html` | `unfolded-workspace-light.png` | `unfolded-workspace-dark.png` | `unfolded-workspace-zones-light.png`, `unfolded-workspace-zones-dark.png` |

Re-render all PNGs from the repo root after any HTML/CSS tweak:

```sh
cd apps/web/e2e && npm ci && cd ../../.. && node docs/mockups/render.mjs
```

---

## 7. Prioritized implementation plan

Implementation and verification checklist:

- [x] Phase 1: generated tokens, shared room lookup and calendar presets, bottom sheets and bottom dialogs.
- [x] Phase 2: read-only sky and cover reachability, thumb dock, room actions, and bulk sheets.
- [x] Phase 3: room switcher, pane/hinge alignment, list/collection actions, and the local hand preference.
- [x] Phase 4: detail action bar, pane-aligned editors, bottom unpair and secure pairing cards, and inverted quick-add presets.
- [x] Phase 5: row swipes and Undo, cross-list drag through the dock, history room bars, and bottom-weighted pairing.
- [x] Phase 6 automated: Fold7 cover and inner galleries in both themes, focused interaction tests, and screenshot verification.
- [ ] Phase 6 manual: software keyboard, system edge-back, and fold posture checks on a Fold7 or equivalent emulator.

### Phase 1 — Design tokens & shared primitives *(foundation, low risk)*

- **`crates/todo-design/src/layout.rs` & `motion.rs` & `export.rs`:** Add the 12 new tokens listed in §5.2; run `TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated` and verify `cargo test -p todo-design`, wasm32, and Android target checks.
- **`crates/todo-client` & `crates/todo-ffi` (what to share vs. keep Android-only):**
  - *Share in `todo-client` / `todo-ffi`:*
    - Add a pure helper for **room switcher quick-filter matching** (accent- and case-insensitive substring match over `Home` smart lists, collections, lists, Activity, Trash—reusing the same normalizer as `intent::merge` and `moveDestinations`).
    - Expose `quickAddPresets` (already in `todo_client::intent::calendar`) for `DueDateSheet`'s quick-preset chips so Detail Due and Quick-Add Calendar share identical `Today` / `Tomorrow` / `Next week` date computation.
  - *Keep Android-only:* Handedness preference (`PrefsKeyValues` / `SharedPreferences`), `FoldingFeature` hinge geometry, bottom sheet drag physics, and pull-down reachability overscroll.
- **`apps/android/app/src/main/kotlin/app/todoornot/ui/Overlay.kt`:**
  - Add `BottomSheet` composable (flat `todo-design` surface, `radius::XL`, drag handle, 1:1 vertical drag, `PANEL` settle, optional pane alignment for unfolded mode, `CONTENT_SWAP` in-place sub-page support).
  - Update `OverlayDialog` alignment from `Alignment.Center` to `Alignment.BottomCenter` with `phone::DIALOG_INSET` (`12 dp`) and `imePadding()`.

### Phase 2 — Room chrome, Bottom Thumb Dock & Filter *(highest daily impact)*

- **`Header.kt` & `Collapse.kt`:** Remove the top `48 dp` interactive app bar (`menu`, `search`, `ellipsis`) from `RoomHeader`. Collapse into the `36 dp` `COMPACT_HEADER` glanceable title strip with compact `done/total` count. Add overscroll pull-down reachability (`HEADER_REACH_EXPAND`, cover posture only).
- **`Pills.kt`:** Replace static `+N` `SubcomposeLayout` with a horizontally scrollable `LazyRow` of `32 dp` pills inside `48 dp` targets (`RoomPillsStrip`) that auto-scrolls the active pill into view.
- **`RoomPane.kt`:** Assemble the 2-row **Bottom Thumb Dock** (Upper Dock: `Composer` / swapped `FilterField`; Lower Dock: `BottomBar` with `[≡]`, `RoomPillsStrip`, `[🔍]`, `[⋯]`). Relocate `Banner` directly above the dock/toast strip. Hide Lower Dock while keyboard is open. Convert Room `⋯` `MenuPopup` to `BottomSheet`.
- **`BulkSelection.kt`:** Replace `BulkSelectionHeader` (top) with the 2-row **Bottom Bulk Dock** (`BulkStatusStrip` 44 dp + `BulkSelectionTools` 56 dp). Convert bulk Move and Due pickers to `BottomSheet`.

### Phase 3 — Bottom Room Switcher Sheet & Settings *(eliminates top-anchored drawer)*

- **`MainScreen.kt`:** Remove `ModalNavigationDrawer`; wire `BottomRoomSwitcherSheet` (cover full-width, unfolded left-pane `INNER_LIST` width), swipe-up gesture on the Bottom Utility Bar, and `FoldingFeature` hinge awareness (`androidx.window.layout.WindowInfoTracker`) for unfolded two-pane split.
- **`Drawer.kt` (refactor to `RoomSwitcher.kt`):** Build the three-zone bottom-up sheet: scrollable bottom-gravity Collections/Lists + Activity/Agents/Trash, pinned 2×3 Smart Rooms Grid (`SMART_TILE`), pinned Quick-Filter input (`"Find a room…"`), and pinned Footer (`+ New`, tappable Sync status, `⚙ Settings`). Convert list/collection long-press menus and `DrawerSettings` to `BottomSheet`s; add the `Thumb-zone hand` (`Right` / `Left`) toggle in `SettingsSheet`.

### Phase 4 — Task Detail bottom bar & bottom-sheet editors

- **`Detail.kt`:** Keep only the `40 dp` non-interactive breadcrumb at the top. Add the pinned `52 dp` **Detail Bottom Action Bar** (`[←]`, `[○/✓]`, `[☀]`, `[★]`, `[⋯]` on Cover; `[○/✓]` … `[☀]`, `[★]`, `[⋯]`, `[×]` on Unfolded right pane). Hide bar while keyboard is open. Convert step `⋯` menu to `BottomSheet`.
- **`Editing.kt`, `Reminders.kt`, `Organize.kt`, `TaskActions.kt`, `QuickAdd.kt`:**
  - `QuickAdd.kt`: Invert `QuickAddCalendar` vertical order so 2×2 presets sit at the bottom edge directly above `📅`.
  - `QuickAdd.kt`: Present the shared detail calendar as `DueDateSheet`, including the repeat warning, keyboard navigation, and the same bottom presets.
  - `Reminders.kt`: Convert `ReminderDialog` to `ReminderSheet` with Time Input placed directly above bottom action buttons.
  - `Organize.kt`: Convert `RepeatCard`, `MoveListCard`, `SwatchCard`, and `TagsDialog` to `BottomSheet`s with bottom-anchored filter/create input in `TagsSheet`.
  - `TaskActions.kt`: Convert `TaskActionsAnchor` menus, `MoveTaskCard`, and `MergeTaskDialog` to `BottomSheet`s with bottom-anchored filter inputs.
  - `Unpair.kt` & `DevicePairing.kt`: Dock `UnpairCard` and `DevicePairingDialog` to bottom-aligned sheet/card placement (`FLAG_SECURE` preserved).

### Phase 5 — Row swipes, bottom-dock task drag & secondary rooms

- **`Rows.kt` & `RoomPane.kt`:** Add `SwipeableTaskRow` wrapper (swipe right ≥ `72 dp` = Complete/Reopen; swipe left ≥ `72 dp` = My Day; `24 dp` edge dead-zone guard; `SWIPE_SETTLE` 160 ms; haptics; TalkBack custom actions).
- **`TaskDrag.kt`:** Replace left-edge drawer trigger with drop targets on bottom **Room Pills** and hover-over-`[≡]` to open the **Bottom Room Switcher Sheet**.
- **`Activity.kt` & `Trash.kt`:** Convert headers to non-interactive Daybreak Activity-hue sky headers and add the `52 dp` Bottom Utility Bar (with horizontal Agent filter pills + bottom Refresh in Activity).
- **`PairingScreen.kt`:** Keep Daybreak sky header at top and QR/explanation in upper-middle viewing zone; bottom-weight inputs and buttons (`Arrangement.Bottom` + `imePadding()`).

### Phase 6 — Paparazzi gallery & verification

Update `apps/android/app/src/test/kotlin/app/todoornot/GalleryTest.kt` and the relevant interaction tests:

1. **Add Galaxy Z Fold7 device configs** (keep `COVER_SIDEWAYS` for short-window checks):
   - `FOLD7_COVER`: `412 × 960 dp` (`824 × 1920 px` at `Density.XHIGH` 320 dpi, `21:9` aspect ratio).
   - `FOLD7_INNER`: `876 × 970 dp` (`1752 × 1940 px` at `Density.XHIGH` 320 dpi, near-square unfolded inner display).
2. **Update existing gallery fixtures** (light & dark on `FOLD7_COVER` and `FOLD7_INNER`):
   - `cover_my_day` (non-interactive sky + 2-row Bottom Thumb Dock)
   - `collapsed_header` (`36 dp` glanceable title bar)
   - `cover_filter` (Bottom Filter Field in Upper Dock)
   - `cover_drawer` → `cover_room_switcher` (Bottom-Anchored Room Switcher Sheet, expanded & folded collections, plus quick-filter active state)
   - `cover_detail` (glanceable top breadcrumb + pinned Detail Bottom Action Bar)
   - `inner_personal_with_detail` (two-pane `FOLD7_INNER` with aligned Left & Right bottom bars)
   - `quick_add_pickers` (inverted Quick-Add Calendar with presets at bottom)
   - `multi_selection` (2-row Bottom Bulk Dock)
   - `list_menu`, `task_actions`, `step_actions`, `settings_menu`, `due_date_picker`, `reminder_picker`, `organize_*` (bottom-sheet & bottom-dialog presentations)
   - `activity`, `trash`, `pairing_*` (bottom utility bars & bottom-weighted pairing controls)
3. **Add new gallery fixtures:**
   - `cover_reach_pull_light` (overscroll pull-down sky reachability state)
   - `cover_handedness_left_light` (mirrored Left-handed cover room & detail bottom bars)
   - `cover_row_swipe_light` / `dark` (task row swipe-right complete & swipe-left My Day reveal frames)
   - `inner_switcher_sheet_dark` (left-pane docked Room Switcher sheet on `FOLD7_INNER`)
   - `inner_detail_sheet_light` (right-pane docked Due/Actions bottom sheet on `FOLD7_INNER`)
4. **Manual emulator checks:** Verify software keyboard (`imePadding()`) hides the Lower Dock and keeps ≥ 4 rows visible on `FOLD7_COVER`, verify `24 dp` edge dead-zone does not fight system predictive back, and verify `New → List` and `Tags` bottom inputs stay clear of the IME in both portrait and sideways orientations.
