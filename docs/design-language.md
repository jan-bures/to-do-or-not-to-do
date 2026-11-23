# Design language: Playful

This document describes how the app looks and moves on every platform: the GPUI desktop app, Android (Compose)
and the web. The numbers live in one place, the `todo-design` crate (`crates/todo-design`). This document explains
the reasoning behind them. When the two disagree, fix whichever one is wrong. Never let a client hard-code its own
value.

The aim is an interface that is calm, precise and a little warm. It should feel crafted rather than generated: a
deliberate type pairing, a restrained palette, exact alignment, and one small illustration that carries meaning.

## Concept: a small landscape that keeps score

Every room has a header, whether it's My Day, a collection or a list. The header is a **sky** in the room's hue,
and it ends in a strip of **two flat hills**. The strip is 48 px on the desktop, 36 dp on a phone's cover screen and
40 dp on a large inner screen.

- **The landscape sun measures completed tasks.** It climbs a faint arc as tasks are done. The part of the
  arc it has travelled is drawn in `sun-ink`, and the part still to go in `arc`. The arc is measured to 3:1 against
  the sky, so it reads as information, not decoration.
- **The count sits beside the sun.** The header summary reads `3 of 12 done · 9 open · est. 5h 40m`. The sun and
  the numbers say the same thing in the same place. Never put a second, floating label on the sun.
