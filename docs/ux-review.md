# Cross-platform UX review

A review of the current Desktop (GPUI), Web (Leptos/wasm) and Android (Jetpack Compose) clients against
`docs/design-language.md`, based on the rendered screenshot galleries and Paparazzi goldens:

- **Desktop:** `apps/desktop/target/gallery/*.png` (100 % and 150 % scale, light and dark themes)
- **Web:** `apps/web/e2e/gallery/*.png` (branch `feat/web-client`: desktop, medium, phone, light and dark)
- **Android:** `apps/android/app/src/test/snapshots/images/*.png` (branch `feat/android-app-v1`: cover screen,
  large inner screen, large text, landscape/short window, light and dark)

Overall, the Daybreak visual identity lands well across all three platforms: the Fraunces / Schibsted Grotesk /
Geist Mono pairing, warm OKLCH room palettes, round checkboxes, dusk scene, and reserved bottom toast strip look
cohesive and crafted rather than generic. The findings below focus on concrete layout bugs, cross-platform drift,
affordance gaps, accessibility issues, and where motion will add the most comprehension.

## Findings ranked by impact

### P0 — Broken or clipped UI

#### 1. Long list names push Detail header actions off-screen (Desktop)

- **Screens / files:** `apps/desktop/target/gallery/my-day-light-100.png`, `my-day-dark-100.png`,
  `my-day-light-150.png`, `long-sidebar-dark-100.png`; `apps/desktop/src/ui/shell/detail.rs`.
- **Problem:** When a task belongs to a list with a long name (`Planning the quarterly roadmap and every review
  meeting`), the breadcrumb at the top of the 392 px detail pane overflows horizontally, clips mid-word
  (`...every revie`), and pushes the three top-right action buttons (My Day sun, Important star, Close `×`)
  completely off-screen. Compare `my-day-light-100.png` (all three buttons unreachable) with `work-light-100.png`
  (short list name `Planning`, all three visible).
- **Recommendation:** Give the breadcrumb container `min-w-0 flex-1 overflow-hidden` with ellipsis on the list
  label, and mark the right-hand action group `flex-none` so My Day, Important and Close can never be displaced.

#### 2. Task metadata lines hard-clip tags mid-word with no ellipsis (Desktop & Web phone)

- **Screens / files:** `apps/desktop/target/gallery/my-day-light-100.png` (row 1: `#planni`);
  `apps/web/e2e/gallery/phone-light.png` (row 2: `#financ`, row 6: `#au`, row 8: `#rodir`);
  `apps/desktop/src/ui/shell.rs`, `apps/web/static/app.css`.
- **Problem:** Uncapped list chips and inline tags sit inside `overflow: hidden` rows without truncation rules.
  On Desktop, a long list chip pushes `#planning` into a hard mid-word clip (`#planni`) and hides `#q4` and the
  source-mail icon completely. On Web phone, tags clip mid-character on nearly every row that has both a due date
  and a list chip. (Android handles this better with ellipsis, though under large text in
  `cover_large_text_light.png` the tag collapses to `1/4 ...`.)
- **Recommendation:** Cap list-chip label width (`max-width: 180 px` desktop, `120 dp` phone) with ellipsis, place
  the compact mail/step indicators before trailing tags so source metadata isn't silently lost, and either
  ellipsis-truncate or collapse overflowing tags into a clean `+N` count instead of mid-character clipping.

#### 3. Medium-viewport Web detail pane overlaps the list without a scrim (Web)

- **Screens / files:** `apps/web/e2e/gallery/medium-light.png`, `medium-dark.png`; `apps/web/static/app.css`.
- **Problem:** At medium desktop/tablet widths, the detail pane overlays the right side of the task list without a
  scrim, slicing across the `+ Add a task...` input, the header progress count (`3 of 12 done`), and every row's
  agent mark, priority and due date. With only a faint border/shadow, the list looks accidentally cropped rather
  than intentionally behind a sheet.
