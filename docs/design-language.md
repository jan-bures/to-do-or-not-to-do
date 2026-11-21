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

- **The sun has exactly one meaning: completed tasks.** It climbs a faint arc as tasks are done. The part of the
  arc it has travelled is drawn in `sun-ink`, and the part still to go in `arc`. The arc is measured to 3:1 against
  the sky, so it reads as information, not decoration.
- **The count sits beside the sun.** The header summary reads `3 of 12 done · 9 open · est. 5h 40m`. The sun and
  the numbers say the same thing in the same place. Never put a second, floating label on the sun.
- **There is only one sun.** No sun icon in the title, no clock ticks, no crescent endpoints. The sun's amber is the
  only amber on screen, at a fixed hue whatever the room's hue.
- **The finished day.** When My Day's last task is done, the scene grows to 164 px.
  - The sky becomes a two-stop dusk gradient.
  - The sun sets behind the hills with three thin rings.
  - A moon and stars come out.
  - The page hands over to tomorrow: "When the sun comes up" shows tomorrow's first task with a sunrise mark and a
    *Plan tomorrow* button, followed by the rest of tomorrow and the rest of the week.

  There is no congratulation copy and no stats recap. The scene is the reward. The dusk scene is a night scene in
  both themes, so the header block uses dark-theme text over it.
- **An empty list shows the hills only.** With no tasks there is nothing to measure, so there is no sun and no arc.
  Below the quick-add sits one italic Fraunces line ("Nothing on the shelf yet.") and, if an agent has any,
  suggestions.
- **Restraint keeps it from being clip art.** There are no faces, no characters and no idle animation. Whimsy rests
  on three things only: the sun, the hand-drawn list glyphs and the round checkboxes.

`todo_design::landscape` computes the geometry: hills, arc, sun, rays, rings, stars and moon. GPUI and Compose
draw the same shapes from it. Everything is a single-colour filled path, a stroked polyline or a circle, plus one
linear gradient for the dusk sky. Build the paths when the size or the count changes, never per frame.

## Colour

### How colours are made

Every colour is **authored in OKLCH** and converted to sRGB. A colour that falls outside the sRGB gamut keeps its
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
keys, used in settings, CSS and JSON.

### Palette rules

- **The pane is near-white or near-black, never pure.** The canvas, which holds the sidebar, title bar and status
  bar, sits one step darker in light mode and one step lighter in dark mode.
- **Colour means something, or it isn't there.**
  - Accent is for the checked box, the selection bar, "due soon" and the primary button.
  - Danger is for overdue items.
  - The list hue is for list identity.
  - The sun's amber is for progress.
- **An overdue row has at most two reds:** the checkbox ring and the overdue badge. Priority stays neutral.
- **Priority is never colour.** It is `!` / `!!` / `!!!` in Geist Mono. Count, weight and ink step together from
  `ink-3` up to `ink`.
- **Selection** is a `wash` background plus a 3 px `accent` bar on the left edge.
- **Translucency** is only used in the landscape: the sun's halo, stars, the moon and the dusk rings. UI surfaces
  are always opaque.

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

## Type

There are three families, and each has a single job. All three are OFL licensed and bundled as static instances
with full Czech coverage. The files and their sources are in `assets/fonts/`.

| Family | Role | Examples |
|---|---|---|
| **Fraunces** (soft display serif, `opsz` 144 / 36, SOFT 100) | the room's voice | the room title at 360, 56/60, −0.028em; the italic day name "Saturday"; the empty-state line |
| **Schibsted Grotesk** | everything you read and operate | task titles at 500, 15/20; body at 14/20; detail title at 600, 21/28; chips at 550, 12/20 |
| **Geist Mono** | anything you compare or scan in a column | due times, counts, tags (`#tax`), keycaps, priority, the `CAPS LABELS` at +0.12em |

The pairing works by contrast. The serif appears only at display sizes and in one italic accent. The sans does all
the work. The mono gives numbers tabular figures and makes metadata read as data. The detail title is in the sans,
not the serif, because it sits next to dense steps.

`todo_design::typography` holds the full scale, with desktop and phone variants. Don't add sizes ad hoc: add a
named style.

## Grid, columns and density

Everything sits on a **4 px grid**. Text and strokes may use half steps, but layout never does. The spacing steps
are 4, 8, 12, 16, 20, 24, 32 and 40.

**The desktop window** is laid out at the reference size of 1440 × 900:

- three panes: sidebar **248**, list **800** (flexible) and detail **392**;
- a 36 px title bar and a 28 px status bar;
- 32 px horizontal padding in the list column.

**Desktop rows** are 56 px tall, on a fixed grid: `20 | 1fr | 20 | 24 | 104` with 12 px gaps. The columns are
checkbox, title (with the metadata line under it), agent mark, priority and due date.

- The agent, priority and due columns are always there, even when they're empty, so every row lines up.
- A long title truncates with an ellipsis before the agent column. Test this with a long Czech title, for example
  *Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení*.

**Phone rows** are 64 dp tall: `22 | 1fr | 20 | 24`. The metadata line reserves a 100 dp due slot, so list chips
always start at the same x. A cover screen fits nine full rows above the composer.

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

- The selected sidebar item gets a 1 px ring.
- A focused input gets an `accent` border plus a 3 px ring in `tint`.
- There is exactly one shadow, a soft two-layer one for transient overlays: menus, popovers and the date picker.
  In dark mode it comes with a 1 px `line-strong` border.
- Nothing is blurred.

## Motion

Motion confirms what happened. It never decorates, and nothing loops or idles. Durations and easings are in
`todo_design::motion`.

**Completing a task** is the one choreographed moment:

1. **The fill (140 ms).** The ring fills with `accent` and pops slightly, using the only overshoot curve in the app.
2. **The tick (180 ms).** After a 60 ms delay, the tick draws itself from left to right.
3. **The hold (450 ms).** The title dims to `ink-3` with a strike-through. The row holds still so the eye sees it
   land.
4. **The exit (220 ms).** The row collapses and fades, and the rows below close the gap.
5. **The sun (700 ms).** The sun eases along its arc to the new count, and the travelled path grows with it. The
   header count changes at the same moment.

**Finishing My Day** turns the sky to dusk and sets the sun behind the hills over 1.2 s.

**Other motion:**

| Event | Duration |
|---|---|
| Toast enters | 180 ms |
| Toast leaves | 140 ms |
| Undo window (shown by a thin countdown line in the toast) | 6 s; Ctrl+Z still works afterwards |
| Panels open or close | 200 ms |
| Hover | 90 ms |

**Reduced motion** replaces every movement with an instant change and caps fades at 100 ms. The completion hold
and the undo window are timers, not animations, so they stay the same.

## Iconography

- **Interface icons come from one set: Lucide.** They are stroke icons on a 24-unit grid with round caps, and
  they're bundled at stroke 1.6. Draw them at 18 px in toolbars and rows, and at 14 px inside chips and metadata.
  Never mix in another set, and never use emoji as icons.
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
- **Where it appears.** The same mark appears everywhere: in rows (in the agent column), in the sidebar's Agents
  panel, in the activity feed and in toasts.
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