- **What the sun measures.** Every client derives the header from one place, `todo_client::room_header`, so the
  same data reads the same everywhere:
  - **Lists, collections, Important, Planned and All** measure *today's work*: tasks completed since local
    midnight (in the owner's time zone, daylight saving included), out of those plus the ones still open. A list
    with 450 old completions starts each day with the sun low, not set.
  - **My Day** measures its whole plan: done of everything planned for today.
  - **Agent activity** measures the tasks agents changed this week: done of all of them.
  - **Completed** only counts: `12 done`, with the sun at the end of its arc.
  - **Activity** and **Trash** are history rooms: they keep no progress score or sun.
  - A room with nothing to measure says `2 finished this month`, or `nothing yet`. The finished day says
    `12 of 12 done · all clear`.
  - A task still playing its completion counts as open until it has left the rows, so the count and the sun
    move once, together.
- **The eyebrow names the room's frame.** One rule per kind of room:

  | Room | Eyebrow |
  |---|---|
  | Smart lists | the day: Saturday `21 NOV 2026` `W47`, since they measure today's work |
  | Collections | Collection `4 LISTS` |
  | Lists | the collection's name (or List) and what is still open: Work `9 OPEN` |
  | Activity | History `ALL CHANGES`, or `AGENT: CLAUDE` while filtered to one agent |
  | Trash | History `DELETED ITEMS` |

  The lead is upright Geist in the room's `accent-strong`; the details are mono caps, set by the type
  token, not by the words.
- **The status bar counts the same room.** `9 open · 3 done today`: what is open now and what was finished since
  local midnight. Activity counts its feed instead: `20 shown · 4 today`.
- **There is only one sun in the landscape.** No sun icon in the title, no clock ticks, no crescent endpoints.
  The landscape and the My Day toggle use the same warm family, independent of the room's hue.
- **The finished day.** When My Day's last task is done, the scene grows to 164 px.
  - The sky becomes a two-stop dusk gradient.
  - The sun sets behind the hills with three thin rings.
  - A moon and stars come out.
  - The page hands over to tomorrow: "When the sun comes up" shows tomorrow's first task with a sunrise mark and a
    *Plan tomorrow* button, followed by the rest of tomorrow and the rest of the week.

  There is no congratulation copy and no stats recap. The scene is the reward. The dusk scene is a night scene in
  both themes, so the header block uses dark-theme text over it.
- **An empty list shows the hills only.** With no tasks there is nothing to measure, so there is no sun and no arc.
  Below the quick-add sits one line of regular Geist in `ink-2` ("Nothing here yet."; a smart list has its own, such as
  "Nothing planned yet.", and a filter that matches nothing says "Nothing matches the filter.") and, if an
  agent has any, suggestions.
- **Restraint keeps it from being clip art.** There are no faces, no characters and no idle animation. Whimsy rests
  on three things only: the sun, the hand-drawn list glyphs and the round checkboxes.

`todo_design::landscape` computes the geometry: hills, arc, sun, rays, rings, stars and moon. GPUI and Compose
draw the same shapes from it. Everything is a single-colour filled path, a stroked polyline or a circle, plus one
linear gradient for the dusk sky. Build the paths when the size or the count changes, never per frame.

## Colour

### How colours are made

Room and accent colours are **authored in OKLCH** and converted to sRGB. Windows caption colours specified by the OS use sRGB directly. A colour that falls outside the sRGB gamut keeps its
lightness and hue and loses chroma. Three kinds of token take their hue from somewhere else:

| Kind | Hue | Dark mode | Used for |
|---|---|---|---|
| **Room** (`Palette`) | the room's hue | hue − 10 | canvas, pane, lines, ink, accent, toast, sky, hills, arc |
| **List** (`ListColors`) | the list's hue | hue − 6 | chip tint and ink, dot, sidebar glyph, active room pill |
| **Agent** (`AgentColors`) | the agent's hue | unchanged | letter mark background, letter, ring |
| **Fixed** | its own | — | danger, success, the sun, the dusk sky |

So a whole room changes atmosphere when its hue changes. My Day (hue 65) is warm apricot, and a teal collection is
cool. The neutrals (canvas, pane, ink) carry a trace of the room's hue, which keeps the grey from looking dead.

Smart lists have fixed hues. Collections and lists pick from twelve **swatches** that are spread around the wheel:
rose, coral, clay, olive, fern, jade, teal, lagoon, azure, cornflower, orchid and pink. The swatch names are stable
keys, used in settings, CSS and JSON. Tags take the same swatches, shown as a small dot (the list's `dot` colour) before
the tag's name. Choosing a colour offers exactly these twelve, as discs in two rows of six with the current one ticked;
there is no free-form colour.

### Palette rules

- **The pane is near-white or near-black, never pure.** The canvas, which holds the sidebar, title bar and status
  bar, sits one step darker in light mode and one step lighter in dark mode.
- **Colour means something, or it isn't there.**
  - Accent is for the checked box, "due soon" and the primary button. Selection bars use `accent-strong`.
  - On a selected row the checked box uses `check-selected` instead, so it still reaches 3:1 on `wash`.
  - Danger is for overdue items.
  - The list hue is for list identity.
  - The sun's amber is for progress.
- **An overdue row has at most two reds:** the checkbox ring and the overdue badge. Priority stays neutral.
- **Priority is never colour.** It is `!` / `!!` / `!!!` in Geist Mono. Count, weight and ink step together from
  `ink-3` up to `ink`.
- **Translucency** is used in the landscape (the sun's halo, stars, the moon and the dusk rings), in neutral
  modal scrims and in small desktop chrome overlays (caption button tints and scrollbar thumbs). UI surfaces
  are opaque.

### Contrast

Contrast is measured from the emitted hex values with WCAG 2.x:

- **4.5:1** for all text, including metadata, toasts and chips;
- **3:1** for control boundaries (checkbox rings, input borders) and for meaningful graphics (the checked box, the
  overdue ring, the sun's remaining arc);
- **informational only** for decorative scenery: the hills, the sun disc against the sky, and the dark silhouettes.
  These must never become a contrast requirement.

OKLCH lightness is perceptual, but WCAG contrast is based on luminance, and the two disagree by hue. A pair that
passes for an apricot room can fall just short for a green one. So a room palette passes through **contrast
guards** after it is derived (`todo_design::palette::GUARDS`). Any guarded foreground that misses its minimum
moves away from its background in lightness, in 0.005 steps, until it passes. Pairs that already pass are not
touched, including every pair in the approved mockups.

The crate's tests check two things:

- every pair in the approved contrast table, at its exact hex values;
- every text and control pair at every whole-degree hue, in both themes.

### Persistent states: fill, not an outline shift

Use the same rule on desktop, web and Android. A state must be apparent at rest, without hover, focus or
animation, and without relying on hue alone.

- **Icon toggles:** Off is the quiet Lucide outline in `ink-3`. On keeps the same silhouette and fills its
  closed shape: `important-fill` gold for the Important star, warm `sun-ink` amber for the My Day sun, and
  `danger` red for a true flag toggle. Fill only the sun's disc; its rays stay strokes. A flag used as a
  field label and a pin used as decoration are not toggles. The neutral `!` / `!!` / `!!!` priority scale
  stays separate. The row star and the detail star show and edit the same high-priority state. Where
  the task-actions menu offers a toggle, its icon shows the same current state as the toolbar.
- **Selected rows, chips, segments and options:** Use a filled or tinted container (`wash`, or the stronger
  fill for a compact segment) and a stronger label. Selected navigation and task rows also get a 3 px
  leading selection bar. Preset, tag and list choices retain their tick; for colour-only swatches the tick
  takes the place of the stronger label. A chip that just identifies a list or tag isn't selected by being
  coloured. A suggestion offered as a one-shot action isn't a toggle. Checkboxes keep their filled disc and
  tick when checked, a bare ring when unchecked; a switch needs a filled track **and** a moved thumb.
- **Contrast and focus:** Both the off outline and the on glyph must reach 3:1 on the surfaces they sit on,
  in light and dark, including a selected row. Keep labels at 4.5:1. A tinted background alone may be subtle;
  pair it with a contrast-checked tick, bar or equivalent state mark. Keyboard focus adds a distinct ring
  or edge and never changes the state; both indicators can be visible at once.
- **Motion:** Crossfade the outline and fill over `CHECK_FILL` with `STANDARD`. On activation, scale the fill
  from `TOGGLE_FILL_FROM_SCALE` to 1 with `POP` over the same duration; don't resize the hit target. With
  reduced motion use only the fade (`REDUCED_FADE`), no scaling. The resting state must work without either.
- **Accessibility:** Keep the toggle's accessible name stable and expose its state (`aria-pressed` on web,
  `toggleable` on Compose). A tooltip can describe the action that will happen next. Use selected/current
  semantics for navigation and choices, checked for completion, and expanded for disclosure. A chevron
  opening content isn't a persistent pressed state.

## Type

Two families share one visual language. Both are OFL licensed and bundled as static instances with full
Czech coverage. The files and their sources are in `assets/fonts/`.

| Family | Role | Examples |
|---|---|---|
| **Geist** | room titles, eyebrows, empty states and all interface text | desktop room title at 600, 42/48, −0.04em; upright accent eyebrow at 500, 13/22; task titles at 500, 15/20; detail title at 600, 21/28 |
| **Geist Mono** | content you compare or scan as data | due times, counts, tags (`#tax`), keycaps, priority and `CAPS LABELS` at +0.12em |

Size, weight and spacing give Geist its hierarchy without changing family: the room title is compact and
semibold, while the empty-state line is regular and uses `ink-2`. Geist Mono keeps a visible distinction
between prose and metadata, and its tabular figures let numbers line up. An eyebrow pairs the two at a
glance: **Collection** `12 LISTS`.

`todo_design::typography` holds the full scale, with desktop and phone variants. Don't add sizes ad hoc: add a
named style.

## Grid, columns and density

Everything sits on a **4 px grid**. Text and strokes may use half steps, but layout never does. The spacing steps
are 4, 8, 12, 16, 20, 24, 32 and 40.

**The desktop window** is laid out at the reference size of 1440 × 900:

- three panes at default widths: sidebar **248**, list **800** (flexible) and detail **392**;
- a 36 px title bar and a 28 px status bar. Windows caption buttons are 46 px wide with matching 10 px
  glyphs and 1 px optical strokes; the close button uses the Windows red on hover;
- 32 px horizontal padding in the list column;
- a quiet 6 px scrollbar thumb in the edge gutter of each overflowing pane. It widens and gains contrast
  on hover or drag, without covering row content.

**A narrow desktop window** (below `desktop::THREE_PANE_MIN`, **1200**, as on the web) keeps the list at its full
width. The detail pane opens over the right part of the list, with the overlay shadow and a scrim over the rest
of it (the shared `elevation::scrim`: room ink at 16% in light mode, black at 40% in dark mode).
Clicking the scrim or pressing Esc closes it. The room title keeps its full size, and the done and open counts
move to a line of their own under it, so the title never gives way to them.

**Desktop and web pane dividers.** Drag the docked detail’s left edge, or the sidebar’s right edge on
desktop. The 12 px hit strip (`desktop::PANE_HANDLE_HIT`) starts 1 px before the divider and extends into
the pane on its right, leaving both the sidebar and list scrollbar tracks clear. Only the 1 px hairline
is visible at rest. Hover shows a
column-resize cursor and fades in a centred **3 px** bar (`desktop::PANE_HANDLE_HIGHLIGHT`) in the room’s
`accent` over `HOVER` (`90 ms`, `STANDARD`, also in reduced motion). The bar stays visible while grabbed,
even if the pointer leaves the strip. During a drag the divider follows the pointer without easing, in
whole logical pixels; the bar fades out over the same duration when neither hovered nor grabbed.

- **Widths (logical px):** sidebar **200–320** on desktop, default **248** (fixed on web); docked detail
  starts at **320**, defaults to **392**, and can grow until the list reaches **400**
  (`desktop::LIST_MIN_BESIDE_DETAIL`, excluding divider hairlines). Without docked detail the desktop
  list keeps at least **556** (`desktop::LIST_MIN`). Below
  that room width, the header counts move below the title even in a wide window. The **1200** docking
  breakpoint is fixed: a narrow window never docks, and a wide window never switches to an overlay just
  because the chosen panes are wide. If both preferences won’t fit, the docked detail gives up width first
  and recovers it as room becomes available. Narrow desktop keeps the sidebar handle; the detail overlay
  keeps its default width and has no handle.
- **Reading width:** In a wide detail pane, left-align the title, notes and step text within the normal
  pane padding. Cap their line length at **72 ch** (`desktop::DETAIL_TEXT_MEASURE`) in each text style.
  Let long step titles wrap when there is room for that measure. Keep step progress and actions near
  their text; fields, links and attachments can use the full width.
- **Keys and focus:** Tab reaches the dividers, which expose their name and current width as vertical
  splitters. Left and Right move the divider in that direction by **8** px (**32** with Shift). Keyboard
  focus adds an `accent` line and the **3 px** `tint` ring, following the shared focus fade. Esc returns
  focus to the list.
- **Reset:** Double-click the divider or press Home or Enter while it has focus. The width settles to its
  default (or as much as fits). On desktop it settles in `OVERLAY_IN` (**150 ms**, `STANDARD`), and snaps
  with reduced motion; a new grab or width change can interrupt it. On web it snaps, and End makes the
  detail as wide as possible. Idle dividers request no frames.
- **Remembered locally:** Save both preferred widths with the desktop pairing’s UI preferences, and the
  docked detail preference in the browser on web. Passive window resizing never writes them. Save
  arrow-key changes when a key is released or the divider loses focus, not on every repeat. On load,
  clamp the sidebar to its limits and the detail to at least its minimum. The live window, not the saved
  detail preference, sets the maximum. The medium and phone web overlays have no divider.

**Sidebar order.** The desktop and web sidebars and the Android drawer list the rooms in one order, top to
bottom:

1. the smart lists: **My Day**, **Important**, **Planned**, **All**, **Completed**, **Agent activity**;
2. **Activity**, a row like the smart lists' (Clock icon in the `agent-activity` hue; the desktop adds its
   **Ctrl H** hint), then indented sub-rows for up to four agents that added or edited tasks today, since
   local midnight. An agent without activity today has no row. There is no "Agents" or "Today" heading:
   each agent's status says it ("2 added today", "2 added, 1 edited today");
3. **Trash**, a single row with the Trash icon in the `agent-activity` hue, after Activity and its agents;
4. the collections, each with its lists;
5. the lists outside any collection;
6. One pinned footer row below a hairline: **+ New** on the left, the sync status in the middle, and the
   settings gear on the right. **+ New** opens a menu with **List** and **Collection**, their icons and any
   existing shortcut. Enter or Space opens the menu, arrows move between choices, and Escape closes it and
   returns focus to **+ New**. Sync shows its coloured dot and a short label (for example, **20:18**,
   **Syncing…**, or **Offline · 3**); a tooltip or accessible description keeps the full wording. At the
   minimum sidebar width, only the sync label can truncate. **Settings** uses the same menu keys. It
   shows the server above **Open admin pages** and **Sync now**. Its **Devices…** item sits above
   **Sign out** (Android: **Unpair this phone**), opens the device pairing dialog, and returns focus
   to the gear when the dialog closes.

Within the sidebar, keyboard navigation through focusable controls follows their order on screen. Desktop
room rows are not Tab stops.

**Trash.** The sky and hills use the Activity palette, without a sun. A short line explains where items
return. Use a flat list with a kind icon, title and deletion metadata, plus a quiet **Restore** button.
For an item whose original parent is deleted, replace the button with a line under the metadata that
says what to restore first. There is no task composer, filter or detail pane. Restore uses the same
reserved Undo strip as other changes.

**Desktop rows** are 56 px tall on a fixed grid with 12 px gaps. The columns are checkbox, title (with the
metadata line under it), agent mark, priority, due date and the Important toggle.

- The agent, priority and due columns keep their places from the right edge, so filled ones line up down the
  list. Empty columns to the left of every filled one are left out, so the title runs on into their room
  instead of stopping short. A row without a date keeps only the dash.
- A long title truncates with an ellipsis before the agent column. Test this with a long Czech title, for example
  *Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení*.
- Below **556** px of room width, the metadata line starts under the title as usual, but can use the empty
  space beneath the trailing columns. It reads list chip, steps, source mark (mail), then tags, so the compact
  indicators are never the ones pushed out. A list chip is at most **180** wide (`desktop::CHIP_MAX`) and ends in an ellipsis inside the
  pill. Tags that don't fit whole collapse into a mono `+N`; nothing is ever cut mid-glyph. In a narrow column the
  items give way in order (tags, then the mail mark, then the step meter, then the step count) and the chip narrows
  last, down to its dot and an ellipsis.

**Phone rows** are 64 dp tall: `22 | 1fr | 20 | 24`, followed by the Important touch target. The metadata can
use the space below the agent and priority columns. As on the desktop, those columns keep their
places from the right edge, and empty ones to the left of every filled one are left out, so the title uses their
room. On a dated row the due date comes first, in a 100 dp slot, so the list chips of dated rows start at the
same x. On Android an undated row has no slot: its chips start flush with the title, because a blank 100 dp gap
reads as missing content. On the web, if any visible row has a date, undated rows also reserve the slot so the
chips stay aligned; if none does, chips start flush with the title. Web phone chips cap at 120 px
(`phone::CHIP_MAX`) and trailing tags can end with an ellipsis. A cover screen fits nine full rows above the
composer. A row is at least 64 dp and grows with the font scale, so its metadata line is never clipped.

**The phone header gives way to the tasks.** As the list scrolls up, the header (eyebrow, big title, landscape)
and the room pills slide away under a 48 dp bar, and the room's name fades into the bar in `room-title-bar`.
Scrolling back to the top brings them back. While the keyboard is open, or when the window is short (under
480 dp), the header stays collapsed, so the list keeps room for at least four rows above the composer.

**Every phone target is at least 48 dp** (`touch`), even where the visual is smaller. A drawer row's highlight is
40 dp (`nav-row`) and a room pill is 32 dp, but each sits in a 48 dp target, and the press ripple stays on the
visual.

**The detail pane's top bar** holds the breadcrumb on the left and three actions on the right, in this order on
every client: **My Day** (sun), **Important** (star), **Close** (×). Android and web also have **Task actions**
(⋯) right after the star. On Android's cover screen Back replaces Close. The breadcrumb takes what's left and ends
in an ellipsis; the actions never move, however long the list's name.

**Task detail activity** sits below Attachments on each client. A quiet heading and hairline introduce a flat list:
actor mark at the leading edge, actor and action beside it, and the relative time in mono at the trailing edge.
The mark repeats the actor already named in text. This is a read-only summary, not a second event feed.

**Undo never covers a row.**

- On the desktop, the bottom of the list reserves a 56 px inset. "Completed 3" sits on the left and the toast on
  the right.
- On phones, a 56 dp strip above the single 48 dp composer holds the toast.

**Pills are rationed.** Only three things are pill-shaped: list chips (a 6 px dot, a 6 px gap and 8 px padding),
the overdue badge and phone room pills. Tags are plain mono `#tag` text. Suggestions get a compact chip with a
6 px radius, not a banner.

## Radii

| Element | Radius |
|---|---|
| checkbox, agent mark | circle |
| list chip, overdue badge | 10 (pill) |
| phone room pill | 16 (pill) |
| quick-add, toast, popover, card | 10 |
| button, suggestion chip, icon button, sidebar item | 6 |
| keycap | 4 |
| phone composer | 12 |

The round checkbox belongs to Playful. Everything else keeps the shared small radii, so inputs, feedback and chips
stay distinct from each other.

## Elevation

The design is **flat**. Surfaces are separated by colour steps and 1 px lines.

- A focused input gets an `accent` border plus a 3 px ring in `tint`.
- There is exactly one shadow, a soft two-layer one for transient overlays: menus, popovers and the date picker.
  In dark mode it comes with a 1 px `line-strong` border.
- Nothing is blurred.

## Motion

Motion confirms what happened. It never decorates, and nothing loops or idles. Every duration and easing curve
comes from `todo_design::motion` (`--duration-*` and `--ease-*` in `tokens.css`; `motion.durationsMs` and
`motion.easings` in `tokens.json`). Clients never invent their own numbers.

### Principles

- **Calm.** The pace belongs to Daybreak: crisp on controls (90–200 ms), unhurried on the sky and sun
  (360–1200 ms). Nothing spins, pulses, breathes, or bounces for attention. Checkbox and icon-toggle fills
  (`POP`) are the only overshoots in the app.
- **Purposeful.** Motion answers one of three spatial questions: *where did it come from?*, *what changed?*, or
  *where did it go?*. If a state change is already unmistakable without movement, it snaps.
- **Never blocks input.** Commands apply optimistically on the first frame. Animations play alongside input and
  never disable clicks, typing, or keyboard navigation (`J`/`K`, `Space`, `Ctrl+N`, `Ctrl+K`, `Ctrl+Z`).
- **Interruptible.** Every in-flight transition retargets smoothly from its current interpolated value instead
  of jumping to the end or restarting from zero:
  - re-clicking a checking task cancels the hold/collapse and reverses immediately;
  - rapid completions retarget the sun smoothly from its current arc position to the latest count;
  - rapid room switches retarget the sky hue from its current intermediate hue;
  - rapid `J`/`K` selection cancels in-flight detail crossfades and renders the newly selected task at once.
- **Spatially consistent.** Elements keep a stable mental map: the header stays anchored while its sky and title
  update; completed rows leave downward toward the "Completed" drawer; the detail pane belongs to the right edge
  on desktop and pushes forward from the right on phone; menus and popovers open from their anchor edge.
- **Respects reduced motion on every platform.**
  - **Web:** `@media (prefers-reduced-motion: reduce)` and `window.matchMedia('(prefers-reduced-motion: reduce)')`
    (checked before starting any View Transition).
  - **Android (Compose):** `Settings.Global.ANIMATOR_DURATION_SCALE == 0f` (and `LocalMotionDurationScale`).
    Intermediate OS scales (e.g. `0.5×`) scale Compose animations automatically; at `0×`, apply the exact
    reduced-motion fallbacks below rather than leaving bare 0 ms flashes.
  - **Windows (GPUI):** `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, ...)` (Settings → Accessibility →
    Visual effects → Animation effects), re-checked on `WM_SETTINGCHANGE`.
  - **The rule (`motion::reduced`):** Every spatial movement, scale, rotation, and stagger becomes `0 ms`
    (instant). Pure opacity and colour fades shrink to at most `REDUCED_FADE` (`100 ms`). Functional timers
    (`COMPLETE_HOLD`, `SCROLLBAR_HOLD`, `UNDO_WINDOW`) are not animations and keep their full duration. Every
    moment in the catalogue below states its exact reduced-motion fallback.

### Tokens

#### Easing curves

| Rust (`motion::`) | CSS / JSON key | Curve | Usage |
|---|---|---|---|
| `STANDARD` | `standard` | `cubic-bezier(0.2, 0, 0, 1)` | Default for state-to-state transitions: tick draw, panels, collection expand/collapse, sibling reorder, hover and focus. |
| `ENTER` | `enter` | `cubic-bezier(0, 0, 0.2, 1)` | Fast start, calm landing for arriving elements: new rows, toasts, menus, dialogs, drawers. |
| `EXIT` | `exit` | `cubic-bezier(0.3, 0, 1, 1)` | Gentle start, brisk departure: collapsing rows, dismissing toasts and overlays, fading scrollbars, sync highlight fade-out. |
| `POP` | `pop` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | Checkbox disc fill only. **The single overshoot curve in the app.** |
| `SUN` | `sun` | `cubic-bezier(0.45, 0, 0.55, 1)` | Symmetric, celestial ease for the sun's arc travel, room sky hue morphs, and the dusk transition. |

Linear timing (`linear`) is reserved for literal time remaining (the undo countdown bar) and 1:1 pointer or
touch tracking; it has no custom curve token.

#### Durations and timers

| Rust (`motion::`) | CSS / JSON key | Value | Kind | Usage |
|---|---|---|---|---|
| `HOVER` | `hover` | 90 ms | animation | Hover, press release, focus ring, selection bar, scrollbar appear. |
| `CONTENT_SWAP` | `content-swap` | 120 ms | animation | Fast in-place crossfade: switching tasks in an open detail pane, room title/eyebrow swap, sync status label/dot change. |
| `ROOM_TITLE_OVERLAP` | `room-title-overlap` | 40 ms | offset | Overlap between outgoing and incoming room title/eyebrow. |
| `OVERLAY_OUT` | `overlay-out` | 120 ms | animation | Menus, popovers, date pickers, dialogs, and quick-add date chips dismissing. |
| `CHECK_FILL` | `check-fill` | 140 ms | animation | Checkbox disc filling with `accent` (`POP`). |
| `TOAST_OUT` | `toast-out` | 140 ms | animation | Toast leaving. |
| `OVERLAY_IN` | `overlay-in` | 150 ms | animation | Menus, popovers, date pickers and dialogs entering. |
| `CHECK_TICK_DELAY` | `check-tick-delay` | 60 ms | offset | Delay after check press before the tick starts drawing. |
| `CHECK_TICK` | `check-tick` | 180 ms | animation | Tick stroke drawing left to right; title dimming and strike-through. |
| `ROW_ENTER` | `row-enter` | 180 ms | animation | New or reopened task row entering a list; sibling rows shifting during reorder. |
| `TOAST_IN` | `toast-in` | 180 ms | animation | Toast entering; inline pairing error or status note appearing. |
| `PANEL` | `panel` | 200 ms | animation | Detail pane, sidebar collections, pairing steps, Android drawer/sheet, predictive-back and shared-element settles. |
| `ROW_COLLAPSE` | `row-collapse` | 220 ms | animation | Completed row collapsing and fading out of the open list. |
| `SCROLLBAR_OUT` | `scrollbar-out` | 300 ms | animation | Overlay scrollbar fading out once idle. |
| `ROOM_HUE` | `room-hue` | 360 ms | animation | Header sky, hills and tinted room palette morphing between room hues (`SUN`); reversing dusk back to daytime. |
| `SUN_TRAVEL` | `sun-travel` | 700 ms | animation | Sun gliding along its arc and travelled stroke growing (`SUN`). |
| `SYNC_HIGHLIGHT` | `sync-highlight` | 900 ms | animation | Quiet `wash` → `pane` background fade on a row modified by live remote sync or an agent. |
| `DUSK` | `dusk` | 1200 ms | animation | My Day finishing: sky crossfade to dusk gradient, header growth to 164 px, sun setting behind hills, moon and stars appearing (`SUN`). |
| `STAGGER` | `stagger` | 25 ms | offset | Per-item delay for small entering groups, capped at `STAGGER_MAX_ITEMS` (5 items / 125 ms max total). |
| `REDUCED_FADE` | `reduced-fade` | 100 ms | cap | Longest opacity or colour fade permitted under reduced motion. |
| `COMPLETE_HOLD` | `complete-hold` | 450 ms | timer | Pause after the tick lands so the eye sees the struck-through row before it exits. Unchanged under reduced motion. |
| `SCROLLBAR_HOLD` | `scrollbar-hold` | 1000 ms | timer | Idle wait after scrolling stops before an overlay scrollbar fades out. Unchanged under reduced motion. |
| `UNDO_WINDOW` | `undo-window` | 6000 ms | timer | Undo toast lifetime and linear countdown line (`Ctrl+Z` still works afterwards). Unchanged under reduced motion. |

### Catalogue of moments

#### 1. Completing (and reopening) a task

Completing a task is the one choreographed moment in the app.

- **Trigger:** Clicking a task's checkbox or pressing `Space` on a selected row (or reopening from "Completed").
- **Choreography:**
  1. **Fill (`CHECK_FILL` 140 ms, `POP`):** The circular disc fills with `accent` (or `check-selected` on a
     selected `wash` row) and scales `0.55 → 1.0`, overshooting slightly (~`1.06`) before settling at `1.0`.
  2. **Tick & title (`CHECK_TICK_DELAY` 60 ms offset, then `CHECK_TICK` 180 ms, `STANDARD`):** The checkmark
     draws from left to right; simultaneously the task title fades to `ink-3` and acquires a strike-through.
  3. **Hold (`COMPLETE_HOLD` 450 ms timer):** After the tick finishes (`CHECK_TICK_DELAY + CHECK_TICK +
     COMPLETE_HOLD` total before exit), the struck-through row stays stationary so the eye confirms it landed.
     Clicking the checkbox again or pressing `Ctrl+Z` cancels the timer and reverses at once.
  4. **Exit & settle (`ROW_COLLAPSE` 220 ms, `EXIT`):** The row fades (`opacity: 1 → 0`) and collapses its
     height (`row → 0`) so rows below close the gap. If "Completed" is currently expanded below, the task enters
     at the top of Completed (`ROW_ENTER` 180 ms, `ENTER`, `height: 0 → row`, `opacity: 0 → 1`); otherwise the
     "Completed *n*" toggle updates its count when the row finishes leaving.
  5. **Sun advance (`SUN_TRAVEL` 700 ms, `SUN`):** Starts once the row has left, together with the header
     count, so the count and the sun confirm the row's departure rather than racing it. The sun disc, halo and
     rays glide along the arc from the old ratio to the new ratio while the travelled stroke grows in lockstep.
     Rapid completions retarget smoothly from the sun's current progress.
- **Reopening:** Checkbox clears (`OVERLAY_OUT` 120 ms, `EXIT`), row exits Completed (`ROW_COLLAPSE` 220 ms,
  `EXIT`), enters the open list at its sorted slot (`ROW_ENTER` 180 ms, `ENTER`), and the sun glides backwards
  along its arc (`SUN_TRAVEL` 700 ms, `SUN`).
- **Stagger:** None.
- **Reduced motion:** Checkbox switches instantly (`0 ms`, no overshoot or stroke draw); title dims in
  `REDUCED_FADE` (`100 ms`); `COMPLETE_HOLD` (`450 ms`) **stays unchanged**; the row fades out in
  `REDUCED_FADE` (`100 ms`) and snaps out of layout at the end (no height slide); the sun jumps immediately
  (`0 ms`) to its new position.
- **Platform notes:**
  - *GPUI:* Frame-driven from a pure motion model (`apps/desktop/src/model/motion.rs`) sampled each frame:
    the disc scales with `POP`, the tick and the title's strike-through are clipped from the left
    (`STANDARD`). Because `uniform_list` uses fixed row height, the leaving row collapses inside its slot and
    the rows below are drawn up by the share that has gone; the slot leaves the virtualized list only when
    the row is released. For the sun, interpolate a scalar `progress: f32` and evaluate `Landscape::sun` /
    `travelled` only while animating, returning to the cached paths as soon as the animation settles.
  - *Compose:* Frame-driven from the same pure motion model as GPUI (`apps/android/.../model/Motion.kt`),
    sampled at each frame's time: a leaving row is laid out at its share of its height inside the
    `LazyColumn`, so the rows below rise with it, and it leaves the list only when released. The sun's
    `progress` is read in the header `Canvas`'s draw, over geometry built once per size.
  - *Web:* Animate checkbox scale with `transform` and tick with `clip-path` or SVG `stroke-dashoffset`;
    collapse rows via a CSS `grid-template-rows: 1fr → 0fr` wrapper; animate the sun `<g>` and arc
    `stroke-dashoffset` over `var(--duration-sun-travel) var(--ease-sun)`.

#### 2. The day finishing (dusk)

- **Trigger:** Completing the last open task in My Day, beginning as its `ROW_COLLAPSE` finishes and open count
  reaches `0`.
- **What moves & properties (`DUSK` 1200 ms, `SUN`):**
  - Header landscape height grows smoothly from its everyday height (`48 px` desktop; `36 dp` / `40 dp` phone)
    to `164 px` (`HEIGHT_FINISHED`).
  - Sky crossfades from room `sky` fill to the `dusk.top → dusk.low` vertical gradient; far and near hills
    crossfade to `dusk.silhouette-far` and `dusk.silhouette-near`. At ~40 % progress, header text switches cleanly
    to dark-theme ink over the dusk sky.
  - The sun descends along the final arc segment behind the hills while its three concentric rings expand subtly
    (`scale: 0.88 → 1.0`, `opacity: 0 → ring opacity`).
  - From 40 % to 100 % of `DUSK`, the moon and stars fade in (`0 → MOON_OPACITY` / `STAR_OPACITY`), with stars
    grouped into up to 5 batches staggered by `STAGGER` (`25 ms`).
  - Starting at 65 % of `DUSK`, the "When the sun comes up" tomorrow handoff section enters (`ROW_ENTER` 180 ms,
    `ENTER`, `opacity: 0 → 1`, `translateY: 6 px → 0`).
- **Reversing dusk:** Reopening a task or adding a new task to My Day returns the header to the daytime strip
  over `ROOM_HUE` (`360 ms`, `SUN`) — brisker than entering dusk so getting back to work never feels delayed.
- **Stagger:** Up to `STAGGER_MAX_ITEMS` (5) rows in the tomorrow handoff and 5 star groups, `STAGGER` (`25 ms`)
  apart (`125 ms` max total).
- **Reduced motion:** Header height switches instantly (`0 ms`); sky gradient, silhouettes, setting sun, moon,
  stars, and tomorrow handoff crossfade in `REDUCED_FADE` (`100 ms`), with `0 ms` stagger and `0 px` translation.
- **Platform notes:**
  - *GPUI:* Drive the transition from a single scalar `t: 0..1`, interpolating container height and alpha-blending
    cached `Scene::Slim` and `Scene::Finished` layers (plus lerped hill/sky fills) without rebuilding geometry per
    frame once settled.
  - *Compose:* The motion model's dusk phase drives the header's height (in layout) and the strip and dusk
    scenes' opacities (in the `Canvas`'s draw), with geometry built once per scene. The strip's sun travels
    from where it is to the end of its arc (`SUN_TRAVEL`, `SUN`) as the strip fades; with reduced motion
    it's there at once.
  - *Web:* Transition header height and layered sky/SVG opacity (avoid repaints on body text by keeping the
    tomorrow handoff below the header).

#### 3. Adding a task and resetting quick-add

- **Trigger:** Submitting a new task from inline quick-add (`Enter`) or global quick-add (`Ctrl+Alt+Space`).
- **What moves & properties:**
  - **Quick-add reset:** Input text clears immediately (`0 ms`). Inline quick-add keeps focus so rapid entry
    never pauses; any parsed date or priority preview chip fades out (`OVERLAY_OUT` 120 ms, `EXIT`,
    `opacity: 1 → 0`, `scale: 1.0 → 0.96`). The global quick-add window dismisses over `OVERLAY_OUT` (`120 ms`,
    `EXIT`, `opacity: 1 → 0`, `translateY: 0 → -4 px`).
  - **Row entry (`ROW_ENTER` 180 ms, `ENTER`):** The new row enters at its sorted insertion slot (`height: 0 →
    row`, `opacity: 0 → 1`, `translateY: -4 px → 0`). If the task lands mid-list (for example, adding a dated
    task in Planned), scroll it smoothly into view and settle its background from `wash → pane` over `ROOM_HUE`
    (`360 ms`, `EXIT`) so the eye finds it immediately.
  - **Empty list → first task:** The empty-state line fades out (`OVERLAY_OUT` 120 ms, `EXIT`),
    and the sun and arc fade into the header (`ROW_ENTER` 180 ms, `ENTER`).
  - **Sun update:** If the room already has completed tasks, the sun eases to the updated `done / total` ratio
    (`SUN_TRAVEL` 700 ms, `SUN`).
- **Stagger:** None.
- **Reduced motion:** Row snaps into layout (`0 ms` height/translation) with `REDUCED_FADE` (`100 ms`) opacity;
  global quick-add closes instantly (`0 ms`); sun snaps (`0 ms`).
- **Platform notes:**
  - *GPUI:* `uniform_list` inserts at full row height and animates `opacity: 0 → 1` over `ROW_ENTER`; global
    quick-add hides immediately on `Esc` or submit if OS window opacity animation would delay focus return.
  - *Compose:* `LazyColumn` `animateItem(fadeInSpec = tween(ROW_ENTER), placementSpec = tween(ROW_ENTER))`.
  - *Web:* CSS `@keyframes` on opacity/transform inside a `grid-template-rows: 0fr → 1fr` wrapper.

#### 4. Switching rooms or lists

- **Trigger:** Clicking a smart list, collection, or list in the sidebar, tapping a phone room pill, or switching
  rooms by keyboard.
- **What moves & properties:**
  - **Sky and room hue morph (`ROOM_HUE` 360 ms, `SUN`):** The header sky, far hill, near hill, horizon band,
    and subtle tinted room surfaces (`canvas`, `wash`, `accent`, `line`) interpolate smoothly from the previous
    room's hue to the new room's hue along the **shortest OKLCH hue arc** (so switching from 350° rose to 20°
    coral travels +30° through red, never 330° through green). Endpoints are already contrast-guarded; mid-flight
    frames lerp directly between the two palettes.
  - **Header title and eyebrow (`CONTENT_SWAP` 120 ms):** Outgoing title/eyebrow fades out and shifts `4 px`
    vertically (`EXIT`); incoming title/eyebrow enters from `±4 px` to `0` (`ENTER`, overlapping by 40 ms). The
    summary count updates in place.
  - **Sun:** When both rooms have tasks, the sun glides along its arc from the previous ratio to the new ratio
    (`SUN_TRAVEL` 700 ms, `SUN`). When moving between an empty list (no sun) and a populated list, the sun and
    arc fade in (`ROW_ENTER` 180 ms, `ENTER`) or out (`OVERLAY_OUT` 120 ms, `EXIT`) without flying in from an
    edge.
  - **Task list (`CONTENT_SWAP` 120 ms, `ENTER`):** Crisp opacity crossfade of the list body; scroll resets
    immediately to top (`0 ms`). Never cascade individual rows on room switch.
  - **Active sidebar item / phone room pill:** The sidebar's active fill and leading mark crossfade over `HOVER` (`90 ms`,
    `STANDARD`). On phones, the active room pill crossfades colour (`ROOM_HUE` 360 ms, `SUN`) while the pill
    strip scrolls smoothly to keep it visible.