- **Recommendation:** Either dim the list behind a translucent neutral scrim when the detail pane overlaps (with
  click-outside and `Esc` dismissal, animated over `PANEL` 200 ms), or collapse the sidebar into a drawer first
  at medium breakpoints so the list and detail columns sit side by side without overlapping.

### P1 — Missing core actions, consistency & accessibility

#### 4. Android detail screen duplicates "My Day" while omitting "Important" and "+ Add step" (Android)

- **Screens / files:** `apps/android/.../cover_detail_light.png`, `inner_personal_light.png`.
- **Problem:** The detail screen places a sun toggle in the top-right bar *and* repeats a tinted `In My Day` pill
  directly under the title. Meanwhile, the Important (`☆`) star toggle and the `+ Add step` input present on
  Desktop and Web are missing altogether.
- **Recommendation:** Align the detail top bar with Desktop (`Sun` My Day, `Star` Important, Close/Back), remove
  the redundant `In My Day` pill from the header body, and add an inline `+ Add step` control at the end of Steps.

#### 5. Cross-platform control-order and feature drift breaks muscle memory (Desktop, Web, Android)

- **Screens / files:** Desktop `work-light-100.png`, Web `desktop-light.png` & `phone-detail-light.png`, Android
  `cover_detail_light.png` & `cover_drawer_light.png`.
- **Problem:**
  - **Reversed detail header actions:** Desktop orders `Sun → Star → ×`, Web reverses them to `Star → Sun → ×`,
    and Android shows only `Sun`.
  - **Search / filter missing on Web and Android:** Desktop provides a prominent title-bar filter (`Ctrl+K`) with
    query syntax (`tag:docs`), while Web and Android expose no search/filter affordance at all.
  - **Sidebar parity:** Collapsible collections, `+ New list`, and the `Agents` activity summary exist on Desktop
    only; Web and the Android drawer omit all three.
  - **Sync status inconsistency:** Web desktop prints `Synced 10:35` twice on the same screen (sidebar footer and
    bottom-right status bar), whereas Android's drawer footer shows only a grey dot and hostname
    (`todo.example.com`) with no sync state, timestamp, or settings/sign-out entry.
- **Recommendation:** Standardise detail header actions to `Sun → Star → ×` on every client; add a search/filter
  entry on Web (`/` or `Ctrl+K`) and Android's top bar; add collection collapse chevrons and `+ New list` to Web
  and Android; remove Web's duplicate right-hand sync timestamp and surface sync status + settings in Android's
  drawer footer.

#### 6. Finished My Day omits quick-add on Desktop/Web, omits "Plan tomorrow" & "Completed" on Android/Web, and leaves a blank detail void on Desktop

- **Screens / files:** Desktop `finished-light-100.png`, Web `finished-light.png`, Android
  `cover_finished_light.png`.
- **Problem:**
  - **No quick-add when finished (Desktop & Web):** Both hide the quick-add input once My Day reaches 0 open
    tasks—even while Desktop's status bar still displays `Ctrl N adds a task`. Android rightly keeps the composer
    reachable.
  - **Missing handoff & Completed controls:** Android omits the `Plan Sunday` primary button mandated by
    `docs/design-language.md` and omits `> COMPLETED 12`. Web also omits `> COMPLETED 12`, leaving no way to view
    today's completed tasks in place.
  - **Empty detail column (Desktop):** Desktop keeps the 392 px `Select a task to see its details` placeholder
    open next to the dusk scene (~27 % empty space), whereas Web lets the dusk scene span full width until a task
    is selected.
- **Recommendation:** Keep a quiet quick-add available on finished My Day on all platforms; show both
  `Plan <weekday>` and `> COMPLETED n` across Desktop, Web and Android; collapse Desktop's empty detail pane when
  nothing is selected on finished My Day (opening it smoothly via `PANEL` when a tomorrow/week task is picked).

#### 7. Android pairing screen has a low-contrast disabled button and draws a sun with zero tasks (Android)

