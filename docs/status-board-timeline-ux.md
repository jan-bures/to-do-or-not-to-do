# Status, Board & Timeline — UX Design Specification

This document specifies **workflow status**, **Board (kanban) view**, and **Timeline view** across all three clients — **GPUI desktop** (`apps/desktop`, visual reference), **Leptos web** (`apps/web`), and **Android Compose** (`apps/android`, following the strict **Sky above, hands below** thumb-zone architecture in [`docs/android-thumb-zone.md`](android-thumb-zone.md) and [`docs/android-header.md`](android-header.md)). Every visual rule conforms to the **Playful / Daybreak** language in [`docs/design-language.md`](design-language.md).

---

## 1. Recommended model & UX judgement

### 1.1 The short recommendation

1. **Per-list opt-in via two small, orthogonal switches — both OFF by default:**
   - **Workflow & Board** *(off by default)* — enables task statuses (`Backlog`, `To do`, `In progress`, `Done`) and unlocks **Board** view.
   - **Timeline & Start dates** *(off by default)* — adds optional **Start dates** (`start_date`) to tasks and unlocks **Timeline** view.
   - When at least one switch is on, a **Default view** selector (`List` | `Board` | `Timeline`) appears so a project list can open directly in its preferred view.
2. **Fixed 4-stage canonical workflow (`Backlog` · `To do` · `In progress` · `Done`), not custom per-list columns.**
3. **`Done` IS completed (`completed_at.is_some()`).** Only the open stage (`Backlog` | `To do` | `In progress`, defaulting to `To do`) is stored beside `completed_at`. There is never a split-brain state between status and completion.
4. **Timeline uses an optional `start_date: Option<Date>` paired with `due: Option<Date>` (`start_date <= due`), never a separate stored duration.** Tasks with only `due` render as 1-day milestones; tasks with neither sit in an **Unscheduled** tray.
5. **Non-destructive disabling:** Turning off Workflow or Timeline only hides its views and controls. Existing task `status` and `start_date` values remain preserved in storage so toggling back restores everything intact.

### 1.2 Why this model wins (and alternatives rejected)

A personal to-do app lives in two worlds at once: **70–80% of lists are light checklists** (*Groceries*, *Packing*, *Errands*, *Admin*) where Microsoft-To-Do-style calm speed is everything, while **20–30% are multi-step projects** (*Home renovation*, *Q4 Launch*, *Trip planning*) where tracking *what is actively in flight* and *how multi-day spans overlap* is genuinely transformative — especially when AI agents (Claude, Codex, Pi) collaborate on tasks.

| Alternative considered | Verdict | Why rejected |
|---|---|---|
| **Always on for every list** | Rejected | Imposes permanent chrome tax (view switchers, status marks, extra detail rows) on *Groceries* and *Errands*. Violates Playful's restraint ("colour means something, or it isn't there"). |
| **Per-collection (group) opt-in** | Rejected | Real collections mix checklists and projects: *Home* holds both **Groceries** (flat checklist) and **Home renovation** (board + timeline); *Work* holds both **Admin** (checklist) and **Q4 Launch** (project). |
| **One monolithic "Project mode" toggle** | Rejected | Bundles unrelated concepts. A *Reading* or *Bug triage* list wants Kanban statuses with zero timeline dates; an *Events* or *Travel* list wants a Timeline with zero Kanban columns. Two clean switches stay tiny while fitting every real list shape. |
| **Custom per-list statuses / column names in v1** | Rejected | Breaks cross-list queries (`status:in_progress`), forces AI agents to look up bespoke column UUIDs before acting, introduces foreign-key column tombstones and move-between-lists mapping dialogs, and invites Jira bloat. *(Revisit only if real usage demands hiding `Backlog` or aliasing labels; the canonical 4-stage enum stays forward-compatible.)* |
| **5th `Blocked` / `Waiting` column** | Rejected | Forces horizontal scrolling on a 1440 px desktop even with Detail closed, blurs workflow stage with a blocker condition already cleanly modelled by `#waiting` / `#blocked` tags across *all* lists, and slows keyboard status cycling. |
| **Decouple `Done` from `completed_at`** | Rejected | Creates two sources of truth ("checked off yet `In progress`", or "`Done` yet still counted in My Day open count"). Breaks the Daybreak Sun arc, Completed smart list, My Day Dusk, and recurring spawn semantics. |
| **Store `duration_days` or time-of-day blocks** | Rejected | Tasks in `todo-core` have `Date`-granularity `due` dates (exact timestamps belong to `Reminder`). Storing `duration` duplicates `due − start_date` and creates stale-state conflicts on edits. Calendar `start_date: Option<Date>` is exact, timezone-free, and derives duration cleanly. |

---

## 2. How it interacts with the rest of the app