- **Stagger:** None.
- **Reduced motion:** Sky and palette colours crossfade in `REDUCED_FADE` (`100 ms`); header title and list swap
  in `REDUCED_FADE` (`100 ms`) with `0 px` translation; sun jumps instantly (`0 ms`).
- **Platform notes:**
  - *GPUI:* Animate a scalar `t: 0..1` on room change and lerp `Palette` fields at paint time — the cached hill
    and arc paths resolve their `Ink` colours dynamically at paint time and never need rebuilding for a hue morph.
  - *Compose:* `animateColorAsState(target, tween(ROOM_HUE, easing = SunEasing))` for sky, hills, and room tokens.
  - *Web:* Switching `data-room-hue` transitions CSS variables / `background-color` and SVG `fill` over
    `var(--duration-room-hue) var(--ease-sun)`, optionally paired with the View Transitions API (see below).

#### 5. Detail pane: opening, closing and switching tasks

- **Trigger:** Selecting a task (open), clicking `×` or pressing `Esc` (close), or choosing another task while
  the detail pane is open (switch).
- **What moves & properties:**
  - **Open (`PANEL` 200 ms, `ENTER`):**
    - *Desktop wide (docked column at the current width):* The empty placeholder fades out (`CONTENT_SWAP` 120 ms, `EXIT`) and
      the detail content enters (`PANEL` 200 ms, `ENTER`, `opacity: 0 → 1`, `translateX: 8 px → 0`).
    - *Web drawer / phone:* Slides in from the right edge (`translateX: 100 % → 0` on phone, `24 px → 0`
      overlay on the web) with `opacity: 0 → 1` (`PANEL` 200 ms, `ENTER`).
    - *Desktop narrow:* The overlay pane enters like the other desktop overlays (`OVERLAY_IN` 150 ms, `ENTER`,
      `translateX: 24 px → 0`, `opacity: 0 → 1`), while its scrim fades in over the same time.
  - **Close (`PANEL` 200 ms, `EXIT`):** Reverse of open (`opacity: 1 → 0`, `translateX: 0 → 8 px` or slide out
    right). The narrow desktop overlay and its scrim leave over `OVERLAY_OUT` (120 ms, `EXIT`).
  - **Switching tasks while open (`CONTENT_SWAP` 120 ms, `STANDARD`):** In-place opacity crossfade of the detail
    body (`opacity: 0.25 → 1.0`) with zero horizontal slide, keeping the top action bar anchored. **Key-repeat
    rule:** if another task is selected within `CONTENT_SWAP` (holding `J`/`K` or arrow keys), cancel the fade
    and paint the new task immediately (`0 ms`).
  - **Row selection bar:** The `3 px` left `accent-strong` bar and `wash` fill crossfade over `HOVER` (`90 ms`,
    `STANDARD`).
