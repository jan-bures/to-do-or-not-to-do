# AI Assistant UX Specification

This specification designs the built-in **AI Assistant** ("Ask AI") across all three clients — **GPUI desktop** (`apps/desktop`, visual reference), **Leptos web** (`apps/web`), and **Android Compose** (`apps/android`, following the thumb-zone rules in [`docs/android-thumb-zone.md`](android-thumb-zone.md) and [`docs/android-header.md`](android-header.md)) — strictly within the **Playful / Daybreak** design language ([`docs/design-language.md`](design-language.md)).

Every layout number, colour, radius, type style, icon, and motion duration comes from `todo-design` (`crates/todo-design`). Auth plumbing (ChatGPT sign-in flow, token storage mechanics, server proxy details) is researched separately; this document specifies the **UI, interaction model, states, privacy cues, provider settings, tokens, motion, accessibility, and phased implementation checklists**.

> The first Windows release implements **Your own server** only. It has no cloud fallback,
> destructive tools or persistent history. Device-local list access defaults to **Hidden**.
> Suggestion chips prepare a draft, and the owner sends it. For supported URLs and transport
> restrictions, see the [provider architecture](ai-assistant-architecture.md#3-provider-choices).

---

## 1. Concept & identity: a calm agent that asks first

The app already treats AI agents (`Claude`, `Codex`, `Pi`) as first-class collaborators with letter marks, per-list permissions, live `SYNC_HIGHLIGHT` updates, and reversible entries in **Activity**. The built-in assistant extends that exact model directly into the app:

- **One native identity (`Assistant`).** Approved changes are attributed to `Actor::Agent { agent_id: "assistant" }`, represented everywhere by the round **`A`** letter mark (`20 px`, Geist Mono 600, `1 px` inset ring) in hue `192°` (`data-agent="assistant"` — calm lagoon-teal, distinct from Claude `38°`, Pi `140°`, and Codex `245°`, and intentionally avoiding generic purple-to-blue "AI" tropes).
- **Nothing changes behind your back.** Reading lists and tasks happens automatically with a compact step log; every write (`CreateTask`, `UpdateTask`, `MoveTask`, `SetMyDay`, `CompleteTask`, `MergeTasks`, `Delete` / archive, steps, reminders, tags) is surfaced as an explicit **Proposal Card** listing the exact fields or before → after diff, waiting for **Approve** or **Decline**.
- **Live feedback & full reversibility.** Approving a proposal executes the same `todo_core` command(s) through `SyncClient`: affected task rows appear or update immediately in the Task List with the standard `SYNC_HIGHLIGHT` (`900 ms`, `wash → pane`) and the **`A`** mark, an Undo toast lands in the reserved bottom inset, the proposal collapses into a compact receipt with an inline **Undo** link, and **Activity** + **Task Detail Activity** record the change under `Assistant` (and the sidebar shows `A Assistant · N added today` under Activity).
- **Zero generic AI chrome.** Flat opaque surfaces separated by colour steps and `1 px` hairlines. No purple-blue gradients, no backdrop blur, no robot mascots, no spinning loaders or pulsing dots, no hype copy, and strict adherence to the pill budget (only list chips, overdue badges, and phone room pills are pill-shaped; suggestion chips use `radius::MD` `6 px`).

---

## 2. Desktop (visual reference, `1440 × 900`)

### 2.1 Shell integration, pane layout & breakpoints

| Window width | Docked columns | Behaviour when opening Ask AI | Behaviour when clicking a task link in chat |
|---|---|---|---|
| **`≥ 1600 px`** (`desktop::FOUR_PANE_MIN`) | Sidebar (`248`) \| List (`≥ 400`) \| Detail (`392`) \| Assistant (`400`) | All four panes dock side by side, each separated by a `1 px` `line` hairline. When shrinking the window, Assistant yields width to `AI_PANEL_MIN` (`340`) first, then Detail yields to `DETAIL_MIN` (`320`); List never drops below `LIST_MIN_BESIDE_DETAIL` (`400`). | Selects the task in the List and updates the docked Detail pane in place (`CONTENT_SWAP`, `120 ms`). |
| **`1200–1599 px`** (`THREE_PANE_MIN` .. `FOUR_PANE_MIN − 1`, incl. `1440 × 900` reference) | Sidebar (`248`) \| List (`792`) \| Assistant (`400`) | Assistant takes the docked right rail (`400 px` default, resizable `340–520 px`). If Task Detail was open, the selected task stays selected in the List while Assistant occupies the right rail. | Selects & scrolls to the task in the List, and opens Task Detail as a **`392 px` anchored overlay** over the right edge of the List (`shadow-overlay` + `elevation::scrim` over the List only), keeping the Assistant docked beside it. `Esc` closes the Detail overlay first; a second `Esc` closes Assistant. |
| **`< 1200 px`** (narrow desktop) | Sidebar \| List | Assistant opens as a right-edge overlay drawer (`min(400 px, 100vw − 48 px)`) with `shadow-overlay` and `elevation::scrim` over the List (`OVERLAY_IN` `150 ms`). Clicking the scrim or pressing `Esc` closes it. | Closes the Assistant overlay (preserving the conversation) and opens the Task Detail overlay for that task. |

### 2.2 Open / close affordance & keyboard shortcut

- **Titlebar trigger (`36 px` titlebar):** Placed outside the window drag region, directly to the left of the Windows caption buttons (`12 px` gap).
  - Geometry: `26 px` tall (fits `36 px` titlebar with `5 px` top/bottom breathing room), `radius::MD` (`6 px`), `1 px` `line-strong` border on `pane` at rest.
  - Content: `sparkles` (`14 px` `ICON_SMALL` in `accent-strong`) + `"Ask AI"` (`BUTTON_SMALL`) + keycap `Ctrl J` (hidden below `1200 px` to keep the titlebar uncluttered).
  - Persistent open state (following *Persistent states: fill, not an outline shift*): `wash` fill, `accent` border, `accent-strong` label, `aria-pressed="true"`.
  - **Pending proposal badge when closed:** If the active conversation has unreviewed proposals while the panel is closed, a `6 px` `accent` dot (`component::PRIVACY_DOT`) appears on the trigger so pending reviews are never forgotten.
- **Keyboard shortcut:** **`Ctrl+J`** (macOS/web: **`Cmd+J`**, with `Ctrl+J` also accepted on non-macOS).
  - Toggles the Assistant panel open/closed from anywhere in the shell.
  - Opening focuses the AI composer textarea immediately.
  - Pressing `Esc` while focus is inside the Assistant panel: (1) closes any open popover (history, model picker), (2) clears a non-empty composer draft or cancels an active reply stream, (3) closes the Task Detail overlay if open over the list, or (4) closes the Assistant panel and returns focus to the Task List (or previous focus target).
- **Status bar (`28 px`):** Adds `Ctrl J Ask AI` to the quiet keyboard hint cluster when space allows, and shows the active AI destination at the trailing edge while the panel is open (e.g. `Private network · mac.tailnet.example` or `Cloud · ChatGPT (OpenAI)`).

### 2.3 Resizing

The docked Assistant panel's left edge uses the exact **Desktop and web pane divider** specification from [`docs/design-language.md`](design-language.md):

- `12 px` hit strip (`desktop::PANE_HANDLE_HIT`), `1 px` `line` hairline at rest, centred `3 px` `accent` highlight (`desktop::PANE_HANDLE_HIGHLIGHT`) fading in over `HOVER` (`90 ms`, `STANDARD`).
- Width bounds: default **`400 px`** (`desktop::AI_PANEL`), minimum **`340 px`** (`desktop::AI_PANEL_MIN`), maximum **`520 px`** (`desktop::AI_PANEL_MAX`), further clamped so the Task List keeps at least `400 px` (`LIST_MIN_BESIDE_DETAIL`).
- Keyboard resizing: Tab reaches the splitter (`role="separator"`, `aria-orientation="vertical"`, `aria-label="Resize AI assistant"`). `Left` / `Right` steps by `8 px` (`32 px` with `Shift`); `Home` / `Enter` resets to `400 px` (`OVERLAY_IN` `150 ms` on desktop, snap on web / reduced motion).
- Persistence: Remembered locally in desktop `ui.json` and web `localStorage` (passive window resizing never overwrites the preference).

### 2.4 Panel header & simple conversation history

Keep conversation management lightweight — no nested sidebar inside the Assistant panel:

- **Top bar (`48 px` `desktop::DETAIL_TOP`, `pane` bg, `1 px` bottom `line` hairline):** Aligns horizontally with the Task Detail top bar.
  - **Leading — Conversation selector button:** `20 px` `A` Assistant mark + current title (`"New chat"`, or auto-titled from the first prompt, e.g. `"Weekend triage"`, truncated with ellipsis) + `14 px` `chevron-down`. Clicking opens the anchored **Conversation History popover** (`280 px` wide `desktop::AI_HISTORY_MENU`, `raised`, `radius::LG`, `shadow-overlay`).
  - **Trailing actions (`32 px` `ICON_BUTTON`s):**
    1. **`+` New chat** (`plus`, disabled when already on an empty conversation).
    2. **`🕒` History** (`clock` — shown on empty state for instant discovery; opens the same history popover).
    3. **`⚙` AI settings** (`settings` — opens Provider & Privacy Settings dialog).
    4. **`×` Close** (`x` — closes panel).
- **Conversation History popover (`280 px`):**
  - Top row: `+ New conversation` (`accent-strong`).
  - Grouped simply by **`TODAY`** and **`EARLIER`** (`CAPS_LABEL`), capped at the most recent 20 local threads (older threads prune automatically).
  - Each row (`40 px`, `radius::MD`): thread title (1-line ellipsis), relative time in `COUNT` mono (`10 min ago`, `Yesterday`), an optional `6 px` `accent` dot if the thread has an unreviewed proposal, and a hover/focus-visible `trash-2` delete button.
  - Footer hairline: quiet `Clear history` action (confirms in place if ≥ 1 thread exists).
  - Conversations are stored locally per device (`pairings/<id>/ai-history.sqlite` or `IndexedDB` — never synced into the task event log).

### 2.5 Empty state & suggestion chips

- **Zero-provider onboarding state:** When no model provider is configured yet, the empty state displays a crisp flat **Connect a model** card listing the three options (*Your own server (private network)*, *ChatGPT account*, *API key*) and disables the composer with placeholder `"Choose a model to start"`.
- **Ready empty state:** Lower-weighted editorial layout so content sits close to the composer:
  - `36 × 36 px` `wash` tile (`radius::LG`, `1 px` `line` border) with `18 px` `sparkles` icon in `accent-strong`.
  - Heading (`DETAIL_TITLE`, `21/28`): `"What can I help plan?"`
  - Subtitle (`BODY`, `ink-2`): `"Ask about your tasks, or describe changes to make. Every edit or new task waits for your approval first."`
  - **4 compact context-aware suggestion chips** (`min-height: 36 px`, `radius::MD` `6 px`, `raised` fill, `1 px` `line` border, `14 px` Lucide icon in `accent-strong`, trailing `12 px` `arrow-up-right` in `ink-4`):
    - *Read-only chips auto-send immediately:* `"What’s due this week?"` (`calendar`), `"Summarize open <Current Room> tasks"` (`list`).
    - *Action/open-ended chips pre-fill and focus the composer so the owner can steer:* `"Move overdue tasks to My Day"` (`sun` — auto-sends to produce a proposal card), `"Turn a rough note into tasks…"` (`plus` — pre-fills `"Turn this note into tasks: "` and focuses composer).

### 2.6 Compact step log & prose replies with task links

- **Live step log (while running):** Never displays spinners, bouncing dots, or raw JSON. Renders 1–3 tight `20 px` lines in `META` (`12/16`) directly above the streaming reply:
  - Active step: `14 px` icon (`search` / `list` / `text`, `accent-strong`) + calm plain wording in `ink-2`, e.g. `"Looking at your lists…"`, `"Reading tasks in Work…"`.
  - Completed steps: `12 px` `check` (`success`) + `ink-3` text + right-aligned duration in `COUNT` mono (`18 ms`).
  - While generating, the composer's `arrow-up` Send button swaps in place to a crisp **`square` Stop** button (`12 px` glyph inside the same button frame, `ink` on `raised` with `line-strong` border) so the owner can cancel at any moment.
- **Settled step log (collapsed by default):** Collapses cleanly (`PANEL` `200 ms`) into one `24 px` disclosure line above the prose:
  - `▸ Checked 4 lists · Read 2 tasks · 1 private skipped`
  - Clicking expands inline to reveal every tool call, scope, duration, and any excluded lists skipped for privacy.
- **Prose replies:** Rendered directly on the `pane` background in `NOTES` (`14/24`, Geist 400, `ink`) — no heavy speech bubble around assistant text. User prompts sit right-aligned (max `88%` width) in a subtle `wash` inset (`radius::LG` `10 px`, `1 px` `line` border, `9 px 12 px` padding) so turns scan effortlessly.
- **Clickable task links:** Any task referenced in prose renders as an inline or stacked interactive reference card (`raised`, `radius::MD` `6 px`, `1 px` `line` border): `6 px` list-colour dot (`--list-dot`) + task title (`weight 500`, 1-line ellipsis) + optional mono due/overdue label + trailing `12 px` `arrow-up-right`. Clicking opens the task per §2.1.

### 2.7 Proposal cards (single & batched)

Every write the assistant wants to perform is emitted as a **Proposal Card** (`raised` fill, `radius::LG` `10 px`, `1 px` `line-strong` border, inset `3 px` leading semantic bar `component::PROPOSAL_BAR`). Mutations are visually distinguished by **icon, mono caps label, leading bar colour, and structural layout** (never colour alone):

| Proposal kind | Leading bar (`3 px`) | Header treatment | Body layout | Action buttons (left → right) |
|---|---|---|---|---|
| **Create** (`NEW TASK`, `NEW STEP`, `ADD NOTE`) | `accent` | `plus` (`accent-strong`) + `NEW TASK` (`CAPS_LABEL`) + target `ListChip` | Title (`TASK_TITLE`) + 2-column field grid (`68 px` `ink-3` key \| value): `List` (`ListChip`), `Due` (`DUE` mono), `Priority` (`PRIORITY` mono, neutral), `My Day` (`sun-filled` if yes), `Reminder`, `Repeat`, `Tags` (`#tag`), `Notes` (capped 3 lines + expand). | **`Approve`** (`primary` `accent-fill`) · **`Decline`** (`secondary`) |
| **Edit / Move / Complete** (`EDIT TASK`, `MOVE TASK`, `MY DAY`, `COMPLETE TASK`) | `sun-ink` (warm amber, guarded ≥ 3:1 across all hues) | `pencil` / `arrow-up-right` / `circle-check` (`sun-ink`) + caps kind + `ListChip` | Clickable target task link (`[• Task title ↗]`) + **explicit `old → new` diff rows** (strikethrough `ink-3` old value → bold `ink` new value; unchanged key fields noted quietly only when relevant). | **`Approve edit`** (`primary` `accent-fill`) · **`Decline`** (`secondary`) |
| **Destructive** (`DELETE TASK`, `MERGE TASKS`, `CLEAR REPEAT`) | `danger` | **`danger-bg` header band** + `trash-2` / `circle-alert` (`danger-ink`) + caps kind + `ListChip`; outer border turns `danger`. | Clickable target task link(s) + explicit plain-language consequence line (`"Moves to Trash. Reversible from Activity or Trash at any time."` or `"Merges steps and notes into target, then moves duplicate to Trash."`). | **Safety-first order:** **`Decline`** (`secondary`, default focus) · **`Delete`** / **`Merge`** (`danger-bg` fill, `danger-ink` text, `1 px` `danger` border). |

#### Batched proposals (e.g. 5 tasks at once)

When an instruction produces 2 or more proposed changes in one turn, they group into a single **Batched Proposal Card**:

- **Leading bar:** Reflects the highest risk present in the batch (`danger` if any destructive item, else `sun-ink` if any edit/move, else `accent` for pure creations).
- **Header:** Kind summary (`+ 5 NEW TASKS` or `5 CHANGES · 3 NEW, 2 EDITS`) + a compact 2-segment toggle **`Summary | Review each`** (`radius::SM` `4 px`).
- **Summary mode (default):**
  - Renders each proposed item as a selectable row with a `16 px` round checkbox (all checked by default, except destructive items inside a mixed batch which may optionally require explicit check — or remain clearly badged with a red `trash-2` icon).
  - Unchecking any row dims it to `0.58` opacity and live-updates the primary button from **`Approve all (5)`** to **`Approve 4`** (disabled at `0`).
  - Clicking any row (or its trailing `▸` chevron) expands that item's complete field grid / `old → new` diff inline inside a quiet `canvas` sub-box (`radius::MD`).
  - Footer: **`Approve 4`** (`primary`) · **`Review each`** (`secondary`) · **`Decline all`** (`quiet`, right-aligned).
- **Review-each mode:**
  - Steps cleanly through one full proposal card at a time with a mono counter (`2 of 5`) and `Prev` / `Next` chevrons, offering **`Approve`** / **`Skip`** / **`Decline`** per item (`CONTENT_SWAP` `120 ms` between items). Decisions commit immediately or at completion and leave a single consolidated batch summary receipt.
- **Scale cap:** Batches larger than 10 items show the first 5 rows with a `Show all N changes` disclosure inside the card so the conversation never becomes a wall of cards.

#### Settled & conflict proposal states

| State | Visual presentation | Interaction |
|---|---|---|
| **Approved** | Collapses (`PANEL` `200 ms`) into a calm 1-line receipt (`canvas` bg, `1 px` `line` border, `radius::LG`): `A` mark + `"Added 'Pick up oat milk and lemons' to Groceries · Just now"` + trailing **`Undo`** (`accent-strong`). | Clicking the title opens the task; clicking **`Undo`** runs tracked server compensation (`POST /events/{seq}/undo`) and changes the receipt to `"Undone"`. |
| **Declined** | Collapses (`PANEL` `200 ms`) into a quiet muted line (`ink-3`, `1 px` `line` border): `"Declined · Delete 'Return library books (duplicate)'"`. | Read-only historical record. |
| **Superseded** | If the owner types a refinement while a proposal is pending (`"Make it Tuesday instead"`), the old card collapses to `"Superseded by updated proposal"` and the new turn renders the revised card. | Read-only. |
| **Stale / Conflict on Approve** | If the target task was edited or deleted on another device while the proposal waited, Approve never overwrites silently. An inline `danger-bg` strip appears inside the card: `"This task changed since this proposal was made."` | **`Refresh proposal`** (re-reads task & regenerates diff) · **`Decline`**. |

### 2.8 Pending-proposal composer state

While one or more proposals in the open conversation remain unreviewed:

- An integrated **`32 px` pending banner** docks directly to the top edge of the composer frame (`wash` background, `1 px` bottom `line` hairline):
  - `14 px` `circle-alert` (`accent-strong`) + `"1 proposal waiting for review"` (or `"2 proposals waiting for review"`, `"4 of 5 proposed tasks selected"`) + quiet **`Jump up`** button that smoothly scrolls to the first pending proposal if the user has scrolled away.
- **Refinement stays open (never a dead lock):** The textarea remains editable with its placeholder updated to `"Approve above, or reply to adjust…"`. Typing a follow-up adjusts/supersedes the pending proposal rather than forcing a Decline-and-retype cycle.

### 2.9 Error states

All errors render as flat inline cards (`raised`, `radius::LG`, `1 px` `line-strong` border, `3 px` `danger` leading stripe) in plain language — never raw HTTP codes or JSON payloads, and never spinning icons:

| Condition | Plain-language message | Actions |
|---|---|---|
| **Your own server asleep / unreachable** | `"Your server isn’t responding. Couldn’t reach mac.tailnet.example. It may be asleep or off your private network."` | If a cloud fallback is configured (and `Ask before switching to a cloud provider` is enabled): privacy note (`"Switching to ChatGPT sends this chat to OpenAI. N private lists stay hidden."`) + **`Use ChatGPT (Cloud)`** + **`Retry`** + `Settings`. If no fallback: **`Retry`** + **`Choose another provider`**. |
| **App offline** | `"You’re offline."` *(If your own server on LAN/Tailscale is still reachable, read-only answers continue with a quiet `"Offline — reading local copy; approved changes will sync when back online"` strip; otherwise composer disables until reconnected.)* | **`Retry`** when back online. |
| **Rate limit / provider quota** | `"ChatGPT rate limit reached. Try again in 42 s."` (live `PAIRING_TICK` `1000 ms` mono countdown, zero spinner). | **`Retry`** (enabled at `0 s`) + **`Switch to Your own server`** (if configured & reachable). |
| **Auth expired / invalid API key** | `"Your ChatGPT sign-in expired."` or `"Your API key was rejected."` | **`Sign in again`** / **`Open AI settings`**. |
| **Tool / permission error** | `"Couldn’t apply that change — the list no longer exists."` or `"Health is hidden from AI in your privacy settings."` | **`Open AI settings`** (for hidden-list notice) or **`Retry`**. |

---

## 3. Web (`apps/web`)

The web app shares the desktop visual language and adapts cleanly across its three existing CSS breakpoints (`apps/web/static/app.css`):

| Viewport width | Trigger placement | Assistant presentation | Interaction notes |
|---|---|---|---|
| **Wide (`≥ 1200 px`)** | `[✦ Ask AI Ctrl J]` button placed in `AppBar` directly beside `[🔍 Filter Ctrl K]` (same `28 px` height, `radius::MD`). | Docked right column (`400 px` default, resizable `340–520 px` via the shared `detail_splitter` pattern; persisted in `localStorage`). At `≥ 1600 px`, docks beside Detail; at `1200–1599 px`, task links open Detail as an overlay beside the docked Assistant. | Identical keyboard shortcuts (`Ctrl+J` / `Cmd+J`, `Esc`). |
| **Medium (`760–1199 px`)** | `[✦ Ask AI]` button in `AppBar` beside `Filter` (keycap omitted). | Right-edge overlay drawer (`400 px` wide, `shadow-overlay`, `elevation::scrim`, `PANEL` `200 ms` slide-in). No splitter handle. | Clicking scrim or `Esc` closes drawer; tab focus is contained (`platform::contain_tab`). |
| **Narrow / Phone (`< 760 px`, `390 × 844` reference)** | `[✦]` `48 × 48 px` icon button in the narrow `AppBar` right beside `[🔍]` (`aria-label="Ask AI"`, `6 px` `accent` dot when a proposal is pending). | Full-width **`88 dvh` bottom sheet** (`radius::XL` `14 px` top corners, `32 × 4 px` handle, `elevation::scrim`). Bottom-gravity stream keeps the active proposal card and full-width `Decline` / `Approve` buttons (`36–44 px` tall) directly above the pinned Pending banner, Model & Privacy bar, and `48 px` rounded-12 Composer (`padding-bottom: env(safe-area-inset-bottom)`). | Respects strict web CSP (no inline scripts/styles; dynamic widths via CSSOM custom properties only). |

---

## 4. Android (`apps/android`, thumb-zone layout)

Follows every invariant of [`docs/android-thumb-zone.md`](android-thumb-zone.md) and [`docs/android-header.md`](android-header.md): **Sky above, hands below**, `48 dp` minimum touch targets, flat opaque surfaces, zero blur, and zero controls crossing the hinge.

### 4.1 Bottom-dock reachability (both postures)

- **Lower Dock (`phone::BOTTOM_BAR` `52 dp`):** Adds a single `48 × 48 dp` **`✦ Ask AI`** (`sparkles`) button directly beside Filter (`🔍`):
  - **Default (Right-handed):** `[≡ Rooms]` · `[Room Pills…]` · `[✦ Ask AI]` `[🔍 Filter]` `[⋯ Menu]`
  - **Left-handed cover mirror:** `[⋯ Menu]` `[🔍 Filter]` `[✦ Ask AI]` · `[Room Pills…]` · `[≡ Rooms]`
  - **Horizontal budget on `412 dp` cover:** `4 × 48 dp = 192 dp` for buttons, leaving **`220 dp`** for the horizontally scrollable `RoomPillsStrip` (2+ pills always visible; in Completed / Agent activity where `⋯` is omitted, `268 dp`).
  - **Active & pending cues:** Filled `wash` tile (`inset 6 dp`, `radius::MD`) + `accent-strong` icon when open on the unfolded inner screen; `6 dp` `accent` dot at upper-right when closed with a pending proposal.
  - **Swipe-up shortcut (optional accelerator):** Dragging upward from the `[✦]` target tracks 1:1 to pull open the Assistant Bottom Sheet (tapping `[✦]` is always the primary accessible trigger).

### 4.2 Folded cover screen (`412 × 960 dp`)

- **Presentation:** Bottom Sheet (`radius::XL` `14 dp` top corners, `pane` surface, `1 dp` `line-strong` top border in dark mode, `elevation::scrim` keeping the top `~16%` Living Daybreak Horizon sky visible).
- **Default height:** `phone::AI_SHEET_DEFAULT_FRACTION` (**`0.84`** ≈ `806 dp` on Fold7 cover), draggable between `0.55` and `0.92` (`SHEET_MAX_FRACTION`), or dragged down / predictive-back to dismiss.
- **Thumb-zone vertical layout (bottom-to-top, closest to thumb first):**
  1. **Gesture inset** (`navigationBarsPadding()` / `imePadding()`).
  2. **Chat Composer (`48 dp` min, auto-grows to `phone::AI_COMPOSER_MAX` `128 dp`, `radius::COMPOSER` `12 dp`):** `"Ask anything…"` (or `"Approve above, or reply…"`), trailing `48 dp` `mic` (voice dictation) + `48 dp` `arrow-up` Send (`accent-fill` when non-empty; swaps to `square` Stop while streaming).
  3. **Thumb-reachable Model & Privacy Strip (`36 dp` visual inside `48 dp` touch row):** `[● Private network · qwen3-14b ▾]` on left (opens bottom-anchored Provider/Model Quick Switcher sheet) + `[👁 1 list hidden]` on right (opens AI Settings sheet). Never forces a stretch to the top just to switch models!
  4. **Pending Proposal Strip (when active):** `32 dp` `wash` bar (`"1 proposal waiting · Tap Approve above"`).
  5. **Bottom-gravity Conversation Stream (`Arrangement.Bottom` when shorter than viewport):** Auto-scrolls so the active Proposal Card's **`48 dp` `Decline` / `Approve` buttons** sit squarely inside the **Comfortable green thumb zone (`y = 600..820 dp`)**.
     - Button order on Cover: `[Decline]` (left) · `[Approve]` (right, directly under the right thumb). Mirrors in `Left-handed` mode for Create/Edit proposals; for Destructive (`DELETE`) proposals, `Decline` stays on the dominant thumb side so accidental taps never delete!
  6. **Compact Sheet Header (`44 dp` below `32 × 4 dp` drag handle):** `A` mark + conversation title (`Today’s plan ▾`, opens Conversation History bottom sheet in place via `CONTENT_SWAP`) + `+` New + `⚙` Settings + `×` Close.
- **Short / landscape window (`maxHeight < 480 dp`):** Sheet opens to `0.95` height, collapses drag handle padding, and hides the privacy strip while IME is open so at least the active proposal buttons or last reply stay visible above the keyboard.

### 4.3 Unfolded inner screen (`876 × 970 dp`, Galaxy Z Fold7)

- **Left Pane (`438 dp`, left-thumb domain):** Unchanged Room view (`80 dp` Living Daybreak Horizon header, task rows, `56 dp` Quick-Add Composer, `52 dp` Bottom Utility Bar with `[✦]` highlighted active).
- **Right Pane (`438 dp`, right-thumb domain):** **Assistant docks directly as the Right Pane** (zero hinge crossing; swaps cleanly with Task Detail over `CONTENT_SWAP` `120 ms`).
  - **Top (`40 dp`, exact match to `phone::DETAIL_BREADCRUMB`):** Non-interactive glanceable strip showing `A` mark + conversation title on the left and quiet connection/privacy status on the right (`Private network` / `Cloud` / `Server unreachable`). Zero top buttons!
  - **Middle:** Bottom-gravity scrollable conversation stream (`paddingHorizontal = S5 = 20 dp`). Proposal buttons (`48 dp` tall) rest in the lower-right thumb arc (`x = 606..876 dp`, `y = 540..850 dp`).
  - **Bottom (aligned to the exact horizontal baseline of the Left Pane's 2-row Bottom Dock!):**
    - **Upper Dock (`56 dp`):** `48 dp` rounded-12 AI Composer (`Ask about Home…`, `mic`, `arrow-up`).
    - **Lower Dock (`52 dp` `phone::BOTTOM_BAR`, `canvas` bg):** `[🕒 History]` · `[● Private · qwen3-14b ▾]` (or `[● Server asleep · qwen3 ▾]`) · `[+ New]` `[⚙ Settings]` `[× Close]`. Every button is `48 dp` tall, directly under the right thumb.
- **Tapping a task link on Fold7 Inner:** Immediately highlights and scrolls to that task in the **Left Pane** so the owner sees it live while keeping chat open; tapping the row again (or an inline `Open detail` action) swaps the Right Pane to Task Detail (`CONTENT_SWAP`) with `[✦]` in the Left Bottom Bar ready to return to the exact preserved chat in one tap.

---

## 5. Provider settings, onboarding & privacy cues

Settings are reachable from three places: (1) `⚙` in the Assistant panel/sheet, (2) tapping the Model & Privacy strip, and (3) **`AI assistant…`** (`sparkles`) in the app's main Settings menu (placed above `Devices…`). On Desktop & Web, it opens as a `520 px` (`desktop::AI_SETTINGS_WIDTH`) opaque modal dialog (`pane`, `radius::XL`, `shadow-overlay`); on Android, as a pane-aligned Bottom Sheet (`CONTENT_SWAP` from `SettingsSheet`).

### 5.1 Three model providers

| Provider | Privacy classification | Configuration fields & states |
|---|---|---|
| **1. Your own server** *(OpenAI-compatible endpoint, e.g. Ollama / LM Studio running on a Mac over a private Tailscale network)* | **`Private network`** (`success` dot when connected; `danger` / `ink-4` when asleep/unreachable) | • **Base URL:** mono text input, placeholder `http://mac.tailnet.example:11434` (auto-handles `/v1` suffix; permits `http://` on loopback and private/Tailscale hosts, warns before saving unencrypted public URLs).<br>• **Optional API key / token:** collapsed `"Add bearer token…"` disclosure with `eye` / `eye-off` toggle.<br>• **Model picker:** populated live from `GET <base>/v1/models` (e.g. `qwen3-14b`, `llama-3.3-70b`) with `Refresh` support.<br>• **`Test` button & live status line:** `"Connected · 4 models available · 38 ms"` (`success` dot) or `"Couldn’t reach mac.tailnet.example — server may be asleep or off your private network"` (`danger` dot). |
| **2. ChatGPT account** *(OpenAI)* | **`Cloud · OpenAI`** (`sun-ink` dot) | • **Signed out:** `"Sign in with your ChatGPT account"` (`log-in` button) + clear note `"Prompts and visible tasks are sent to OpenAI."`<br>• **Signed in:** `"Signed in · ChatGPT account"` (`success` dot) + **Model picker** (`GPT-5`, `GPT-5 mini`, `o4-mini`) + **`Sign out`** (`log-out`). *(Auth flow mechanics are engineered separately.)* |
| **3. API key** *(OpenAI, Anthropic, or custom OpenAI-compatible)* | **`Cloud · <Provider>`** (`sun-ink` dot; or `Private network` if custom URL points to a private/loopback host) | • **Provider preset:** `OpenAI` \| `Anthropic` \| `Custom`.<br>• **Base URL** (shown for `Custom`).<br>• **API key:** masked input (`eye` / `eye-off`), stored in OS secure storage (DPAPI / Android Keystore / server-side encrypted store, never logged or committed).<br>• **Model picker** + **`Test`** connection check. |

### 5.2 Server asleep / unreachable & explicit cloud fallback

Because a personal Mac on Tailscale may sleep when the owner is away from their desk:

- **Fallback selector:** `"When your server is asleep or unreachable"` → dropdown choosing `None` \| `ChatGPT account` \| `API key` (only configured secondary providers appear).
- **Explicit cloud consent (checked by default):** `[✓] Ask before switching to a cloud provider`.
  - When checked and `mac.tailnet.example` times out (short connect timeout, e.g. `4 s`), the app **never silently routes prompts to OpenAI**. Instead, the inline error card (§2.9) offers a single-click/single-tap **`Use ChatGPT (Cloud)`** action accompanied by a plain privacy reminder (`"Switching to ChatGPT sends this chat to OpenAI. 2 private lists stay hidden."`) and **`Retry`**.
  - Choosing `Use ChatGPT (Cloud)` switches the active session to Cloud (updating the Model & Privacy bar immediately to `● Cloud · ChatGPT · GPT-5`) and offers a quiet `"Your server is back — switch back to Private network"` nudge once a periodic or manual check succeeds.
  - If the user explicitly unchecks `Ask before switching to a cloud provider`, fallback happens automatically with a clear inline status note above the reply (`"Your server was unreachable — answered using Cloud · ChatGPT"`).

### 5.3 Privacy indicators & per-list visibility

1. **Always-visible destination indicator:** Directly beside the chat composer on every platform:
   - **Private network:** `success` `6 px` dot + **`Private network`** + `· <model>` (tooltip: `"Sent only to mac.tailnet.example on your private network."`).
   - **Cloud:** `sun-ink` `6 px` dot + **`Cloud · ChatGPT`** (or `Cloud · API`) + `· <model>` (tooltip: `"Sent to OpenAI’s cloud servers. Lists marked Private only or Hidden are excluded."`).
   - **First cloud message disclosure:** The very first time the owner sends a prompt to a Cloud provider, a quiet dismissible inline notice states: `"Only the tasks and lists the assistant reads to answer your question are sent — never excluded lists or file attachments."`
2. **Per-list visibility ("Lists the assistant can see"):**
   - Mirrors the server's existing per-list agent access model (`docs/agents/guide.md`): hidden lists do not exist to the assistant — they are omitted from list enumerations, search queries, and task lookups.
   - Each list and collection header offers a 3-way choice (default **`Hidden`**). The collection
     choice applies to all current and future members, replacing current list overrides. Individual
     choices can then override the default; **`Mixed`** means members differ.
     - **`All models`** — visible to both Private network and Cloud providers.
     - **`Private only`** (`eye-off` in `sun-ink`) — visible to **Your own server**, completely hidden whenever using **ChatGPT** or a **Cloud API key**. Ideal for lists like `Finance`!
     - **`Hidden`** (`eye-off` in `ink-3`) — never visible to any AI model (e.g. `Health`).
   - The Model & Privacy strip reflects the live active count (`1 list hidden` on Private network; `2 hidden from cloud` on Cloud), and tapping it opens the list-access settings directly.

---

## 6. `todo-design` token mapping, motion & accessibility

### 6.1 Reused existing tokens

All existing tokens from `crates/todo-design` remain authoritative:

- **Palette (`palette.rs`):** `canvas`, `pane`, `raised`, `hover`, `wash`, `tint`, `line`, `line-strong`, `control`, `ink`, `ink-2`, `ink-3`, `ink-4`, `accent`, `accent-strong`, `accent-fill`, `on-accent`, `sun-ink`, `danger`, `danger-bg`, `danger-ink`, `success`, `toast`, `toast-ink`, `toast-accent`, and `ListColors` / `AgentColors`.
- **Typography (`typography.rs`):** **Zero new text styles needed.** Uses `DETAIL_TITLE`, `ROOM_TITLE_BAR`, `TASK_TITLE`, `NOTES` (assistant prose, `14/24`), `BODY`, `QUICK_ADD`, `COMPOSER`, `NAV_SUB`, `META`, `META_PHONE`, `CAPS_LABEL`, `DUE`, `TAG`, `PRIORITY`, `COUNT`, `BUTTON`, `BUTTON_SMALL`, `KEYCAP`, `AGENT_LETTER`.
- **Spacing & Radii (`layout.rs`):** `space::S1..S6` (`4..24`); `radius::SM` (`4`), `MD` (`6` — suggestion chips, buttons, task links), `LG` (`10` — proposal cards, user messages, popovers), `XL` (`14` — settings dialog, bottom sheets), `CHIP` (`10`), `COMPOSER` (`12`).
- **Icons (`icons.rs`):** **Zero new SVGs needed.** Uses already-bundled Lucide icons: `sparkles`, `arrow-up`, `square`, `plus`, `pencil`, `trash-2`, `check`, `x`, `clock`, `settings`, `chevron-down`, `chevron-right`, `chevron-left`, `arrow-up-right`, `circle-alert`, `cloud-off`, `refresh-cw`, `eye`, `eye-off`, `log-in`, `log-out`, `undo-2`, `sun`, `sun-filled`, `calendar`, `flag`, `list`, `search`, `mic`.

### 6.2 New additive tokens (added in `crates/todo-design`)

All 13 additive tokens below are added in `crates/todo-design/src/hues.rs` and `crates/todo-design/src/layout.rs` and exported to `generated/tokens.css` and `generated/tokens.json`:

| Module | Rust constant | CSS / JSON key | Value | Purpose |
|---|---|---|---|---|
| `hues::AGENTS` | `named("assistant", 192.0)` | `[data-agent="assistant"]` / `agents.assistant` | `192°` (lagoon-teal) | In-app `Assistant` agent letter mark (`A`) background, ink, and `1 px` inset ring. |
| `layout::desktop` | `AI_PANEL` | `--desktop-ai-panel` | `400.0` (px) | Default docked AI assistant panel width (and narrow overlay drawer width). |
| `layout::desktop` | `AI_PANEL_MIN` | `--desktop-ai-panel-min` | `340.0` (px) | Narrowest resizable docked AI assistant panel. |
| `layout::desktop` | `AI_PANEL_MAX` | `--desktop-ai-panel-max` | `520.0` (px) | Widest resizable docked AI assistant panel (clamped by `LIST_MIN_BESIDE_DETAIL`). |
| `layout::desktop` | `FOUR_PANE_MIN` | `--desktop-four-pane-min` | `1600.0` (px) | Narrowest window that docks Sidebar, List, Task Detail, and AI Assistant together. |
| `layout::desktop` | `AI_COMPOSER_MIN` | `--desktop-ai-composer-min` | `44.0` (px) | Single-line minimum height of the desktop/web AI chat input (`QUICK_ADD`). |
| `layout::desktop` | `AI_COMPOSER_MAX` | `--desktop-ai-composer-max` | `140.0` (px) | Maximum auto-grow height of the desktop/web AI composer before scrolling. |
| `layout::desktop` | `AI_HISTORY_MENU` | `--desktop-ai-history-menu` | `280.0` (px) | Width of the anchored conversation history popover. |
| `layout::desktop` | `AI_SETTINGS_WIDTH` | `--desktop-ai-settings-width` | `520.0` (px) | Width of the AI Provider & Privacy Settings modal dialog on desktop/web. |
| `layout::phone` | `AI_SHEET_DEFAULT_FRACTION` | `--phone-ai-sheet-default-fraction` | `0.84` | Default open height of the Android cover AI Assistant bottom sheet. |
| `layout::phone` | `AI_COMPOSER_MAX` | `--phone-ai-composer-max` | `128.0` (px/dp) | Maximum auto-grow height of the phone AI chat composer (~4 lines). |
| `layout::component` | `PROPOSAL_BAR` | `--component-proposal-bar` | `3.0` (px/dp) | Width of the leading semantic stripe on proposal cards (`accent`, `sun-ink`, `danger`). |
| `layout::component` | `PRIVACY_DOT` | `--component-privacy-dot` | `6.0` (px/dp) | Status dot diameter in the Model & Privacy indicator (`CHIP_DOT`). |

### 6.3 Motion & reduced-motion rules

**Zero new motion tokens needed.** Every moment reuses `todo_design::motion` and strictly upholds the no-spinner / no-pulse rule:

| Moment | Motion (`todo_design::motion`) | Reduced-motion fallback |
|---|---|---|
| **Open / close docked panel or overlay drawer** | `PANEL` (`200 ms`, `ENTER` / `EXIT`); desktop docked content fades + `8 px` horizontal glide; Android cover sheet tracks drag 1:1 and settles over `PANEL`. | `REDUCED_FADE` (`100 ms`) opacity only, `0 px` translation (direct touch drag stays 1:1). |
| **Swap Detail ↔ Assistant (Fold7 inner) or switch conversation** | `CONTENT_SWAP` (`120 ms`, `STANDARD`) in-place crossfade. | `REDUCED_FADE` (`100 ms`). |
| **New message, step line, or proposal card entering stream** | `ROW_ENTER` (`180 ms`, `ENTER`, `opacity: 0 → 1`, `translateY: 6 px → 0`). | `REDUCED_FADE` (`100 ms`) opacity only. |
| **Step log auto-collapse / manual expand** | `PANEL` (`200 ms`, `STANDARD`), chevron rotates `0° ↔ 90°`. | Snap (`0 ms` height/chevron), `REDUCED_FADE` (`100 ms`) opacity. |
| **Proposal card → Approved / Declined receipt** | `PANEL` (`200 ms`, `STANDARD`) height settle + `CONTENT_SWAP` (`120 ms`) receipt crossfade. | Snap (`0 ms` height), `REDUCED_FADE` (`100 ms`) crossfade. |
| **Approved task appearing / updating in Task List** | Existing `SYNC_HIGHLIGHT` (`900 ms`, `EXIT`, `wash → pane`); never slides or jumps rows under the user's cursor. | Hold `wash` for `COMPLETE_HOLD` (`450 ms`), then fade out in `REDUCED_FADE` (`100 ms`). |
| **History popover, model switcher, settings dialog** | `OVERLAY_IN` (`150 ms`, `ENTER`) / `OVERLAY_OUT` (`120 ms`, `EXIT`). | `REDUCED_FADE` (`100 ms`), `scale: 1.0`, `0 px` shift. |
| **Rate-limit retry countdown** | Functional `PAIRING_TICK` (`1000 ms`) mono text update. | Unchanged (`1000 ms` functional timer). |
| **While waiting for model / tool execution** | **No spinner, no pulsing dot, no skeleton shimmer.** Calm plain text (`"Reading tasks in Work…"`) and incremental token streaming. | Identical. |

### 6.4 Accessibility

- **Contrast (WCAG 2.x):** Every text element meets **≥ 4.5:1** (`ink`, `ink-2`, `ink-3`, `accent-strong`, `on-accent`, `danger-ink`, `list-ink`, `agent-ink`) across all 360° room hues in both Light and Dark themes. The `3 px` proposal leading bars (`accent`, `sun-ink`, `danger`) all meet **≥ 3:1** on `raised` and are always paired with a Lucide icon + mono caps label (`NEW TASK`, `EDIT TASK`, `DELETE TASK`) so meaning never relies on colour alone.
- **Screen readers & live regions:**
  - Step log status updates use a polite live region (`aria-live="polite"`, debounced so rapid steps don't flood speech).
  - Pending proposals announce concisely when ready (`"Proposal ready: Edit task Review quarterly budget. Approve or Decline."`).
  - Proposal cards expose `role="region"` with `aria-label` naming kind and title; diff values include screen-reader context (`"Due changed from Thursday 19 November to Monday 23 November"`).
  - The Model & Privacy strip exposes its full meaning in one accessible description (e.g. `"AI model: qwen3-14b on your private network, connected. 1 list hidden. Activate to change provider."`).
  - On Android, every interactive control in the sheet, proposal cards, and inner right pane keeps a `≥ 48 dp` hit target (`phone::TOUCH`), and the top `40 dp` inner breadcrumb stays non-interactive with `semantics { heading() }`.
- **Keyboard navigation (Desktop & Web):**
  - `Ctrl+J` / `Cmd+J` toggles panel and moves focus to composer.
  - `Enter` sends message; `Shift+Enter` inserts newline.
  - `Tab` cycles cleanly through header controls → task links → proposal Approve/Decline buttons → pending jump link → composer → privacy/model switcher.
  - On destructive proposal cards (`DELETE TASK` / `MERGE TASKS`), **Decline** precedes **Delete** in both visual and DOM/Tab order so hasty `Tab + Enter` never deletes.
  - Large font scale (`1.3×–2.0×`): Proposal field grids wrap value below key when narrow, `proposal-actions` stack vertically if button labels exceed half-width, and composer grows cleanly without clipping.

---

## 7. Visual mockups

All mockups live in [`docs/mockups/ai-assistant/`](mockups/ai-assistant/) and are built directly from `crates/todo-design/generated/tokens.css`, bundled Geist / Geist Mono fonts, authentic project Lucide icons & hand-drawn list glyphs, and neutral sample data (`Groceries`, `Home`, `Work`, `Finance`, `Errands`, `Garden`, `Health`, `mac.tailnet.example`). Open [`docs/mockups/ai-assistant/index.html`](mockups/ai-assistant/index.html) for the interactive gallery.

| Screen / State | Viewport | HTML source | Light PNG | Dark PNG |
|---|---|---|---|---|
| **Desktop — Empty state & suggestion chips** | `1440 × 900` | `desktop-empty.html` | `desktop-empty-light.png` | `desktop-empty-dark.png` |
| **Desktop — Pending Edit & Destructive proposals + live highlight** | `1440 × 900` | `desktop-proposal.html` | `desktop-proposal-light.png` | `desktop-proposal-dark.png` |
| **Desktop — Batched proposal (5 tasks) & Cloud privacy cue** | `1440 × 900` | `desktop-batch.html` | `desktop-batch-light.png` | `desktop-batch-dark.png` |
| **Web Narrow — 88 dvh sheet & single Create proposal** | `390 × 844` | `web-narrow.html` | `web-narrow-light.png` | `web-narrow-dark.png` |
| **Android Cover — 84% thumb-zone sheet & Edit proposal** | `412 × 960` | `android-cover.html` | `android-cover-light.png` | `android-cover-dark.png` |
| **Android Inner — Fold7 docked right pane & Server asleep fallback** | `876 × 970` | `android-inner.html` | `android-inner-light.png` | `android-inner-dark.png` |
| **Provider & Privacy Settings** (Your own server, ChatGPT, API key, fallback, list access) | `1440 × 900` | `provider-settings.html` | `provider-settings-light.png` | `provider-settings-dark.png` |

Re-render all 14 PNGs at 2× from the repo root:

```sh
cd apps/web/e2e && npm ci --ignore-scripts && cd ../../.. && node docs/mockups/ai-assistant/render.mjs
```

---

## 8. Phased implementation checklist

### Phase 0 — Shared core, design tokens & assistant view-model (`crates/todo-design`, `crates/todo-core`, `crates/todo-client`)

- [x] Add `assistant` (`192°`) agent hue and the 12 layout/component tokens to `crates/todo-design` and regenerate `tokens.css` + `tokens.json` (`TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated`).
- [ ] Add shared pure view-model & state types in `crates/todo-client` (no I/O, wasm32 + Android portable):
  - `Actor::Agent { agent_id: "assistant" }` attribution formatting (`"Assistant"`, letter mark `'A'`, hue key `"assistant"`).
  - Proposal model (`ProposalKind::Create | Edit | Move | Complete | Delete | Merge`, structured fields, `old → new` diff builder, batch selection state, conflict check against live `todo_core::State`).
  - Step log model (compact summary builder + expandable step list + skipped private list count).
  - Provider & privacy configuration model (`OwnServer { base_url, model, has_token }`, `ChatGptAccount { model }`, `ApiKey { preset, base_url, model }`, fallback provider + `ask_before_cloud_fallback`, per-list `ListAiAccess::AllModels | PrivateOnly | Hidden`).
  - List-visibility filter ensuring `Hidden` lists (and `PrivateOnly` lists on cloud providers) are stripped before any tool query or model context serialization.
- [ ] Add unit & property tests verifying: hidden lists never leak into serialized context, approving a proposal produces valid idempotent `Command`s with exact undo, and stale targets trigger conflict detection instead of silent overwrites.

### Phase 1 — Desktop GPUI (`apps/desktop`, visual reference)

- [ ] **Shell & trigger:** Add `[✦ Ask AI Ctrl J]` button to `titlebar()` (`apps/desktop/src/ui/shell.rs`), `ToggleAiPanel` action bound to `Ctrl+J`, pending-proposal indicator dot when closed, and `AI assistant…` entry in the account/settings menu.
- [ ] **Docked panel & splitter:** Add `Pane::Ai` to `model::layout` with `AI_PANEL` (`400`), `AI_PANEL_MIN` (`340`), `AI_PANEL_MAX` (`520`), `FOUR_PANE_MIN` (`1600`), keyboard-accessible splitter, narrow `< 1200` overlay drawer, `1200–1599` Detail overlay beside docked Assistant when following task links, and `ui.json` width persistence.
- [ ] **Conversation UI:** Panel header (`48 px`), anchored `280 px` History popover, empty state (ready + zero-provider onboarding) with suggestion chips, compact step log (live + collapsible), prose with clickable task links, single & batched Proposal Cards (Create / Edit / Delete / Summary / Review-each / Settled receipts with tracked Undo / Conflict state), pending-proposal composer banner, and all error cards (Server asleep + explicit Cloud fallback consent, Offline, Rate limit countdown, Auth expired).
- [ ] **Provider & Privacy Settings dialog (`520 px`):** Your own server (Base URL, `/v1/models` fetch, `Test` with latency/error), ChatGPT account status card, API key (DPAPI sealed), fallback & `Ask before switching to a cloud provider`, and per-list `All models` / `Private only` / `Hidden` picker.
- [ ] **Screenshot gate:** Add deterministic `--fixture` views (`ai_empty`, `ai_proposal`, `ai_batch`, `ai_settings`) in light & dark at 100% and 150% scaling to `apps/desktop/scripts/gallery.ps1`.

### Phase 2 — Web Leptos (`apps/web`)

- [ ] **Responsive shell:** Add `Ask AI` trigger to `AppBar` (wide/medium beside `Filter`; narrow `< 760 px` `48 × 48` icon button), `Ctrl+J` / `Cmd+J` shortcut, docked resizable right column at `≥ 1200 px` (`localStorage` width persistence), `400 px` overlay drawer at `760–1199 px`, and `88 dvh` bottom sheet at `< 760 px` (`env(safe-area-inset-bottom)`).
- [ ] **Conversation, proposals & settings:** Mirror desktop feature parity using `tokens.css` variables, strict CSP compliance (zero inline styles/scripts), `aria-live` step/proposal announcements, `contain_tab` focus management on overlays/dialogs, and IndexedDB local conversation history.
- [ ] **Browser tests & gallery (`apps/web/e2e`):** Add Playwright tests covering keyboard `Ctrl+J` / `Esc`, proposal approve → live `SYNC_HIGHLIGHT` + Undo, batch select/deselect, narrow viewport bottom sheet, zero CSP violations, and gallery screenshots in light & dark.

### Phase 3 — Android Compose (`apps/android` + `crates/todo-ffi`)

- [ ] **FFI bridge (`crates/todo-ffi`):** Expose the shared assistant view-model, proposal approval/decline/undo commands, provider/privacy config, and list-visibility filter through UniFFI.
- [ ] **Bottom-dock trigger (`RoomPane.kt`):** Add `48 dp` `[✦]` button to `BOTTOM_BAR` (respecting `Left-handed` mirroring on cover, pending-proposal badge dot, and active tile highlight on inner screen).
- [ ] **Cover bottom sheet (`412 × 960 dp`):** Build `AiAssistantSheet` (`AI_SHEET_DEFAULT_FRACTION` `0.84`, draggable `0.55..0.92`, `PANEL` settle, predictive back, `imePadding()`, bottom-gravity stream placing `48 dp` `Decline` / `Approve` buttons in the green thumb zone, thumb-zone Model & Privacy strip above `48 dp` composer with `mic` + `arrow-up`, in-place `CONTENT_SWAP` History and Settings sub-sheets).
- [ ] **Inner docked right pane (`876 × 970 dp`):** Dock Assistant in the Right Pane (`40 dp` glanceable top breadcrumb, bottom-gravity stream, aligned `56 dp` Composer + `52 dp` Bottom Utility Bar matching Left Pane baseline, zero hinge crossing). Wire task links to highlight in the Left Pane with smooth Detail swap.
- [ ] **Paparazzi gallery & interaction tests (`GalleryTest.kt`):** Add `FOLD7_COVER` (`cover_ai_sheet`, `cover_ai_batch`, `cover_ai_settings`) and `FOLD7_INNER` (`inner_ai_pane`, `inner_ai_server_asleep`) snapshots in light & dark, plus large-font (`1.5×`), left-handed mirror, and reduced-motion (`ANIMATOR_DURATION_SCALE == 0f`) tests.