| Existing feature | Interaction rule |
|---|---|
| **Completion (`CompleteTask` / `UncompleteTask`)** | `Done` **is** `completed_at.is_some()`. Completing a task in List, Board, Timeline, or Detail moves it to `Done` and plays the exact Playful completion choreography (`CHECK_FILL` → `CHECK_TICK` → `COMPLETE_HOLD` → `SUN_TRAVEL`). Reopening restores its prior open `status` (`In progress`, `To do`, or `Backlog`), or sets the target column if dragged out of `Done`. |
| **Daybreak Sun & header counts** | Unchanged. The sun measures *today's completions* (`done of total done · N open`) identically in List, Board, and Timeline. Moving a task from `Backlog` → `In progress` does **not** move the sun; completing it advances the sun. "The sun means done" stays 100% pure. |
| **My Day (and Dusk)** | Stays **List** view only (mixing checklist tasks from *Groceries* into a Kanban board creates a noisy "no status" bucket and undermines My Day's calm daily ritual). Tasks from workflow-enabled lists quietly show their status mark in My Day **only when non-default** (`In progress` or `Backlog`), keeping ordinary `To do` rows uncluttered. |
| **Important, All, Completed, Agent activity** | Stay **List** view. Same quiet cross-list rule: show a task's status mark only when its list has Workflow enabled **and** its open status is `In progress` or `Backlog`. |
| **Planned** | Offers **List | Timeline**! Because Planned already collects every dated open task across lists, Timeline is naturally useful here without cluttering simple lists. Bars in Planned carry each task's own `ListColors` (`--list-tint` / `--list-swatch` / `--list-ink`). |
| **Collections** | If any child list has Timeline enabled, the Collection offers **List | Timeline** (bars tinted by child list hue). Collections stay List + optional Timeline (no cross-list Board mixing checklist and workflow lists). |
| **Smart-list query DSL (`Ctrl+K`, MCP)** | Adds `status:backlog`, `status:todo`, `status:in_progress`, `status:done` (plus `!=` and `sort:status`) and `start<=today`, `start:any`, `start:none` (plus `sort:start`). Works across all lists (`status:todo` matches open tasks in checklist lists too, since their stored status defaults to `Todo`). |
| **Repeat (Recurrence)** | Completing a recurring task (via checkbox **or** dragging into `Done` on the Board) spawns the next occurrence with `status = Todo` (never inheriting `InProgress` or `Backlog`). If the task had both `start_date` and `due`, `spawn_next` shifts `start_date` by the exact same `delta = days_between(old_due, next_due)` (mirroring reminder shifting), preserving the task's timeline span! Clearing `due` clears `recurrence` as today; `start_date` is kept unless cleared. |
| **Steps (Subtasks)** | Steps remain simple binary checklists inside a task — never sub-statuses. Completing steps updates the compact step meter (`2/4`) on rows and cards without silently auto-changing the parent task's status (predictable, zero surprise undo). |
| **Moving a task between lists (`MoveTask`)** | Always preserves the task's stored `status` and `start_date` (non-destructive). If moved from *Home renovation* to *Groceries*, the fields stay dormant; moving it to *Q4 Launch* surfaces them immediately. |
| **AI Agents (MCP)** | Huge collaboration upgrade: `list_lists` exposes each list's `features` (`workflow`, `timeline`, `default_view`). Agents with `plan` access can transition a task to `in_progress` when starting work (appearing live in the Board's **In progress** column with the agent's letter mark `C`/`X`/`π` and the calm `SYNC_HIGHLIGHT` wash fade) and call `complete_task` when finished. |

---

## 3. Status on rows, in task detail & in quick-add

### 3.1 Visual language for the 4 stages (zero pill bloat)

Following `docs/design-language.md` (*Pills are rationed* and *Persistent states: fill, not an outline shift*), status never uses rainbow pills or colour-only dots. Instead, it uses **four distinct single-colour geometric circle glyphs** (24-unit SVG grid, `currentColor`, stroke `1.8` so they stay crisp at `14 px`):

| Stage | Key | Glyph | Colour token | Surface treatment |
|---|---|---|---|---|
| **Backlog** | `backlog` | Dashed circle (`status-backlog`) | `ink-3` | Plain icon + text, no container (quietly recessed) |
| **To do** | `todo` | Open solid ring (`status-todo`) | `ink-2` | Plain icon + text, no container (baseline open) |
| **In progress** | `in_progress` | Half-filled progress circle (`status-progress`) | `accent-strong` | Subtle `wash` container (`radius::SM` `4 px`, `6 px` horizontal pad, `20 px` tall) — NOT a pill! Draws calm focus to active work |
| **Done** | `done` | Filled circle with checkmark (`status-done`) | `accent` (`check-selected` on `wash`) | Unified with Playful's checked checkbox |

Every foreground/background pair reuses existing contrast-guarded `Palette` tokens (`ink-3`, `ink-2`, `accent-strong` on `pane`/`canvas`/`wash` ≥ **4.5:1** text and ≥ **3:1** graphics across all 360° hues in light and dark themes).

### 3.2 List view rows

- **Leading checkbox stays a round checkbox** for 1-click completion everywhere — never replace it with a status dropdown that forces two clicks to check off a task.
- **In a workflow-enabled list:** The status mark sits **first in the metadata line** (before step meter, mail icon, and tags), keeping the exact `56 px` desktop (`64 dp` phone) row height and leaving the title column completely uncrowded for long Czech titles.
  - Clicking the status mark on desktop/web (or pressing **`S`** on a selected row) opens a compact 4-choice **Status popover** anchored to the mark; **`Shift+S`** cycles `Backlog → To do → In progress → Done`.
  - When a task also has a `start_date` (timeline-enabled list), the right-aligned Due column displays a compact mono span (`19–23 Nov`, or `28 Nov–3 Dec` across months) instead of a single date; overdue items keep the exact red `OverdueBadge` (`◆ 1d overdue`).
- **In cross-list rooms (My Day, Important, Planned, All, Collections):** Only tasks belonging to a workflow-enabled list with status `In progress` or `Backlog` show the status mark (right after the list chip). `To do` and checklist tasks omit it.
- **In simple checklists (Workflow OFF):** Zero visual change from today's app.

### 3.3 Task detail pane / screen

- **When Workflow is ON for the task's list:** A **Status** property row (`desktop::PROP_ROW` `34 px`, Android `48 dp`) sits **first** in the property list (above Dates, Priority, Reminder, Repeat, Tags), with keycap **`S`**. Selecting `Done` invokes `CompleteTask` (with full completion Undo).
- **When Timeline is ON for the task's list:** A **Starts** property row (`calendar-range` icon, keycap **`⇧D`**) sits directly above **Due** (`D`). Choosing a Start date opens the shared Calendar picker (desktop/web popover; Android `StartDateSheet` with inverted bottom presets `Today`, `Tomorrow`, `Next week`, `Clear`). If the chosen `start_date` is after `due`, `due` shifts to match `start_date` (preserving the `start_date <= due` invariant) in the same undoable patch.
- **When Workflow or Timeline is OFF for the task's list:** Those property rows are **not rendered** — simple checklists keep their exact current detail pane.

### 3.4 Quick-add composer & parser

- **Picker buttons (Workflow lists only):** A compact **Status button** (showing the staged status glyph, default `status-todo`) is placed immediately before Calendar and Flag (`[Status] [Calendar] [Flag]`), keeping the Calendar-before-Flag order intact. Picking a non-default status stages a removable preview chip (`◐ In progress ×`) beside any date/priority chip. On Board view, inline column composers automatically pre-stage that column's status!
- **Calendar picker (Timeline lists only):** Adds a compact **Starts** / **Due** range toggle at the top of the calendar grid (or bottom on Android above the presets) so the user can pick either a single Due date or a `Start → Due` span (`26–28 Nov ×`).
- **Typed quick-add syntax (safe against English & Czech prose):**
  - **Status suffix (workflow lists only):** Explicit `status:<value>` or `stav:<value>` (e.g. `status:backlog`, `status:todo`, `status:wip`, `status:in_progress`, `stav:rozpracovano`). Because `status:` / `stav:` never occur in normal prose, there are zero false positives; `\status:wip` keeps it literal using the existing backslash escape rule.
  - **Date range suffix (timeline lists only):** Compact `..` range between two existing date tokens (`mon..fri`, `24 Nov..28 Nov`, `today..+4d`, `2026-11-24..2026-11-28`, Czech `po..pa`, `dnes..+3d`). Uses `..` rather than `-` so hyphenated prose (`Check-in 12-14`, `v1-2`) is never misparsed.

---

## 4. View switcher (Desktop, Web & Android thumb-zone)

### 4.1 Visibility rule

| List / room configuration | Switcher shown |
|---|---|
| Simple checklist (Workflow OFF, Timeline OFF) | **None** (zero extra chrome) |
| Workflow ON, Timeline OFF | `List` · `Board` |
| Workflow OFF, Timeline ON *(or Planned / eligible Collection)* | `List` · `Timeline` |
| Workflow ON, Timeline ON | `List` · `Board` · `Timeline` |

### 4.2 Desktop & Web placement

- Lives in the **room header's eyebrow row**, directly to the left of the `⋯` Room menu button (`height = 28 px`, `radius::MD` `6 px`, `canvas` track with `1 px` `line` border; active segment uses `raised` fill, `ink` 600 label, and `1 px` `line-strong` inner edge).
- **Responsive compaction:** Below `desktop::LIST_MIN` (`556 px`) — e.g. when the docked Detail pane is widened — segments drop text labels to **icon-only** (`28 × 24 px` each) with tooltips and `aria-label`.
- **Keyboard:** `V` cycles enabled views; `1` / `2` / `3` jump directly to enabled views while the room canvas has focus.

### 4.3 Android placement (strict **Sky above, hands below**)

The Living Daybreak Horizon header at the top of the screen remains **100% non-interactive** — zero view tabs, dropdowns, or buttons in the sky.

- **Cover screen (`412 × 960 dp`):**
  - **Lower Dock (`52 dp`):** Adds a compact **`[View]` button** (`44 × 48 dp` target, active `wash` highlight when in Board or Timeline) between the scrollable Room Pills and `[🔍 Filter]` `[⋯]`.
    - When **2 views** are enabled (`List`/`Board` or `List`/`Timeline`): tapping toggles directly (`CONTENT_SWAP` 120 ms).
    - When **3 views** are enabled: tapping opens an **upward anchored popover** directly above the button (`List`, `Board`, `Timeline` `48 dp` rows, current ticked, plus `List settings…` below a hairline).
  - **Upper Dock (`52 dp`):** Adapts cleanly to the active view!
    - In **List view:** unchanged Quick-Add Composer (or Filter Field).
    - In **Board view:** **Column Switcher Strip** (`Backlog 2` · `To do 3` · **`In progress 3`** · `✓ 3`) + accent `[+]` button (tapping `+` expands the Quick-Add Composer pre-targeted to the active column directly above the IME!).
    - In **Timeline view:** **Timeline Controls Strip** (`Today` · `1W | 2W | M` zoom · `Unsched 2` · `[+]`).
  - **Left-handed cover mirror:** The Lower Dock mirrors (`[⋯]` `[🔍]` `[View]` · Room Pills · `[≡]`), and `[+]` in the Upper Dock mirrors to the leading edge.
- **Unfolded inner screen (`876 × 970 dp`):**
  - Full-width **Inner Bottom Thumb Dock (`56 dp`)** split cleanly around the hinge dead zone:
    - **Left-thumb cluster:** `[≡ Rooms]` · active Room Pill · inline **`List | Board | Timeline`** segmented control (`36 dp` visual inside `48 dp` hit area).
    - **Centre across the fold:** intentionally empty (zero controls over the hinge).
    - **Right-thumb cluster:** `[+ Add task]` primary button · `[🔍 Filter]` · `[⋯ Room menu]`.

---

## 5. Board (Kanban) view

### 5.1 Layout & surface hierarchy

Preserves Playful's flat, unblurred elevation grammar (`pane` → `canvas` → `raised`):

- **Desktop & Web (`1440 × 900` reference):**
  - Board stage sits on `pane` with `32 px` horizontal padding (`desktop::LIST_PAD_X`) and `12 px` gaps (`board::COL_GAP`).
  - Four column wells (`Backlog`, `To do`, `In progress`, `Done`) sit on `canvas` (`radius::LG` `10 px`, `1 px` `line` border, default `280 px`, flexible `248–320 px`). At 1440 px with the 248 px sidebar and Detail closed (`1192 px` room width), **all four columns fit side by side with zero horizontal scroll**.
  - When a card is clicked and the **Detail pane (`392 px`)** opens, the `Done` column automatically collapses to a **`44 px` vertical rail** (`✓ Done · 3`, clickable to expand and still a live drop target!) so the three active open columns (`Backlog`, `To do`, `In progress`) stay wide and readable beside Detail. Below `1200 px` (`desktop::THREE_PANE_MIN`), Detail opens as the standard scrim overlay.
  - **Cards** sit on `raised` (`1 px` `line` border, `radius::LG` `10 px`, `12 px` padding, zero drop shadow at rest). Selected card gets `wash` fill + `3 px` left `accent-strong` bar.
  - **Card content (max 2 concise rows):**
    1. `20 px` round checkbox (`danger` ring if overdue) + Title (`Geist 500 14/19`, wraps up to **2 lines** with ellipsis) + Important star (`★` visible at rest when on; quiet outline on hover/focus).
    2. Metadata row (indented to align with title): due date or `start–due` span (`DUE` mono) or red `OverdueBadge`, step meter (`2/4`), neutral priority (`!` / `!!` / `!!!`), mail mark, up to 2 `#tags` (`+N` overflow), and trailing `20 px` Agent letter mark (`C`, `X`, `π`).
- **Android Cover (`412 × 960 dp`):**
  - Displays **one full column at a time** (`360 dp` wide) inside a `HorizontalPager`, flanked by **`14 dp` peeks** of neighbouring columns (`12 dp` gap) so horizontal swipe affordance is unmistakable.
  - Horizontal pager gestures ignore touches starting within `phone::EDGE_GESTURE_INSET` (`24 dp`) of either screen edge so Android predictive edge-back is never intercepted.
  - Bottom Upper Dock holds the 4 thumb-reachable **Column Switcher Tabs** (`Backlog 2` · `To do 3` · `In progress 3` · `✓ 3`) + `[+]` FAB. Tapping a tab glides the pager smoothly (`PANEL` 200 ms, `STANDARD`).
- **Android Inner (`876 × 970 dp`):**
  - **Browsing (Detail closed):** Full `876 dp` width showing **3 open columns (`Backlog` | `To do` | `In progress`, ~`260 dp` each) + the `44 dp` collapsed `Done` rail** (all 4 states visible in one glance!).
  - **Editing (Card tapped):** Detail slides into the right half (`438 dp`, aligned cleanly to the fold), while the left half (`438 dp`) shows **2 horizontally scrollable columns** (or 1.5 columns snapping cleanly) so the left thumb can switch cards while the right thumb edits Detail.

### 5.2 Quick-add, empty columns & completed column

- **Per-column Quick-Add:** Board view omits the full-width top composer. Instead, each open column (`Backlog`, `To do`, `In progress`) has an inline **`+ Add to <Column>`** footer at the bottom of its card stack (`36 px` desktop, `44 dp` touch target Android). Activating it transforms the footer into a focused inline input (with Calendar & Flag pickers and the shared quick-add parser) that creates tasks directly in that column's status. On Android Cover, tapping the bottom-right `[+]` FAB opens the standard bottom Composer above the IME pre-targeted to the active column.
- **Empty columns:** Open columns never disappear when empty; they show a calm dashed drop well (`1 px dashed line-strong`, `radius::MD`, quiet `ink-3` hint like *"Drop future ideas here"* / *"Nothing in progress"*) above `+ Add to <Column>`.
- **`Done` column behaviour:**
  - Displays **recently completed tasks only** (default: **last 7 days**, capped at 20 cards, newest first), followed by a quiet **`View all N in Completed →`** link at the bottom of the column. A list with 400 old completions never slows down or crowds the board.
  - Has **no** `+ Add` footer.
  - **Dragging into `Done` (including onto the collapsed `44 px` rail)** runs `CompleteTask` (completion animation, sun arc advance, recurring next-occurrence spawn in `To do`, Undo toast).
  - **Dragging out of `Done` into an open column** runs `UncompleteTask` + status update + placement in one atomic undoable command.

### 5.3 Drag, touch & keyboard interaction

- **Pointer drag (Desktop & Web):**
  - Pickup (`HOVER` 90 ms): lifted card gains `shadow-overlay` (`1 px` `line-strong` in dark mode) and tracks the pointer 1:1 with zero lag (never scaling text). Origin slot shows a quiet `wash` placeholder; hovered target column renders a `3 px` `accent-strong` horizontal insertion bar.
  - Dropping in the same column issues `Reorder`; dropping in a different open column updates `status` + `position` in one undoable command (`SetTaskStatus` / `UpdateTask + Reorder`) and shows a toast (`Moved "Strip wallpaper…" to In progress · Undo`).
- **Touch drag & one-handed alternative (Android):**
  - Long-press a card (`LongPress` haptic, `DRAG_TOUCH_SCALE` 1.01) to lift it. On Inner, drag directly across visible columns (or onto the `Done` rail). On Cover, drag toward the left/right edges (auto-flips after `350 ms` dwell) **or drop directly onto any Column Tab in the bottom Upper Dock!**
  - **Zero-drag accessible alternative:** Tapping any card opens Detail where the `Status` row sits in the thumb zone, or long-press `⋯` → `Move to column…` opens a compact 4-row bottom sheet.
- **Keyboard (Desktop & Web):**
  - `H` / `L` (or `Left` / `Right`): move column focus (preserving vertical row index clamped to column count).
  - `J` / `K` (or `Up` / `Down`): move card focus within the column.
  - `Alt+Left` / `Alt+Right`: move focused card one column left/right (with Undo!).
  - `Alt+Up` / `Alt+Down`: reorder focused card up/down within its column.
  - `Space`: complete/reopen; `S`: Status picker; `Enter` / `E`: open Detail / edit title; `N`: focus active column's `+ Add` input; `Ctrl+click` / `Shift+click`: multi-select cards with bulk **Status** action in the selection bar.

---

## 6. Timeline view

### 6.1 Date representation & bar shapes

Timeline is driven by `start_date: Option<Date>` and `due: Option<Date>` (local calendar dates, invariant `start_date <= due` when both exist):

| Stored dates | Timeline rendering | Dragging behaviour |
|---|---|---|
| **`start_date` + `due`** | Horizontal **span bar** (`start_date` left edge to `due` right edge, inclusive) with `8 px` left/right resize handles. | Body shifts both dates by `Δ` days; left handle edits `start_date`; right handle edits `due`. |
| **`due` only** (`start_date == None`) | **1-day milestone bar** anchored on `due` with a leading flag/diamond mark and label flowing cleanly to the right of the bar. | Body shifts `due`; dragging the left handle leftward sets `start_date` and stretches it into a span! |
| **`start_date` only** (`due == None`) | **Open-ended starting bar** (1-day default width with a dashed trailing right edge). | Body shifts `start_date`; dragging the right handle rightward sets `due`. |
| **Neither date** | Placed in the collapsible **Unscheduled** tray. | Drag onto any date cell (desktop/web) or tap `Schedule` (all clients) to assign dates. |

### 6.2 Zoom levels, today marker & layout per client

- **Zoom levels:**
  - **Week (`1W`):** 7 days (default on Android Cover, showing 6–7 touch-wide day columns).
  - **2 Weeks (`2W`):** 14 days (**default on Desktop, Web, and Android Inner** — ideal balance for multi-day spans and readable labels).
  - **Month (`M`):** 4–5 weeks with Monday-first ISO week markers (`W47`, `W48`, `W49` — matching the Daybreak eyebrow).
  - **Quarter (`Q`):** 12 weeks by week columns (Desktop & Web only, for long projects like *Home renovation*).
- **Today marker:** Today's column header gets a subtle `wash` tint with `accent-strong` day/number, a `7 px` filled `accent-strong` dot on the bottom ruler edge, and a crisp **`2 px` vertical hairline** (`accent-strong`) running down through all lanes. Weekend columns (`Sat`, `Sun`) receive a quiet `canvas` background step so weekly rhythm scans effortlessly.
- **Label legibility rule:** Whenever a bar's pixel width is narrower than `110 px` (milestones, 1–2 day spans in Month zoom), the task title renders **outside to the right of the bar** (or to the left if near the right edge) so titles are never truncated to unreadable stubs.
- **Bar styling (flat, zero gradients):**
  - Single-list Timeline: `raised` + `line-strong` border for `To do`; `wash` + `accent` border + `3 px` left `accent-strong` cap for `In progress`; dashed `canvas` bar for `Backlog`; `danger-bg` + `danger` border + `danger-ink` text for **overdue** tasks.
  - Cross-list Timeline (**Planned** smart list or **Collection**): bars use each task's **`ListColors`** (`--list-tint` fill, inset `3 px` `--list-swatch` leading rail, `--list-ink` label).
- **Desktop & Web chrome:**
  - Quiet `40 px` **Timeline Toolbar** directly below the Daybreak sky header (`< Today >` + mono date range on left; `Week | 2W | Month | Quarter` zoom + `Unscheduled · N` toggle on right).
  - Sticky `224 px` left **Task Rail** (collapsible) so every lane's title stays readable regardless of horizontal scroll.
  - Collapsible **Unscheduled tray** (`148 px` expanded, `36 px` collapsed) docked above the reserved `56 px` Undo strip.
- **Android Cover (`412 × 960 dp`) & Inner (`876 × 970 dp`):**
  - Top: Non-interactive Living Daybreak Horizon header + non-interactive `44 dp` sticky **Date Ruler** (6 days visible on Cover, 14 days on Inner).
  - Lanes: `48 dp` tall (`phone::TOUCH`) with `32 dp` bars; optional quiet status group headers (`In progress · 3`, `To do · 3`) so Cover needs no left Task Rail to waste horizontal width.
  - Bottom Upper Dock (`52 dp`): `[Today]` · `[1W | 2W | M]` zoom · `[Unsched · N]` (opens a bottom sheet with quick date presets `Today`, `Tomorrow`, `Next week`, `Pick dates…`) · `[+]` FAB.
  - Touch reschedule/resize: tap a bar to select it (revealing tactile left/right edge handles), then drag the body or handle horizontally in 1-day snaps with subtle `TextHandleMove` haptics and a live date readout; or tap again to open Detail.

### 6.3 Keyboard interaction (Desktop & Web)

- `Up` / `Down` (`J` / `K`): move selection between timeline lanes (auto-scrolling the bar into horizontal view).
- `Alt+Left` / `Alt+Right`: shift selected task's dates by **±1 day** (`Shift+Alt+Left/Right` for **±1 week**), with one Undo toast.
- `[` / `]`: shrink or extend duration (`due` ±1 day, initializing `start_date` if due-only).
- `0` (or `T` when not focused on a task? Keep **`0`** for Jump to Today so `T` stays Tags!): scrolls Today smoothly into view.
- `D` / `Shift+D`: open Due / Start date calendar popover.

---

## 7. Per-list settings UI

Every new list is created exactly as today (instant inline rename, **Simple checklist** — both toggles OFF). Enabling advanced views is two clicks away and completely non-destructive:

- **Entry points:**
  - Room menu (`⋯`) → **`List settings…`** (placed below `Colour…` and `Move to…`, separated from `Delete list…`).
  - Sidebar / Android Room Switcher context menu on any list → **`List settings…`**.
- **Presentation:**
  - **Desktop & Web:** In-place sub-page inside the `⋯` Room Menu popover (`304 px` wide, `CONTENT_SWAP` 120 ms, `← List settings` header, `Esc` returns cleanly).
  - **Android:** In-place sub-page inside the **Room Actions Bottom Sheet** (`CONTENT_SWAP` 120 ms, `48 dp` rows, thumb-reachable).
- **Fields:**
  1. **Workflow & Board** *(switch, default OFF)* — *"Track Backlog, To do, In progress and Done, and unlock Board view."*
  2. **Timeline & Start dates** *(switch, default OFF)* — *"Add start dates to tasks and view them over time."*
  3. **Default view** *(visible only when ≥ 1 toggle above is ON; segmented control `List` | `Board` | `Timeline`, showing only enabled views)* — *"Opens this view when you visit the list."*
  4. Quiet `ink-3` reassurance note: *"Turning a feature off hides its view. Your task statuses and start dates are kept."*
- **Synced vs. local state:**
  - **Synced on `List` (`todo-core`):** `workflow`, `timeline`, `default_view`. A project list should open as a Board on Windows, Web, and Android alike, and AI agents need to know which lists use statuses.
  - **Local-only client preferences (`ui.json` / `localStorage` / `SharedPreferences`):** Timeline zoom level, collapsed/expanded `Done` rail state, Unscheduled tray open/closed state, and temporary in-session view overrides.

---

## 8. Domain & data implications (engineer handoff)

Short, concrete specification for `crates/todo-core`, `crates/todo-client`, and `crates/todo-server`:

### 8.1 New domain types & entity fields (`crates/todo-core/src/model.rs`)

```rust
/// Open workflow stage of a task. Completion (`completed_at.is_some()`) always represents "Done".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Backlog,
    #[default]
    Todo,
    InProgress,
}

/// Default room presentation for a list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListView {
    #[default]
    List,
    Board,
    Timeline,
}

/// Per-list optional capability flags (all default false / List for backward compatibility).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListFeatures {
    #[serde(default)]
    pub workflow: bool,
    #[serde(default)]
    pub timeline: bool,
    #[serde(default)]
    pub default_view: ListView,
}
```

- **`List`:** add `#[serde(default)] pub features: ListFeatures`.
  - Validation in `decide::validate_entity`: `default_view == Board` requires `workflow == true`; `default_view == Timeline` requires `timeline == true` (disabling a feature automatically resets `default_view` to `List` in the same `UpdateList` event).
- **`Task`:** add:
  - `#[serde(default)] pub status: WorkflowStatus` (defaults to `WorkflowStatus::Todo` for all historical events and snapshots).
  - `#[serde(default)] pub start_date: Option<Date>`.
  - Invariant in `decide::validate_entity`: when both `start_date` and `due` are `Some`, require `start_date <= due` (`DomainError::invalid("start_date", "must be on or before due date")`).

### 8.2 Commands, events & undo (`command.rs`, `event.rs`, `decide.rs`, `undo.rs`)

- **Commands:**
  - `UpdateList`: `features` is an optional `ListFeaturesPatch`. It updates only the supplied `workflow`, `timeline`, or `default_view`; the event records the resulting settings together.
  - `NewTask`: add `#[serde(default)] status: WorkflowStatus`, `#[serde(default)] start_date: Option<Date>`.
  - `TaskPatch`: `status: Option<WorkflowStatus>` and `start_date: Option<Option<Date>>` (using `double_option`, matching `due`). The server reconciles a one-date update against the current other endpoint: if they cross, the unchanged endpoint moves to match the edited one in the same event. If both dates are supplied, reject an inverted span.
  - `BulkTaskAction`: add `Status { status: WorkflowStatus }` (completing via bulk already uses `Completion { done: true }`; setting status to an open stage on a completed task can optionally reopen or ignore completed rows — recommend matching `Reorder`: act on open tasks, or offer explicit `SetStatusAndReopen` only from board drag-out-of-Done as a 2-event command group).
  - **Board drag across columns:** Either extend `UpdateTask` with an optional `placement: Option<Placement>` (atomic status + position change in one event group, single undo!) or emit a single command `MoveTaskStatus { id, status, placement }` / `CompleteTask` when targeting `Done`. Keeping status + reorder in **one command group** ensures `Ctrl+Z` reverses both cleanly in a single step.
- **Events & conflict keys:**
  - `FieldChange::ListFeatures(Change<ListFeatures>)` → `FieldKey::ListFeatures`
  - `FieldChange::Status(Change<WorkflowStatus>)` → `FieldKey::Status`
  - `FieldChange::StartDate(Change<Option<Date>>)` → `FieldKey::StartDate`
- **Sync, merge & recurrence rules:**
  - **Backward compatibility:** Every new field uses `#[serde(default)]` so existing SQLite/IndexedDB snapshots, old event logs, and `ImportEntity` payloads replay identically with zero migration downtime.
  - **`MergeTasks`:** Target keeps its own `status` and `start_date` (if target has `start_date == None` and source has `start_date <= target.due`, optionally leave target unchanged to stay predictable — simplest & least surprising rule).
  - **`spawn_next` (recurrence):** Always sets `status: WorkflowStatus::Todo`. If `t.start_date` and `t.due` are both `Some`, shifts `start_date` by `days_between(due, next)` using `add_days`.
  - **Legacy sync clients:** JSON/SSE event reads and REST activity reads omit the new field-change variants unless `workflow_fields=true` is requested. A projected event keeps the same sequence, ID and batch markers, even if it has no legacy field changes. Updated clients replay the confirmed cache once on upgrade, keeping their outbox, receipts and log epoch. The server database needs no schema migration.
  - **Column ordering (`PosKey`):** Reuse the list's single `Task.position: PosKey`! Filtering a list's tasks by `status` preserves their relative `PosKey` order inside every column with zero second-sort-key sync conflicts. When dragging a card between columns, `Placement::Before/After(sibling_in_target_column)` computes a valid `PosKey` cleanly in the shared list scope!

### 8.3 Query DSL & MCP changes

- **Query DSL (`crates/todo-core/src/query/`):**
  - `Predicate::Status { op: CmpOp, value: QueryStatus }` where `QueryStatus` is `Backlog | Todo | InProgress | Done` (`status:done` evaluates `task.is_done()`; others require `!task.is_done() && task.status == ...`).
  - `DateField::Start` (`start<=today`, `start>=2026-11-24`, `start:any`, `start:none`).
  - `SortKey::Status` (`Backlog < Todo < InProgress < Done`) and `SortKey::Start`.
- **MCP tools (`crates/todo-server/src/mcp/tools.rs`):**
  - `list_lists`: include `features: { workflow, timeline, default_view }` on each returned list.
  - `get_task` / `query_tasks`: include `status` (`"backlog" | "todo" | "in_progress" | "done"`, derived when `completed_at` is set) and `start_date` (`YYYY-MM-DD | null`).
  - `create_task` (`capture` level): accept optional `status` (`"backlog" | "todo" | "in_progress"`) and `start_date`.
  - `update_task` (`plan` level): accept optional `status` (`"backlog" | "todo" | "in_progress"` — setting `"done"` either routes cleanly to `CompleteTask` or directs the agent to `complete_task`; routing `"done"` cleanly or rejecting with a clear hint *"use complete_task to mark done"* keeps recurrence & undo crystal clear — **recommend documenting `complete_task` for Done and rejecting `"done"` in `update_task` with a clear message**, matching how `completed_at` is already handled) and `start_date` (nullable to clear).
  - Update `INSTRUCTIONS` briefly so agents know to check `list_lists().features.workflow` and set `status: "in_progress"` while actively working on a task.

---

## 9. Tokens, motion & accessibility

### 9.1 Proposed additive tokens (`crates/todo-design`)

No existing `Palette` colour keys need changing — every surface, status mark, bar, and rail reuses existing contrast-guarded tokens (`pane`, `canvas`, `raised`, `wash`, `tint`, `line`, `line-strong`, `ink`, `ink-2`, `ink-3`, `accent`, `accent-strong`, `accent-fill`, `on-accent`, `danger`, `danger-bg`, `danger-ink`, and `ListColors`).

Add these 19 layout/icon tokens to `crates/todo-design/src/layout.rs` and `icons.rs`, then regenerate `tokens.css` and `tokens.json` with `TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated`:

| Module | Rust constant / variant | CSS / JSON key | Value | Purpose |
|---|---|---|---|---|
| `layout::board` | `COL_WIDTH` | `--board-col-width` | `280.0` (px) | Default desktop/web Kanban column width (fits 4 cols at 1440 px). |
| `layout::board` | `COL_MIN` | `--board-col-min` | `248.0` (px) | Minimum readable column width before horizontal board scroll. |
| `layout::board` | `COL_MAX` | `--board-col-max` | `320.0` (px) | Maximum flexible column width on ultrawide screens. |
| `layout::board` | `COL_COLLAPSED` | `--board-col-collapsed` | `44.0` (px/dp) | Collapsed vertical `Done` rail width (also a live drop target). |
| `layout::board` | `COL_GAP` | `--board-col-gap` | `12.0` (px/dp) | Gap between board columns (`space::S3`). |
| `layout::board` | `CARD_PAD` | `--board-card-pad` | `12.0` (px/dp) | Internal card padding (`space::S3`; `14 dp` on Android cover). |
| `layout::board` | `CARD_GAP` | `--board-card-gap` | `8.0` (px/dp) | Vertical gap between cards in a column (`space::S2`). |
| `layout::board` | `HEADER_HEIGHT` | `--board-header-height` | `36.0` (px/dp) | Column header strip height. |
| `layout::timeline` | `TOOLBAR_HEIGHT` | `--timeline-toolbar-height` | `40.0` (px) | Desktop/web timeline navigation, zoom, and Unscheduled bar height. |
| `layout::timeline` | `RULER_HEIGHT` | `--timeline-ruler-height` | `44.0` (px/dp) | Sticky date ruler height across desktop, web, and Android. |
| `layout::timeline` | `RAIL_WIDTH` | `--timeline-rail-width` | `224.0` (px) | Desktop/web sticky left task-label rail width. |
| `layout::timeline` | `LANE_ROW` | `--timeline-lane-row` | `40.0` (px) | Desktop/web timeline lane height. |
| `layout::timeline` | `LANE_ROW_PHONE` | `--timeline-lane-row-phone` | `48.0` (dp) | Android touch-safe timeline lane height (`phone::TOUCH`). |
| `layout::timeline` | `BAR_HEIGHT` | `--timeline-bar-height` | `28.0` (px) | Desktop/web timeline task bar height (`radius::MD` 6 px). |
| `layout::timeline` | `BAR_HEIGHT_PHONE` | `--timeline-bar-height-phone` | `32.0` (dp) | Android timeline task bar height. |
| `layout::timeline` | `TODAY_LINE` | `--timeline-today-line` | `2.0` (px/dp) | Width of vertical Today marker hairline (`accent-strong`). |
| `layout::timeline` | `RESIZE_HIT` | `--timeline-resize-hit` | `8.0` (px) | Hit width of left/right date-resize handles on a timeline bar. |
| `layout::timeline` | `TRAY_COLLAPSED` / `TRAY_EXPANDED` | `--timeline-tray-collapsed` / `--timeline-tray-expanded` | `36.0` / `148.0` (px) | Unscheduled tray collapsed header and expanded height. |
| `layout::component` | `VIEW_SWITCHER` | `--component-view-switcher` | `28.0` (px) | Height of segmented view switcher in desktop/web header eyebrow. |
| `icons::Icon` | `CircleDashed`, `Circle`, `CircleProgress`, `StatusDone`, `Columns3`, `GanttChart`, `CalendarRange` | `status-*` / Lucide keys | 24-unit SVG, stroke 1.6–1.8 | 4 geometric status marks + Board, Timeline, and Start-date icons (`currentColor`, zero multicolour). |

### 9.2 Motion

Zero new curves or durations needed — every moment maps directly onto `todo_design::motion`:

- **Switching view (`List ↔ Board ↔ Timeline`):** `CONTENT_SWAP` (`120 ms`, `STANDARD`) opacity crossfade of the room body while the Daybreak sky header, sun, and sidebar stay completely anchored. Under reduced motion: `REDUCED_FADE` (`100 ms`).
- **Board card drag & drop:** Pickup elevates with `HOVER` (`90 ms`, `STANDARD`; `DRAG_TOUCH_SCALE` `1.01` on touch only); drag tracks pointer/finger **1:1**; drop settles siblings over `ROW_ENTER` (`180 ms`, `STANDARD`; `0 ms` snap under reduced motion). Dropping into `Done` runs the full completion choreography (`CHECK_FILL` → `CHECK_TICK` → `SUN_TRAVEL`).
- **Android Cover column swipe:** `HorizontalPager` tracks thumb 1:1; tapping a bottom Column Tab glides over `PANEL` (`200 ms`, `STANDARD`; instant under reduced motion).
- **Timeline zoom & Today jump:** Horizontal scroll to Today eases over `PANEL` (`200 ms`, `STANDARD`); zoom scale switches via `CONTENT_SWAP` (`120 ms`) keeping the centre anchor date steady.
- **Unscheduled tray expand/collapse & Done rail collapse:** `PANEL` (`200 ms`, `ENTER`/`EXIT`; `0 ms` height under reduced motion).
- **Live agent / remote sync updates:** Never animate layout shifts; update card/bar position cleanly and flash `SYNC_HIGHLIGHT` (`900 ms`, `EXIT` `wash → pane`; reduced motion `COMPLETE_HOLD` + `REDUCED_FADE`).

### 9.3 Accessibility

- **Non-colour status encoding:** Every status stage has a unique geometric silhouette (dashed ring / empty ring / half-filled circle / filled tick) plus visible text label or accessible name (`"Status: In progress"`).
- **Contrast:** All status labels, timeline bar text, Today line, overdue bars, and switch controls meet WCAG **4.5:1** (text) and **3:1** (controls/graphics) across all 12 swatches + 6 smart hues in light and dark themes.
- **Zero drag-only actions:** Every board drag has a 1-key (`Alt+←/→`, `S`) and 1-tap (Detail Status row / `Move to column…` sheet) equivalent; every timeline bar drag/resize has `Alt+←/→`, `[`/`]`, and Detail `Starts`/`Due` calendar pickers.
- **Semantics:** View switcher uses `role="tablist"` / `role="tab"` (`aria-selected`); Board columns expose landmark headings (`"In progress, 3 tasks"`); switches expose `role="switch"` / `aria-checked`; live status moves announce politely (`"Moved Strip wallpaper to In progress"`).
- **Android edge-back & touch targets:** Every interactive control meets `48 dp` (`phone::TOUCH`), nothing interactive sits at the top sky, and horizontal Board/Timeline gestures respect `phone::EDGE_GESTURE_INSET` (`24 dp`).

---

## 10. Visual mockups

Interactive HTML mockups (built directly on `crates/todo-design/generated/tokens.css`, bundled Geist / Geist Mono fonts, authentic Lucide/glyph SVGs, and neutral fixture data) and their 2× rendered PNGs live in [`docs/mockups/status-board-timeline/`](mockups/status-board-timeline/). Open [`docs/mockups/status-board-timeline/index.html`](mockups/status-board-timeline/index.html) to browse interactively or toggle `?theme=light|dark`.

| Screen | HTML source | Rendered 2× PNG |
|---|---|---|
| **Desktop — Board (Light)** | [`desktop-board.html`](mockups/status-board-timeline/desktop-board.html) | [`desktop-board-light.png`](mockups/status-board-timeline/desktop-board-light.png) |
| **Desktop — Board (Dark)** | [`desktop-board.html`](mockups/status-board-timeline/desktop-board.html) | [`desktop-board-dark.png`](mockups/status-board-timeline/desktop-board-dark.png) |
| **Desktop — Timeline (2-Week + Unscheduled tray)** | [`desktop-timeline.html`](mockups/status-board-timeline/desktop-timeline.html) | [`desktop-timeline-light.png`](mockups/status-board-timeline/desktop-timeline-light.png) |
| **Desktop — List with Status, Quick-Add & Detail** | [`desktop-list-status.html`](mockups/status-board-timeline/desktop-list-status.html) | [`desktop-list-status-light.png`](mockups/status-board-timeline/desktop-list-status-light.png) |
| **Desktop — Per-List Settings Popover** | [`desktop-list-settings.html`](mockups/status-board-timeline/desktop-list-settings.html) | [`desktop-list-settings-light.png`](mockups/status-board-timeline/desktop-list-settings-light.png) |
| **Android Cover — Board (`412 × 960 dp`)** | [`android-cover-board.html`](mockups/status-board-timeline/android-cover-board.html) | [`android-cover-board-light.png`](mockups/status-board-timeline/android-cover-board-light.png) |
| **Android Inner — Board (`876 × 970 dp`)** | [`android-inner-board.html`](mockups/status-board-timeline/android-inner-board.html) | [`android-inner-board-light.png`](mockups/status-board-timeline/android-inner-board-light.png) |
| **Android Cover — Timeline (`412 × 960 dp`)** | [`android-cover-timeline.html`](mockups/status-board-timeline/android-cover-timeline.html) | [`android-cover-timeline-light.png`](mockups/status-board-timeline/android-cover-timeline-light.png) |

Re-render all PNGs from the repo root:

```sh
node docs/mockups/status-board-timeline/render.mjs
```

---

## 11. Phased implementation checklist

- [x] **Phase 0 — Design tokens (`crates/todo-design`):** Add `layout::board`, `layout::timeline`, `component::VIEW_SWITCHER`, and the single-colour status and view SVGs (`assets/icons/lucide/`); regenerate exports with `TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated` and verify contrast & portable target checks (`wasm32`, `aarch64-linux-android`).
- [x] **Phase 1 — Domain, Query DSL & MCP (`crates/todo-core`, `crates/todo-server`):**
  - [x] Add `WorkflowStatus`, `ListView`, `ListFeatures` on `List`, and `status` + `start_date` on `Task` (`#[serde(default)]`).
  - [x] Enforce `start_date <= due` and `default_view` invariants in `decide::validate_entity`; update `spawn_next` (reset status to `Todo`, shift `start_date` by recurrence `delta`).
  - [x] Add `FieldChange` / `FieldKey` variants and property tests verifying event replay & undo round-trips.
  - [x] Extend query parser/evaluator with `status:`, `start:`, `sort:status`, `sort:start`.
  - [x] Expose `features`, `status`, and `start_date` in REST + MCP tools (`list_lists`, `get_task`, `query_tasks`, `create_task`, `update_task`) with `capture`/`plan` access checks.
- [x] **Phase 2 — Shared client intent & view models (`crates/todo-client`, `crates/todo-ffi`):**
  - [x] Extend `intent::quick_add` with workflow-gated `status:`/`stav:` suffix, `start..due` range syntax, and `Draft` reconciliation.
  - [x] Add shared pure board grouping (4 columns, 7-day recent `Done` cap), column drag placement resolver, and timeline layout/span/unscheduled projection helpers.
  - [x] Expose list settings, status updates, start-date edits, and board/timeline projections through `todo-ffi` for Android.
- [x] **Phase 3 — Desktop client (`apps/desktop`):**
  - [x] Add `List settings…` in-place sub-page in `menu.rs` and eyebrow segmented view switcher (`V`, `1`, `2`, `3`).
  - [x] Render non-pill status marks in List rows, Quick-Add status picker/chip, and `Status` (`S`) + `Starts` (`⇧D`) rows in Detail.
  - [x] Build Board view (4 columns, collapsible `Done` rail beside Detail, inline column quick-add, pointer drag + `H`/`L`/`Alt+←/→` keyboard).
  - [x] Build Timeline view (toolbar, 4 zooms, sticky rail, Today hairline, bar drag/resize + tooltips, Unscheduled bottom tray).
- [x] **Phase 4 — Web client (`apps/web`):**
  - [x] Mirror List settings, eyebrow view switcher, List/Detail/Quick-Add status & start date, Board view (Pointer Events drag + keyboard), and Timeline view with responsive narrow-viewport layout and Playwright e2e tests.
- [ ] **Phase 5 — Android client (`apps/android`):**
  - [x] Add `List settings` sub-sheet inside Room Actions Bottom Sheet and thumb-zone `[View]` switcher in Lower Dock (Cover) / Left-Thumb segment (Inner). Timeline is disabled on Android until its view lands.
  - [x] Render status marks in rows, Status & Starts rows + `StartDateSheet` in Detail, and Status button in Quick-Add (workflow lists only).
  - [x] Build Cover 1-column `HorizontalPager` Board (with `14 dp` peeks, `24 dp` edge guard, bottom Column Switcher Strip + `+` FAB) and Inner 3-column Board + `44 dp` `Done` rail + hinge-free dock.
  - [ ] Build Cover & Inner Timeline views (sticky non-interactive Date Ruler, `48 dp` lanes, touch reschedule/resize, bottom Zoom/Today/Unscheduled strip + Unscheduled Bottom Sheet).
- [ ] **Phase 6 — Screenshot gate & feature matrix:**
  - [ ] Add light/dark desktop gallery fixtures (`apps/desktop/scripts/gallery.ps1`), web Playwright gallery shots, and Android Paparazzi Fold7 Cover & Inner snapshots.
  - [ ] Update [`docs/feature-matrix.md`](feature-matrix.md) as each client lands.