- **Stagger:** None.
- **Reduced motion:** Open, close, and switch drop all translation and width slides, using `REDUCED_FADE`
  (`100 ms`) opacity crossfade (`0 ms` on key-repeat).
- **Platform notes:**
  - *GPUI:* Keep the docked column's current width constant during task navigation so the list never reflows
    while the detail content fades and shifts. Only deliberate divider resizing (including reset) changes its
    width while it remains docked.
  - *Compose:* `AnimatedContent` for in-place tablet task switches; `NavHost` slide or `SharedTransitionLayout`
    on phones (see Android transitions below).
  - *Web:* CSS `transform`/`opacity` drawer on narrow viewports; View Transitions API optional on route change.

#### 6. Collapsing and expanding sidebar collections

- **Trigger:** Clicking a collection header or chevron, choosing Collapse/Expand from its context menu, or
  navigating to a list inside a collapsed collection.
- **What moves & properties:**
  - **Chevron:** Rotates `0° ↔ -90°` over `PANEL` (`200 ms`, `STANDARD`).
  - **Child list group:** Height animates `0 ↔ natural height` with clipping (`overflow: hidden`) and
    `opacity: 0 ↔ 1` over `PANEL` (`200 ms`, `ENTER` on expand, `EXIT` on collapse).
  - **Hidden active list indicator:** When collapsing a collection that contains the active list, the collection
    header takes on the selected fill and leading mark over `HOVER` (`90 ms`, `STANDARD`).