- **Screens / files:** `apps/android/.../pairing_address_light.png`, `pairing_error_dark.png`.
- **Problem:**
  - **Contrast failure:** When the server URL field is empty, `Get a pairing code` renders pale off-white text on
    muted tan (~2.2:1 contrast, well below WCAG 4.5:1).
  - **Design-language conflict:** Both pairing screens render a partial sun and arc peeking over the far hill,
    contradicting `docs/design-language.md` ("The sun has exactly one meaning: completed tasks... An empty list
    shows the hills only"). Desktop's `pairing-light-100.png` correctly shows hills only.
- **Recommendation:** Use a contrast-compliant disabled button style (`wash` background, `ink-3` text, `1 px`
  `line-strong` border, ≥ 4.5:1) and suppress the sun and arc on pairing and error screens.

#### 8. Long sidebar clips the bottom list mid-row with no scroll affordance (Desktop)

- **Screens / files:** `apps/desktop/target/gallery/long-sidebar-dark-100.png`;
  `apps/desktop/src/ui/shell/sidebar.rs`.
- **Problem:** When collections and lists overflow the sidebar height, the last visible item (`Language course 0`)
  is sliced vertically right at the footer hairline with no visible scrollbar or overflow cue, and both `+ New
  list` and the `Agents` panel scroll completely out of sight.
- **Recommendation:** Paint the overlay scrollbar on scroll/hover (using `HOVER` / `SCROLLBAR_HOLD` /
  `SCROLLBAR_OUT`), add a subtle bottom hairline cue when scrolled content continues below, and keep `+ New list`
  pinned above the footer so creating a list never requires scrolling to the bottom of every collection.

### P2 — Alignment, affordances, empty/error states & keyboard clarity

#### 9. Undated phone rows collapse the reserved 100 dp due-date slot (Android & Web phone)

- **Screens / files:** Android `cover_my_day_light.png`, `inner_personal_light.png` (`Call the dentist to
  reschedule`); Web `phone-light.png` (`Review the API latency report`, `Draft the agenda`, `Rotate the backup`,
  `Uklidit sklep`).
- **Problem:** `docs/design-language.md` specifies a fixed `100 dp` due slot on phone rows so list chips always
  start at the same x-coordinate. Both Android and Web phone collapse the slot when a task has no due date,
  causing list chips to zig-zag horizontally and slowing visual scanning.
- **Recommendation:** Always reserve the `100 dp` due slot on phone rows (empty or subtle `–`), restoring vertical
  column alignment across mixed dated and undated lists.

#### 10. Empty-state copy, read-only detail fields, recovery hierarchy, and keyboard hints need tightening (Desktop, Web, Android)

- **Screens / files:** Desktop `empty-light-100.png`, `my-day-light-100.png`, `collapsed-collection-light-100.png`,
  `open-failed-confirm-light-100.png`, `pairing-code-dark-100.png`; Web `empty-list-light.png`, `phone-light.png`;
  Android `cover_empty_light.png`, `cover_toast_light.png`, `cant_open_light.png`.
- **Problem:**
  - **Empty-state copy & wrong shortcut hint:** Three clients use three different phrases (`"Nothing here yet."`
    Desktop, `"Nothing on the list yet."` Web, `"Nothing on the shelf yet."` Android & design doc). Desktop's
    empty Reading List subtext tells the user to *"press Ctrl+Alt+Space from anywhere"*, which actually adds to
    **My Day**, not Reading List. Desktop's bottom-left status bar also reads `00 done` while `COMPLETED 2` sits
    directly above it (and uses leading zeros `03 done` that clash with `3 of 12 done`).
  - **Unmarked read-only fields & ambiguous icons:** On Desktop, `Reminder`, `Repeat` and `Tags` in the detail
    pane are non-editable (`docs/desktop.md`) yet styled in the same bold ink as editable `Due` (differing only by
    a small `D` keycap). Similarly, the calendar and flag icons inside empty quick-add and the `sliders` / `...`
    icons in the room header have no visible hover/focus label affordance.
  - **Inconsistent keyboard bar:** Desktop's status bar drops `P priority` in `collapsed-collection-light-100.png`
    even with a task selected, shows task-editing shortcuts (`J K`, `Space`, `E`, `D`) when a list is empty, and
    never surfaces `Alt+Up/Down` (reorder) or `Esc` (close detail).
  - **Error & pairing polish:** Desktop's `open-failed-confirm-light-100.png` leads with raw SQLite filename/error
    details above the plain-language explanation and keeps `Try again` styled as a primary button directly above
    the destructive `Start fresh` confirmation card. Android's `cant_open_light.png` has better copy hierarchy,
    but styles destructive `Unpair this phone` as a neutral outlined button below red warning text. Desktop's
    `pairing-code-dark-100.png` gives equal visual weight to `Open the page again` and `Use another address` and
    lacks a quick copy-code button.
  - **Minor phone spacing polish:** Web `phone-light.png` clips the top sliver of row 9 above `COMPLETED 3` and
    leaves a tall empty band between `COMPLETED 3` and `+ Add a task...`; Android `cover_toast_light.png` puts
    `now` next to a truncated title (`"Koupit rohlíky a ml...`), squeezing the task name unnecessarily.
- **Recommendation:** Standardise empty-state copy across all clients and point Desktop's list-empty hint to
  `Ctrl+N`; drop leading zeros (`3 done`) and reconcile `done` counts with `COMPLETED n`; mute read-only detail
  values (`ink-2`) until editable; make Desktop's shortcut bar context-aware; lead Desktop recovery errors with
  plain-language text (demoting raw SQLite strings to a quiet mono detail line) and demote `Try again` while the
  destructive confirmation card is open; drop `now` from immediate Android completion toasts to give the title
  more room.

## Where motion will help comprehension most

Prioritising motion work by the gaps surfaced in this review gives the highest user-facing payoff first:

1. **Task completion choreography & sun arc advance (`CHECK_FILL`, `CHECK_TICK`, `COMPLETE_HOLD`, `ROW_COLLAPSE`,
   `SUN_TRAVEL`):** Desktop has the checkbox and row fade, but the sun jumps and Web/Android lack the full
   sequence. Connecting the checkbox confirmation directly to the sun's travel along its arc makes progress
   unmistakable without a single line of congratulation copy.
2. **Room switching & shortest-arc OKLCH sky hue morph (`ROOM_HUE`, `CONTENT_SWAP`):** With 12+ swatches and
   nested collections across Desktop, Web and Android room pills, morphing the sky hue and gently swapping the
   title anchors spatial orientation so room changes feel calm instead of jarring flashes.
3. **Detail pane open/close/task-switch & medium-viewport scrim (`PANEL`, `CONTENT_SWAP`):** Directly fixes the
   abrupt overlap seen in Web `medium-light.png`, makes closing/opening Desktop's empty detail pane on finished My
   Day seamless, and ensures rapid `J`/`K` selection never flickers.
4. **Sidebar collection collapse/expand & overlay scrollbar fade (`PANEL`, `SCROLLBAR_HOLD`, `SCROLLBAR_OUT`):**
   Makes collection folding feel spatial and resolves the clipped-list ambiguity in Desktop's
   `long-sidebar-dark-100.png`.
5. **Android collapsing header, drawer & predictive back (`PANEL`):** Smoothly bridges the expanded header
   (`cover_my_day_light.png`) and scrolled compact bar (`cover_collapsed_light.png`), while keeping drawer swipes
   and Android 14+ predictive back 1:1 with the finger.
6. **My Day dusk transition (`DUSK`):** Turns the stark jump between the light task list and dark dusk scene into
   Daybreak's signature reward as the last task leaves.
7. **Toasts, undo & quiet live-sync highlights (`TOAST_IN`, `TOAST_OUT`, `UNDO_WINDOW`, `SYNC_HIGHLIGHT`):**
   Keeps undo firmly inside the reserved bottom strip and ensures remote/agent edits (Claude, Codex, Pi) never
   cause layout jumps while the owner is reading or clicking.