- **Stagger:** When expanding a collection of ≤ `STAGGER_MAX_ITEMS` (5) lists, child rows may stagger opacity by
  `STAGGER` (`25 ms`); collapse always uses `0 ms` stagger so closing feels immediate.
- **Reduced motion:** Chevron snaps (`0 ms`); child rows snap in/out of layout (`0 ms` height, `0 ms` stagger,
  optional `REDUCED_FADE` `100 ms` opacity on expand).
- **Platform notes:**
  - *GPUI:* The motion model drives a clipped group of fixed-height sub-rows (`desktop::NAV_SUB_ROW`), its
    opacity and the chevron angle. Closing rows are paint-only; reversing samples the current height and angle.
  - *Compose:* `AnimatedVisibility(enter = expandVertically(tween(PANEL)) + fadeIn(...), exit =
    shrinkVertically(tween(PANEL)) + fadeOut(...))`.
  - *Web:* CSS `grid-template-rows: 0fr ↔ 1fr` with `overflow: hidden` on the inner track — no JS height
    measurement needed.

#### 7. Hover, press states and focus rings

- **Trigger:** Pointer enter/leave, pointer down/up, keyboard focus (`Tab`, shortcuts).
- **What moves & properties:**
  - **Hover (`HOVER` 90 ms, `STANDARD`):** Background (`pane → wash`), border (`line → line-strong`), and icon
    or text colour shift smoothly. Never scale rows or cards on hover.
  - **Press (active):** Background deepens immediately on desktop pointer-down (`0 ms` for tactile immediacy; up
    to `60 ms` touch press) and releases over `HOVER` (`90 ms`, `STANDARD`). Primary buttons and small icon
    buttons may dip subtly (`scale: 1.0 → 0.98`, `0 ms` down, `HOVER` `90 ms` `STANDARD` release); task rows
    never scale on press.
  - **Focus ring (`HOVER` 90 ms, `STANDARD` in / `EXIT` out):** The `3 px` `tint` band and `accent` border fade
    in on keyboard focus (`:focus-visible`) and fade out on blur. Never animate ring width from `0 px` to `3 px`.
- **Stagger:** None.
- **Reduced motion:** Remove button press scale (`1.0` constant); hover and focus colour fades remain at `HOVER`
  (`90 ms`, already under `REDUCED_FADE`).
- **Platform notes:**
  - *GPUI:* Instant hover styles are acceptable on virtualized rows when avoiding extra animation bookkeeping;
    apply `HOVER` colour transitions on persistent chrome (sidebar items, buttons, quick-add focus ring).
  - *Compose:* Replace Material's default ripple with a subtle `wash`/`tint` `Indication` driven by `HOVER`.
  - *Web:* Use `:focus-visible` and transition only `background-color`, `border-color`, `color`, and `box-shadow`
    with `var(--duration-hover) var(--ease-standard)`.

#### 8. Scrollbar fading

- **Trigger:** Scrolling a pane (sidebar, task list, detail pane), hovering the scrollbar gutter, or stopping
  scroll.
- **What moves & properties:**
  - **Appear (`HOVER` 90 ms, `ENTER`):** Thumb opacity `0 → 1` as soon as scroll starts or the pointer enters the
    scrollbar track.
  - **Hold (`SCROLLBAR_HOLD` 1000 ms timer):** Thumb stays visible for 1 s after the last scroll event (and stays
    visible continuously while hovered or dragged).
  - **Fade out (`SCROLLBAR_OUT` 300 ms, `EXIT`):** Thumb opacity `1 → 0`.
  - **Thumb hover / drag:** Thumb colour darkens (`ink-4 → ink-3`) over `HOVER` (`90 ms`, `STANDARD`); dragging
    tracks the pointer 1:1 with zero lag.
- **Stagger:** None.
- **Reduced motion:** Appear in `HOVER` (`90 ms`); `SCROLLBAR_HOLD` (`1000 ms`) stays unchanged; fade out caps at
  `REDUCED_FADE` (`100 ms`). Respect OS "Always show scrollbars" settings (Windows Accessibility → Visual effects)
  by keeping thumbs permanently visible and skipping fade-out.
- **Platform notes:**
  - *GPUI:* Overlay thumb painted in the scroll container's gutter so content width never shifts; driven by one
    resettable hold timer per pane. The pure motion model requests frames only while opacity or emphasis changes.
  - *Compose:* Custom `LazyColumn` scrollbar overlay driven by `LazyListState.isScrollInProgress`.
  - *Web:* Use `scrollbar-gutter: stable` and themed thin scrollbars (`scrollbar-color`) so layout width never
    jumps when scrollbars appear.

#### 9. Toasts and undo

- **Trigger:** An undoable action (completing, deleting, moving a task) or transient status feedback; expires
  after `UNDO_WINDOW` or on clicking Undo / dismissing.
- **Layout invariant:** Toasts always live inside the reserved `56 px` (desktop) / `56 dp` (phone) bottom inset
  and never cover a task row.
- **What moves & properties:**
  - **Enter (`TOAST_IN` 180 ms, `ENTER`):** `opacity: 0 → 1`, `translateY: 8 px → 0`.
  - **Countdown line (`UNDO_WINDOW` 6000 ms, `linear`):** Thin `2 px` line in `toast-ink-2` along the toast's
    bottom edge shrinks from `100 % → 0 %` (`scaleX: 1 → 0`, left-anchored). Pauses while hovered or focused,
    resuming on leave/blur.
  - **Replace:** A second undoable action updates the existing toast in place (`CONTENT_SWAP` 120 ms) and resets
    the countdown bar to `100 %`. Never stack multiple toasts.
  - **Leave (`TOAST_OUT` 140 ms, `EXIT`):** `opacity: 1 → 0`, `translateY: 0 → 6 px`.
  - **Undo clicked:** Toast leaves (`TOAST_OUT` 140 ms, `EXIT`), the restored task enters its list (`ROW_ENTER`
    180 ms, `ENTER`), and the sun glides back (`SUN_TRAVEL` 700 ms, `SUN`).
- **Stagger:** None.
- **Reduced motion:** Enter and leave drop `translateY` and fade over `REDUCED_FADE` (`100 ms`). `UNDO_WINDOW`
  (`6000 ms`) stays unchanged; the linear countdown bar remains functional (it communicates time remaining, not
  decorative motion).
- **Platform notes:**
  - *GPUI:* Rendered in the reserved bottom inset with `with_animation`.
  - *Compose:* `AnimatedVisibility` inside the 56 dp strip above the phone composer.
  - *Web:* Mounted in an `aria-live="polite"` region using CSS `transform`/`opacity` and `scaleX` on the
    countdown bar.

#### 10. Sync status changes

- **Trigger:** `SyncClient` transitioning between `Synced`, `Syncing`, `Offline`, and `Error` / `Couldn't sync`.
- **Hard rule:** **Nothing spins, pulses, or breathes.** Syncing is calm text and colour, never a rotating arrow
  or pulsing dot.
- **What moves & properties:**
  - **Status dot & label (`CONTENT_SWAP` 120 ms, `STANDARD`):** Dot colour crossfades (`success` for Synced,
    `accent` for Syncing, `ink-4` for Offline, `danger` for Error) alongside a crossfade of the status label
    (for example, `20:18`, `Syncing…`, `Offline · 2`).
  - **Anti-flicker debounce:** Wait `300 ms` before showing transient `Syncing…` so fast background round-trips
    do not blink the footer.
  - **Status-bar notice / Retry action:** Enters in place over `TOAST_IN` (`180 ms`, `ENTER`, `opacity: 0 → 1`)
    without shifting the task list above it.
- **Stagger:** None.
- **Reduced motion:** Dot and label crossfade in `REDUCED_FADE` (`100 ms`), zero movement; the `300 ms` debounce
  stays unchanged.
- **Platform notes:** All three clients observe `todo_client::SyncStatus` and share the `300 ms` transient
  debounce.

#### 11. Pairing flow

- **Trigger:** First run or post-sign-out transitions between `Enter server address`, `Starting`, `Waiting for
  approval` (user code), and the paired handoff into the main shell (or returning to `Enter` on denial/expiry).
- **Hard rule:** Nothing spins while waiting for approval — the code and plain status text carry the state.
  Invite and approval QR tiles always use dark modules on the light My Day pane (including in dark mode),
  a four-module quiet zone from the shared encoder, and the `component::PAIRING_QR` size. Render modules
  on whole device pixels or with SVG crisp edges. The countdown uses the functional `PAIRING_TICK` timer,
  independent of animation preferences.
- **What moves & properties:**
  - **Step transitions:** Outgoing step content fades out and shifts `12 px` opposite the direction of travel
    (`OVERLAY_OUT` 120 ms, `EXIT`); incoming step fades in from `±12 px` to `0` (`PANEL` 200 ms, `ENTER`). The
    Daybreak header landscape stays anchored above the column throughout.
  - **Error or transient poll note:** Reveals smoothly below the input or code (`TOAST_IN` 180 ms, `ENTER`,
    `opacity: 0 → 1`, `height: 0 → natural`).
  - **Paired → main shell:** Pairing column fades out (`PANEL` 200 ms, `EXIT`); the main shell panes fade in
    (`PANEL` 200 ms, `ENTER`) while the header sky settles into My Day (`ROOM_HUE` 360 ms, `SUN`).
- **Stagger:** None.
- **Reduced motion:** Horizontal step shifts become `0 px`; step swaps, notes, and the shell handoff crossfade in
  `REDUCED_FADE` (`100 ms`).
- **Platform notes:**
  - *GPUI:* Keeps the pairing header landscape mounted while swapping the centred column (`src/ui/pairing.rs`).
  - *Compose:* `AnimatedContent` with `fadeIn + slideInHorizontally`.
  - *Web:* Route/state swap with CSS transitions or View Transitions API.

#### 12. Dialogs, menus and popovers

- **Trigger:** Opening or dismissing a context menu, overflow menu, settings menu, date picker popover, or
  confirmation dialog (delete list, "Start fresh").
- **What moves & properties:**
  - **Anchored menus & popovers:**
    - *Enter (`OVERLAY_IN` 150 ms, `ENTER`):* `opacity: 0 → 1`, `scale: 0.97 → 1.0`, and `4 px` shift away from
      the anchor edge (`transform-origin` at the anchor corner so the overlay feels attached to its trigger).
    - *Leave (`OVERLAY_OUT` 120 ms, `EXIT`):* `opacity: 1 → 0`, `scale: 1.0 → 0.98`.
  - **Modal confirmation dialogs:**
    - *Scrim:* Translucent neutral dimming layer fades `0 → 1` (`OVERLAY_IN` 150 ms, `ENTER`) and `1 → 0`
      (`OVERLAY_OUT` 120 ms, `EXIT`). **Never blur the backdrop** (see Elevation).
    - *Dialog card (opaque `pane`):* Enters with `opacity: 0 → 1`, `scale: 0.97 → 1.0`, `translateY: 6 px → 0`
      (`OVERLAY_IN` 150 ms, `ENTER`); leaves with `opacity: 1 → 0`, `translateY: 0 → 4 px` (`OVERLAY_OUT` 120 ms,
      `EXIT`).
  - **A menu's next page** (a picker, a confirmation or the colour swatches that one of its items opens in
    place): the menu stays where it is, and the new page's content fades in over `CONTENT_SWAP` (`120 ms`,
    `ENTER`).
- **Stagger:** None.
- **Reduced motion:** Drop scale and translation (`0 px`, `scale: 1.0`); scrim and overlay fade over
  `REDUCED_FADE` (`100 ms`), and so does a menu's next page.
- **Platform notes:**
  - *GPUI:* `anchored` overlays omit `scale` (keeps 1 px borders pixel-crisp at 100–150 % DPI) and animate
    `opacity` + `4 px` vertical offset over `OVERLAY_IN` / `OVERLAY_OUT`.
  - *Compose:* `DropdownMenu`, `Popup`, and `Dialog` use `scaleIn(0.97f)` + `fadeIn(tween(OVERLAY_IN))` and
    `fadeOut(tween(OVERLAY_OUT))`.
  - *Web:* Native `<dialog>` and Popover API with `@starting-style` and `transition-behavior: allow-discrete`.

#### 13. Drag-reordering (and keyboard nudge)

- **Trigger:** Dragging a task row or sidebar list to a new position, or pressing `Alt+Up` / `Alt+Down`.
- **What moves & properties:**
  - **Pickup (`HOVER` 90 ms, `STANDARD`):** Dragged row adopts `shadow-overlay` (plus `1 px` `line-strong`
    border in dark mode) and opaque `pane` fill; on touch only, scales subtly (`1.0 → 1.01`). Never scale desktop
    text during drag so subpixel rendering stays sharp.
  - **Dragging:** The dragged item follows the pointer or finger **1:1 with zero easing and zero lag**, locked to
    the vertical axis of its list.
  - **Sibling displacement (`ROW_ENTER` 180 ms, `STANDARD`):** Whenever the dragged item crosses a slot midpoint,
    displaced siblings glide smoothly to their new slot.
  - **Drop settle (`ROW_ENTER` 180 ms, `STANDARD`):** Released item glides into the target slot and sheds
    `shadow-overlay`.
  - **Keyboard nudge (`Alt+Up` / `Alt+Down`):** Swapped rows exchange positions over `ROW_ENTER` (`180 ms`,
    `STANDARD`), retargeting immediately on rapid presses.
- **Stagger:** None.
- **Reduced motion:** Pickup keeps `shadow-overlay` and an `accent` outline (drops touch scale); dragged item
  still tracks the pointer 1:1 (direct manipulation is never disabled); sibling displacement, drop settle, and
  keyboard nudges snap instantly (`0 ms`).
- **Platform notes:**
  - *GPUI:* Paint the lifted row in a top overlay layer at pointer `y`; animate sibling offsets where variable
    layout allows, or swap immediately inside `uniform_list` with a `HOVER` `wash` flash on the moved row.
  - *Compose:* `LazyColumn` `animateItem(placementSpec = tween(ROW_ENTER, easing = StandardEasing))`, paired with
    `HapticFeedbackType.LongPress` on pickup and `TextHandleMove` on slot crossing.
  - *Web:* Pointer Events driving GPU-composited `translateY` transforms during drag, committing DOM order once
    on pointer-up.

#### 14. Android transitions (Compose)

- **Navigation drawer:** Edge swipe tracks the finger **1:1**, coupling scrim opacity directly to drawer progress
  `0..1`. Release settles open (`PANEL` 200 ms, `ENTER`) or closed (`PANEL` 200 ms, `EXIT`).
- **Collapsing room header (scroll-linked):** Scrolling the task list collapses the expanded Daybreak header into
  the compact title bar **1:1 with scroll offset** (fading the landscape, date eyebrow and room-pill strip
  proportionally so nothing snaps mid-scroll); releasing midway settles cleanly to either expanded or collapsed
  over `PANEL` (`200 ms`, `STANDARD`).
- **Predictive back (Android 14+ / `PredictiveBackHandler`):** Gesture progress `0..1` continuously scales the
  outgoing surface down (`1.0 → 0.92`), rounds its corners to `radius::CARD`, and shifts it toward the swipe edge
  while revealing the previous screen underneath. Commit finishes over `PANEL` (`200 ms`, `EXIT`); cancel returns
  to `1.0` over `PANEL` (`200 ms`, `STANDARD`).
- **Shared-element transitions (`SharedTransitionLayout`):**
  - *Task row → Detail screen:* Task title, round checkbox, and list chip morph bounds from row to detail header
    over `PANEL` (`200 ms`, `STANDARD`); surrounding rows fade out (`OVERLAY_OUT` 120 ms, `EXIT`) while detail
    notes and steps fade in (`PANEL` 200 ms, `ENTER`).
  - *Phone room pill → Room header:* Room sky hue morphs (`ROOM_HUE` 360 ms, `SUN`) while title crossfades
    (`CONTENT_SWAP` 120 ms).
- **Haptics:** Restraint matches motion — confirm on task completion (`Confirm`), pickup on drag (`LongPress`),
  and subtle tick on reorder slot cross (`TextHandleMove`). Never vibrate on room switch, scroll, or tap.
- **Reduced motion (`ANIMATOR_DURATION_SCALE == 0`):** Button-triggered drawer snaps (`0 ms`; drag still tracks
  finger 1:1); header scroll collapse remains scroll-linked without settle bounce (or snaps on threshold);
  predictive back falls back to instant back on commit; shared-element bounds morphs disable in favour of
  `REDUCED_FADE` (`100 ms`) crossfades.

#### 15. Web transitions (View Transitions API)

- **Progressive enhancement:** Wrap room switches and narrow-viewport detail navigation in
  `document.startViewTransition(...)` only when `'startViewTransition' in document` and
  `!window.matchMedia('(prefers-reduced-motion: reduce)').matches`. Unsupported browsers fall back cleanly to
  standard CSS transitions with no missing state.
- **Named elements (`view-transition-name`):**
  - `room-sky` (landscape header): crossfades sky and hills with `var(--duration-room-hue)` and `var(--ease-sun)`.
  - `room-title` (room title and eyebrow): crossfade + `4 px` vertical glide with
    `var(--duration-content-swap)` and `var(--ease-standard)`.
  - `task-detail` (narrow-viewport detail sheet): enters/exits with `var(--duration-panel)` and
    `var(--ease-enter)` / `var(--ease-exit)`.
  - Assign `view-transition-name` **only for the active navigation** (set just before `startViewTransition` and
    clear in `transition.finished`), never permanently on every list row.
- **Leptos / wasm32 integration:** Commit reactive signal updates synchronously inside the `startViewTransition`
  callback so the browser snapshots the exact post-navigation DOM. Never wrap key-repeat (`J`/`K`) or incoming
  remote SSE updates in a View Transition.
- **Reduced motion:** Skip `startViewTransition` entirely (or restrict `::view-transition-old(*)` /
  `::view-transition-new(*)` to `var(--duration-reduced-fade)` opacity-only crossfade with `0 px` movement).

### Anti-patterns: what never animates

1. **Live remote and agent updates never move layout.** When SSE delivers a change from another device or an AI
   agent, never slide rows in, pop checkboxes, animate heights, or trigger a View Transition while the owner may
   be reading or about to click. Update the list cleanly (anchoring scroll if items change above the viewport)
   and draw attention with a quiet `wash → pane` background highlight (`SYNC_HIGHLIGHT` 900 ms, `EXIT`; under
   reduced motion: hold `wash` for `COMPLETE_HOLD` 450 ms, then fade out in `REDUCED_FADE` 100 ms).
2. **No text reflow or font-metric animation.** Never animate container width or padding in a way that re-wraps
   titles or Czech strings mid-flight, and never animate `font-size` or `font-weight`.
3. **No spinners, pulsing dots, or idle loops.** Pairing, syncing, the sun, and the stars never loop or idle.
   Progress is shown by the sun's position on its arc or plain text.
4. **No spring bounce outside the checkbox.** `POP` is exclusively for `CHECK_FILL`. Panels, drawers, toasts,
   menus, and the sun never overshoot.
5. **No bulk row cascades.** Switching rooms, filtering (`Ctrl+K`), or loading a list never staggers 20 rows one
   after another. `STAGGER` (`25 ms`) is capped at `STAGGER_MAX_ITEMS` (5) and reserved for small intentional
   reveals.
6. **No input gating.** Never wait for an animation to finish before accepting the next click, keypress, or undo.
7. **No backdrop blur transitions.** Elevation forbids blur everywhere; modal scrims fade opacity only.
8. **No per-frame landscape rebuilds at rest.** `todo_design::landscape` paths stay cached; only interpolate
   progress during an active `SUN_TRAVEL` or `DUSK` transition and re-cache as soon as it settles.

### Rollout priorities

Implementation order across clients follows the comprehension gaps identified in `docs/ux-review.md`:

1. **Task completion & sun arc advance** (`CHECK_FILL`, `CHECK_TICK`, `COMPLETE_HOLD`, `ROW_COLLAPSE`,
   `SUN_TRAVEL`) across Desktop, Web and Android.
2. **Room switching & OKLCH sky hue morph** (`ROOM_HUE`, `CONTENT_SWAP`).
3. **Detail pane open/close/task-switch** (`PANEL`, `CONTENT_SWAP`), including Web's medium-viewport scrim and
   Desktop's empty-detail collapse on finished My Day.
4. **Sidebar collection collapse/expand & overlay scrollbar fade** (`PANEL`, `SCROLLBAR_HOLD`, `SCROLLBAR_OUT`).
5. **Android scroll-linked header collapse, drawer & predictive back** (`PANEL`).
6. **My Day dusk transition** (`DUSK`) and **toasts / live-sync highlights** (`TOAST_IN`, `TOAST_OUT`,
   `UNDO_WINDOW`, `SYNC_HIGHLIGHT`).

## Iconography

- **Interface icons come from one set: Lucide.** They are stroke icons on a 24-unit grid with round caps, and
  they're bundled at stroke 1.6. Draw them at 18 px in toolbars and rows, and at 14 px inside chips and metadata.
  Never mix in another set, and never use emoji as icons. Windows caption buttons are OS chrome and use
  a matching set of custom minimize, maximize, restore and close marks.
- **The hand-drawn list glyphs** are the app's own: sun, briefcase, layers, house, flag, coin, paperclip,
  calendar-star, code, wrench, book, basket, heart, paper-plane and document. They are slightly irregular single
  strokes, drawn at 16 px in the list's `glyph` colour. They give the sidebar and the phone room pills character
  without colour blocks.
- **Every icon and glyph is a single-colour SVG** (`currentColor`, with no fills), so each platform tints it like
  text. GPUI can't render multicolour SVG in core UI, and the design doesn't need it.

## Agents: letter marks

Agents are shown as **round letter marks**: **C** (Claude), **X** (Codex) and **π** (Pi).

- **The mark.** It is 20 px. The letter is in Geist Mono 600 at half the mark's size, on the agent's tinted
  background, with a 1 px inset ring.
- **Where it appears.** The same mark appears everywhere: in rows (in the agent column), in the sidebar's agent rows
  under Activity, in the activity feed and in toasts.
- **The name.** It is in the hover tooltip, in the accessible name ("Agent: Codex") and in the detail activity.
- **You.** Your own entries use a neutral `Y` mark.
- **No faces, avatars or mascots anywhere.** Attribution must be precise, not cute.

## Banned looks

These are the generic "AI-default" looks, and they don't ship:

- purple-to-blue gradients, and gradients in general (the dusk sky is the single exception, and it has a meaning);
- glassmorphism, background blur and frosted panels;
- emoji used as icons;
- floating cards with heavy drop shadows;
- Inter (or any system sans) on white with no typographic identity;
- generic SaaS dashboard chrome: stat tiles, KPI rows, sparkline decoration;
- decoration without meaning, such as extra suns, mascots, confetti or illustrations that measure nothing;
- pills everywhere (see the pill budget above);
- congratulation copy such as "You crushed it!".

## The screenshot gate

The design requirement is a hard gate, not polish. A UI change is done only when it has been checked on screen.

- **A screenshot mode.** The desktop app has a deterministic `--screenshot <out.png> --fixture <name>
  [--theme light|dark]` mode. It renders fixture data in its own window and captures it, without injecting input.
- **The gallery.** A script, or CI, renders a gallery of every screen in both themes at 100% and 150% scaling.
  Reviews compare the gallery side by side with the approved mockups, which are kept outside this repository.
- **What reviewers check:**
  - alignment to the 4 px grid and the fixed columns;
  - truncation of long Czech titles;
  - contrast;
  - that the sun matches the count;
  - that the toast inset never covers a row;
  - that 1 px strokes and the landscape paths stay crisp at 100% and 125% scaling.
- **Fixtures and mockups use invented data only:** made-up tasks, lists and people, and `example.com` addresses.
  Never use real tasks.

## The app icon

**Daybreak:** the sun, which means "done" everywhere in the app, rises over the My Day hills and carries the tick.
It sits on a light sky tile, so the icon says "done" in the app's own language. Sky, hills, halo, sun and tick are
`sky`, `hill-far`, `hill-near`, `sun` and `ink` of the light My Day room. The shapes are flat and unblurred.

- **Two drawings.** `master.svg` is for 32 px and up. At 16 and 24 px the halo and the fine curves turn to mush, so
  `small-16.svg` drops the halo and uses a bigger disc, a heavier tick and one hill: disc, tick and hill is all
  that is left to read. Never scale the master down for 16 or 24 px.
- **One source.** The SVGs in `assets/brand/` are the source of every size and platform: the Windows `.ico`, the
  PWA and apple-touch PNGs, and the Android adaptive layers (key art inside the 66-unit safe circle of the 108-unit
  canvas). `assets/brand/generate.mjs` produces the rasters, the `.ico` and the monochrome layer flattened for
  Android; CI fails when they are stale. Clients embed the generated files and never redraw the mark in code.
- **The tile is part of the icon.** The tile is a rounded square on desktop and the web, a full-bleed square for
  apple-touch and maskable (the OS cuts the shape), and the adaptive background layer on Android.

See `assets/brand/README.md` for which file goes where.

## Using the tokens

| Platform | How |
|---|---|
| GPUI (desktop) | Depend on `todo-design`. `palette(Theme::Light, hues::MY_DAY)` returns a `Palette` of `Rgb`; `Rgb::to_u32()` feeds `gpui::rgb()`. Type, spacing, radii, metrics and motion are consts. `landscape::Landscape` gives the header paths. Fonts are in `fonts::FACES` (the `embed-fonts` feature embeds the bytes). Icons are in `Icon::svg()`. |
| Compose (Android) | Call the Rust crate through the shared core, or read `crates/todo-design/generated/tokens.json`, which has every precomputed colour for the named hues, plus the OKLCH specs to derive others. |
| Web | Use `crates/todo-design/generated/tokens.css`. Set `data-theme` and `data-room-hue` on the room's root element, `data-list-hue` on chips, and `data-agent` on marks. |

To change a token, edit the tables in `crates/todo-design/src/` and regenerate the exports:

```sh
TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated
```

Then run `cargo test -p todo-design`. The contrast tests will tell you if the change broke a pair.
