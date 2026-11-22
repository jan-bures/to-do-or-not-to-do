# Web app

`apps/web` is the browser client. It is Rust compiled to `wasm32-unknown-unknown`, with a
[Leptos](https://leptos.dev) UI rendered on the client (CSR). It uses the same `todo-core` and `todo-client`
as the other clients, so the state, the commands, the smart-list queries and the sync engine are exactly the
desktop's and Android's. `todo-server` serves it at `/app`, and it signs in with the owner's passkey session:
there is no token in the browser.

The web app covers sign-in, smart lists, collections, lists, task detail, adding, completing (with the F2
check animation), and live updates. Edit titles, steps, notes, due dates, priority, My Day, repeat rules,
reminders, tags and web links in detail. Upload, download and remove attachments. Create, rename, recolour, reorder
and delete lists and collections, and move lists between collections. Activity shows confirmed changes
and lets you undo past events.
Undo compensates the exact change. It has the desktop layout on wide
windows and the phone layout on narrow ones, in both themes.

## Building and running

```sh
rustup target add wasm32-unknown-unknown              # the toolchain file already lists it
cargo install wasm-bindgen-cli --version 0.2.130 --locked   # must match apps/web/Cargo.lock
node apps/web/scripts/build.mjs                       # -> apps/web/dist
node apps/web/scripts/build.mjs --fixtures            # -> apps/web/dist-fixtures (gallery build)

cd apps/web
cargo fmt --all
cargo clippy --all-targets -- -D warnings             # host: the view-model and its tests
cargo clippy --target wasm32-unknown-unknown -- -D warnings
cargo test                                            # view-model, landscape and fixture tests

cd e2e && npm ci && npx playwright install chromium
npx playwright test                                   # browser tests + screenshot gallery (fixtures build)
npx playwright test -c live.config.mjs                # against a real todo-server (see "Tests")
```

The [Linux release](hosting-linux.md#deploy) includes the production build and switches it with the
server. The Linux config template sets `[web] dir = "/opt/todo/current/web"`. For a local or manual
build, set `[web] dir` (or `TODO_SERVER_WEB_DIR`) to the path of `apps/web/dist` and restart the server.

With a build configured, `/` opens the app and `/app/` serves the same session-gated page as `/app`.
Without a build, `/` opens the admin pages as before.

The Activity release adds the `events_actor` index migration. A server binary that predates this
migration refuses the migrated database. To roll back to that binary, restore the database copy taken
before deployment; see the [Linux deploy recovery procedure](hosting-linux.md#an-interrupted-deploy).

### Its own workspace

`apps/web` is a separate Cargo workspace with its own `Cargo.lock`, like `apps/desktop`. Leptos and its
dependencies stay out of the root lock, and the root `cargo … --workspace` jobs don't build it. The CI job
`web app (wasm, browser tests)` lints, tests and builds it, and runs the browser tests.

### No Trunk, no bundler

`scripts/build.mjs` runs `cargo build --target wasm32-unknown-unknown --release`, then `wasm-bindgen
--target web`, and copies `static/` (the page, `boot.js`, `app.css`), `tokens.css` and the bundled fonts next
to the output. Trunk would have been the usual choice, but it injects an inline bootstrap `<script>`, and
the page's CSP allows no inline script. `boot.js` is a module file that sets the theme and starts the wasm
module. The script also writes gzipped copies of the larger files (`*.gz`), which the server sends to
browsers that accept gzip.

## Layout of the code

| File | What |
|---|---|
| `src/model.rs` | The view-model: sidebar counts, a room's header and rows, the detail pane, tomorrow's plan. Pure Rust over `todo_core::State`, unit-tested on the host. |
| `src/landscape.rs` | The header landscape as SVG markup, from `todo_design::landscape`. Shapes carry classes; `app.css` colours them from the room's tokens. |
| `src/app.rs` | The Leptos components. They set classes and `data-*` hue keys only. |
| `src/backend.rs` | The live backend (the sync engine) or the fixtures backend, including exact undo and dismissal of refused changes. |
| `src/app/activity.rs` | The activity room, day groups, older pages and per-event undo feedback. |
| `src/app/room_actions.rs` | List and collection menus, move and colour pickers, rename and delete dialogs, and the Not synced list. |
| `src/app/reminder.rs`, `repeat.rs`, `tags.rs`, `links.rs` | Task detail editors using the shared client intents. `pickers.rs` shares the anchored controls and swatches. |
| `src/app/attachments.rs` | File picker, drops onto the detail pane, upload status, and download and removal controls. |
| `src/platform.rs` | Browser helpers: clock and UTC offset, theme, history, local fold preferences, CSRF token, downloads. |
| `src/fixtures.rs` | Invented gallery data with a fixed clock (Saturday 21 November 2026, 10:35). |
| `static/app.css` | The layout. Every colour, size, radius, font and duration is a `tokens.css` variable. |
| `crates/todo-client/src/web.rs` | The browser platform for the sync engine (behind the `web` feature). |
| `crates/todo-client/src/mirror.rs` | `MirrorStorage`: the replica's storage, journalled for IndexedDB. |
| `crates/todo-server/src/web/spa.rs` | Serving the build at `/app`. |

## Serving: why `/app`, and from memory

The app shares its origin with the server's own pages (`/login`, `/setup`, `/pair`, `/admin`), the API
(`/api/v1`), MCP and share links (`/s/…`). A path prefix keeps the app's client-side routes (`/app/important`,
`/app/list/<id>/task/<id>`) from ever colliding with a server route, and it lets every `/app` route sit behind
the session gate in one place: a signed-out browser is sent to `/login?next=/app…` and comes back after
signing in. The same origin is what makes the passkey session usable at all: the cookie is
`__Host-`, `SameSite=Strict`, and never leaves this origin.

The build is a directory named in the config, read **into memory at start-up** (`WebApp::load`):

- The server binary doesn't depend on the wasm toolchain, and a server build doesn't rebuild the app. Deploying
  the app means replacing the directory and restarting the server.
- A request never touches the file system. Paths are looked up in a map, so nothing outside the build can be
  named, and there is no traversal to get wrong.
- Only known file types are loaded, at most 512 files and 64 MB in all; `index.html` must hold the session
  marker `<!--todo-session-->` exactly once, or the server refuses to start.

`index.html` is sent with the session's CSRF token written in place of the marker, so it is
`Cache-Control: no-store, no-transform`: never cached, and never compressed by a proxy in front (Cloudflare
honours `no-transform`). Compressing a response that holds a secret next to text an attacker can influence
leaks the secret through the compressed size (BREACH), so every response that may carry one gets the same
header: the server's other pages (CSRF form fields), the API's JSON (pairing and agent tokens) and
`/auth/session`. Private attachment downloads (the owner's files, which can quote anything) get
`private, no-store, no-transform`. Only the app's static files, which hold no secret, are compressed, by the
build. They carry an `ETag` and `private, no-cache`, so a deploy is picked up on the next load.

### Headers

The page gets its own CSP:

```
default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data: blob:;
connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self';
object-src 'none'
```

`'wasm-unsafe-eval'` allows compiling the wasm module and nothing else; there is no `'unsafe-inline'` or
`'unsafe-eval'`. Styles come from the two stylesheets. Dynamic values are chiefly classes and `data-*`
attributes, and the landscape's stroke widths and opacities are SVG presentation attributes. Room menus set
only their measured screen coordinates via CSSOM custom properties; they don't need CSS anchor positioning
or inline style markup. Browser tests fail on any CSP violation. The rest of the server's headers
(`Referrer-Policy: same-origin`, `X-Content-Type-Options`, ...) apply as on every page.

## Signing in and the API

The app uses the owner's existing passkey session. Nothing new is stored in the browser: no token, no
password, no key.

- **The session cookie** (`__Host-todo_session`, `HttpOnly`, `Secure`, `SameSite=Strict`) is sent with every
  same-origin `fetch`.
- **The CSRF token** reaches the app in `<meta name="csrf-token">`, written into the page by the server per
  request. The app sends it in `X-CSRF-Token` on **every** API request, reads included. A custom header
  can't be sent cross-origin without a CORS preflight, which the server never grants, so this alone stops a
  foreign page from using the session. The token must be the session's own: a token from an earlier session
  is refused.
- **The origin check** (`check_origin`, the same as the admin pages) runs on every write: `Origin` must be the
  server's public origin.

`auth::require_auth_or_session` implements this for `/api/v1` only. A request with an `Authorization`
header always takes the bearer path, unchanged: agents and paired devices work exactly as before, and a
wrong token is a 401 even with a live session cookie. The attachment upload (`POST /api/v1/tasks/{id}/attachments`)
uses the same middleware with its own body limit and deadline. MCP and the imported-attachment content fill
(`PUT /api/v1/attachments/{id}/content`) stay bearer-only. A session acts as the owner on the device `web` (`Actor::Owner { device: "web" }`), so its changes are attributed like
any other device's. The session's event stream is closed when the session ends (sign-out, expiry), checked
on revocation and on every heartbeat.

Only a request that passes the CSRF (and, for writes, the origin) check counts as activity for the session's
idle timeout.

**A stale token.** The session behind the cookie can change while a tab is open: signing in again in another
tab replaces the cookie, and the open tab's token no longer fits it. The API answers `403 csrf`. The
transport (`todo_client::web::FetchTransport`, through `todo_client::session::with_fresh_token`) then reads
the session's current token from `GET /auth/session` and, if it differs from **the token the refused request
was sent with**, sends the same request once more with it, with the same body and so the same command ID:
the server applies it once. Comparing with the token sent, not with the tab's current one, matters when the
event stream and a command are refused at the same time: whichever refreshes first updates the shared token,
and the other must still retry rather than take the new token for "unchanged". One refresh and one retry per
request. If the token hasn't changed, the refusal was about something else (the `Origin`) and stands for now:
the engine treats a `403 csrf` as "retry later" with backoff, never as a verdict on the command, so queued
changes are never marked "couldn't sync" for it. If the refresh itself fails, the request counts as a network
failure and is retried later; a 401 from it means signed out. The page uses the refreshed token too
(sign-out, downloads). An attachment upload uses the same token refresh: it sends the browser `File` as
its raw body and reuses its `attachment_id` if it has to send it again. The attachment arrives in the
replica over the event stream, not through a separate local edit.

`GET /auth/session` (cookie only, `{"csrf": "..."}`, a 401 without a session) answers this origin only: it
refuses a request whose `Sec-Fetch-Site` is anything but `same-origin` (so `same-site`, `cross-site` and a
typed-in `none` too) or whose `Origin`, if present, isn't the server's. And it never counts as activity: it
looks the session up without touching it, so a sibling origin or a forgotten tab can't keep an idle session
alive through it.

When the session is gone (the stream or a request gets a 401), the app sends the browser to
`/login?next=<the current path>`. Changes not yet sent stay in IndexedDB and go out after signing in.

## The event stream: a streaming `fetch`, not `EventSource`

The engine reads `GET /api/v1/events` (server-sent events) with `fetch` and a `ReadableStream`, parsed by the
shared `todo_client::sse::SseParser`. `EventSource` doesn't fit:

- it can't send headers, so it can't send the CSRF token that the API requires on every request;
- it hides the response: the engine needs the status and body of a refused stream (`409 catch_up` or
  `epoch_mismatch` to page or resync, `429` with `Retry-After`, `401` to sign in again);
- it reconnects on its own schedule, outside the engine's backoff and its idle timeout.

With `fetch` the browser platform is the same `Transport`/`EventStream` pair as the native one, and the engine
code is shared unchanged.

## Storage: the replica in IndexedDB

The sync engine's `Storage` trait is synchronous, and IndexedDB is not. `MirrorStorage` bridges them:

- It keeps the whole replica image in memory (the snapshot, the confirmed events since, the epoch, the outbox,
  the "couldn't sync" list) and behaves exactly like `MemoryStorage` (a property test checks this).
- Every write also appends a batch of changes to a journal. `IdbFlusher` writes the journal into IndexedDB
  (one transaction per batch, in order); a batch leaves the journal only when its transaction's `complete`
  fired.
- At start the image is read back from IndexedDB (`MirrorStorage::recover`).

**A change is queued once IndexedDB has it.** The view shows an edit at once, but the engine doesn't send an
outbox entry until the batch that added it has committed (`Storage::is_durable`), and sends nothing after it
before it either. So an edit the server has is never one a closed tab could have forgotten. While a write is
uncommitted or has failed, closing the page asks first (`beforeunload`), and hiding it (`pagehide`) writes
what is pending. A failed write (out of quota, say) shows a persistent alert, "Couldn’t save in this browser",
with the browser's error in its tooltip, and is retried every second until it works; the edit is sent then.

The database has one object store per part: `kv` (snapshot and epoch), `events`, `outbox`, `failed` and
`damaged`. The outbox and the "couldn't sync" list are stored **one record per command**, keyed by its
command ID, and a write puts or deletes only the records of the commands it touches.

**Several tabs.** Each tab has its own copy of the replica over the same database. With per-command records, a
tab never overwrites another tab's unsent changes: after every tab is closed, the next start reads all of them
back, ordered by submission time, and sends each. The confirmed part is a function of the server's log, so
whichever tab wrote it last is fine: the replica skips stored events it already has and refetches after a gap.

**One tab sends for all.** If each tab sent its own outbox, a rename made in a stalled tab could reach the
server after a later rename of the same task from another tab, and the older title would win. So the tabs
elect one sender with the Web Locks API: the tab holding the lock `todo-sender` sends; the others only queue
(`SyncClient::set_sending(false)`). After every committed batch that touched the outbox or the failed list, a
tab posts on the `BroadcastChannel` `todo-outbox`; the sender then reads the `outbox` store and takes the
other tabs' entries in (`SyncClient::adopt`), skipping what it has, refused, or saw confirmed. Every tab still
follows the event stream, which confirms its own entries; entries the sender refused reach the other tabs'
"couldn't sync" lists the same way.

- **Taking over.** When the sender's tab closes, the browser hands the lock to the next tab. That tab sends
  nothing, not even its own new edits, until it has read the whole `outbox` store and taken it in. Its own
  queue and the adopted entries are then merged by submission time: only commands *this tab* started sending
  keep their place at the front, and a tab that wasn't the sender has started none. So a rename the old sender
  left behind still lands before a newer one made in the new sender's tab.
- **Missed announcements.** A message on the channel can be lost (a tab that was frozen, say), so the sender
  doesn't rely on it: while it holds the lock it reads the `outbox` store again every 5 seconds, when the page
  becomes visible, and when the browser comes back online. A read that fails is retried every second; if it
  keeps failing, the same persistent "Couldn’t save in this browser" alert shows.
- **Settled commands.** The replica remembers the last 4096 commands it settled, to skip them when another
  tab's record of one is read late. A command that has aged out of that memory and is sent again is harmless:
  the server answers with the events it already recorded, which the replica has confirmed, so the entry just
  leaves the outbox, without a resync.

A browser without Web Locks falls back to every tab sending its own outbox: each tab's commands stay in order,
and command IDs are idempotent, but two tabs' offline edits to one task may land in either order.

**Upgrades.** Opening the database at a newer version waits until every other tab has closed its connection.
While it waits, the page says "Close other To-do tabs to finish updating", and goes on by itself when they do.
Every connection listens for `versionchange`: when another tab needs a newer version, the tab stops syncing
and from then on is **read-only**. `backend::submit` refuses every command without applying it (an edit made
now would show as saved but never reach a database), and quick-add, the checkboxes, the toggles and the
editors are disabled. A persistent alert says "A newer version is open in another tab. Reload to keep
editing", with a **Reload** button, in case the automatic reload doesn't happen. The tab writes what it still
has, releases the sender lock, closes the database and reloads (to get the newer build).
It never closes the database while its journal holds a write: if a write is failing, the tab keeps the
connection open (which keeps the upgrade waiting), keeps retrying, and says "This tab has changes it couldn’t
save yet; other tabs will update once they’re saved." (That alert comes before the read-only one: reloading
then would drop those changes.) Once they are stored, it closes and reloads.

**Recovery never deletes unsent work.** The confirmed state is a cache of the server; the outbox is not.

- An unreadable snapshot or epoch drops the confirmed part, which is fetched from the server again; an
  unreadable event drops the stored events after the snapshot. The outbox is kept.
- An unreadable outbox or failed record is moved to the `damaged` store under its original key, never
  deleted, and the app says so in a toast.
- The database is never deleted automatically. If it can't be opened at all, the app shows a storage error
  with **Reset local data**, which deletes it only after a confirmation that says unsent changes go with it.
- Version 1 of the database kept each list as one array in `kv`; it is split into records by the first write,
  in one transaction that puts every record (keyed by command ID) before it deletes the array, so an
  interrupted conversion leaves one or the other and simply runs again. (No version 1 build was deployed.)

So a change made while the server can't be reached waits in the outbox, survives a reload or a closed tab
(from one tab or several), and is sent when the app runs again (the live browser tests do exactly this). The
page itself still needs the server to load: there is no service worker yet.

## Look and layout

Everything follows `docs/design-language.md` and takes its values from `crates/todo-design/generated/tokens.css`.
The page sets `data-theme` (from `prefers-color-scheme`, following changes) and `data-room-hue` on `<html>`,
`data-list-hue` on chips, glyphs and pills, and `data-agent` on letter marks. The finished My Day's header sets
`data-theme="dark"` on itself, since the dusk scene is a night scene in both themes.

Media queries can't read custom properties, so the breakpoints are written in `app.css`, derived from the
tokens:

| Width | Layout |
|---|---|
| ≥ 1200 px | three panes: sidebar (`--desktop-sidebar`), list, detail (`--desktop-detail`) |
| 760–1199 px | sidebar and list; detail opens as a modal pane with a scrim |
| < 760 px | phone: app bar, room pills, 64 px rows, a bottom composer, a drawer for the sidebar, a full-screen detail |

760 is about the sidebar plus the phone's inner-screen list (`--phone-inner-list`) plus padding; 1200 leaves
the list at least 560 px next to both side panes (`--desktop-three-pane-min`, which the desktop app uses too). On a phone, dated and undated rows share the same due
slot if any displayed row uses it. On every viewport, agent and priority columns are reserved only when a
visible row uses them. List chip labels and trailing tags ellipsise; steps and mail come before the tags.

The filter narrows open and completed rows in the current room using the shared smart-list query language
(e.g. `tag:docs`), including accent-insensitive text. Room totals are unchanged. Sidebar chevrons show or
hide a collection's lists without changing rooms. The browser remembers folded collections in `localStorage`,
with one key per ID so changes to different collections in separate tabs don't overwrite each other. Open tabs
follow storage changes; after sync, the app drops keys for collections that no longer exist. If storage is
unavailable, folding still works for this visit.
**New list** creates a sibling of the current list, a child of the current collection, or a top-level
list from a smart room. **New collection** creates a collection at the end of the sidebar. Both get a swatch.

Right-click a list or collection in the sidebar, press **Shift+F10** on its link, or use its **…** button.
A list or collection header has a **Room menu** too. Use Arrow Up/Down to navigate, Enter to act, or
Escape to close the menu. **Move up** and **Move down** stay within the item's siblings. **Move to…**
chooses the collection for a list, or **No collection** for the top level. Moving into a folded collection
unfolds it. **Colour…** offers the twelve design swatches, with the current swatch ticked. Both open
separate pickers: use the arrows to browse, Enter or Space to choose, or Back to return to the menu.
**Delete** asks before hiding the item and its contents; the scrim blocks other controls and focus starts on
**Cancel**. Undo brings back the deleted item and makes its surviving contents visible again.

When the sidebar's sync line says **n not synced**, open it to see the refused changes and plain-language
reasons. **Dismiss** clears one; **Dismiss all** clears only the entries shown during this opening. A refusal
that arrives while the list is open stays for the next opening. Dismissing doesn't retry a change.

**Activity** (`/app/activity`) shows confirmed changes from you, agents and email ingest, newest first
and grouped by local day. Scroll down or choose **Load older activity** for more. Select an agent in the
sidebar (the Lists drawer on a phone) to open `/app/activity/agent/<agent-id>`. Use **Clear filter** to see
all actors again. **Refresh activity** starts from the newest changes again without moving the feed as
new events arrive. If an older page is loading, the refresh waits for that request. If refreshing removes
the focused row, focus moves to the Activity heading. A dot on the refresh button means the replica has
advanced since the feed was read. Event summaries may use an item's current name, even for older changes.
The pages, the undos asked for, what each row's Undo offers and the wording are shared with the desktop's
Activity room (`todo_client::activity`: `Feed`, `Undos`, `undo_action`).

Each change at the head of its undo chain offers **Undo**. A queued undo disables the action for that
event, including after reload and when another tab sees it in the shared outbox. Conflict and other refusals stay beside the
change in plain words; the server is never asked to force the undo. Activity uses
`SyncClient::undo_activity`: a past event has no local tracked submission to cancel. The shared outbox
sends its sequence and expected event ID, with the replica's log epoch when known, to compensate the
original command's whole batch. The Activity read uses the same session and CSRF transport as sync.

Quick-add uses the [shared syntax](desktop.md#quick-add-syntax) to show a live chip for a recognized date
and to set that date when you submit. The chip uses the same short date as desktop and Android, such as
`Sun 22 Nov`. A trailing date can be `tomorrow`, `zítra`, `+3d`, a current or future ISO date, or a weekday (`Sunday`,
`by fri`). Whole phrases work too: `in 2 days`, `za 3 týdny`, `příští týden`, `next Friday`, `o víkendu`,
`end of month`, or a day and month such as `1. prosince` or `1. 12.`. Past ISO dates stay literal. A separate `!!` or `!!!` sets priority. `See you \tomorrow` keeps
`See you tomorrow` as the title. The room supplies defaults: My Day adds today's My Day stamp,
Important defaults to high priority and Planned defaults to due today. New tasks go first in their
list's manual order. If there is no destination list, quick-add creates **Tasks** first, inside the
current collection when applicable. **Alt+Up/Down** moves the selected task one place among its live
siblings in its own list, even when the list is filtered. Aggregate rooms don't support manual reordering.

Completing a task runs the F2 choreography: the fill pops (`--duration-check-fill`, `--ease-pop`), the tick
draws, the row holds in place struck through (`--duration-complete-hold`), then collapses
(`--duration-row-collapse`), and only then does the sun move along its arc (`--duration-sun-travel`, eased
with `motion::SUN`) and the count change. The toast offers Undo for `--duration-undo-window`, with a thin line
running out. **Ctrl+Z** outside text fields can still undo the last tracked change after the toast closes.
Completion, reorders, list moves, colour changes, repeat and reminder changes, tag edits and membership
changes, web link edits, and removal of a list, collection, step or attachment use `SyncClient::compensate_tracked`. It queues undo of
the whole original event batch, waiting for its authoritative event if necessary. A tab never
cancels the original locally, because another tab may already be sending it. For recurring tasks, Undo reopens
the original and removes the next occurrence it created. A late undo result cannot replace a newer toast;
refusals are also listed in Not synced, in plain language. Reduced motion turns movement into instant changes.

The detail pane stays mounted while the same task is shown, and a live update changes its fields in place.
The title and notes editors keep a draft: while it is dirty (typed in and not yet saved) a live update doesn't
touch it, and it saves when the field loses focus (notes) or on Enter (title); fields that aren't being
edited follow the update. Click **Due** or press **D** to edit a date. The editor accepts the same relative
syntax as the desktop and also accepts past dates. Enter saves; Escape or clicking away cancels. Saving a
blank field clears both the date and any repeat rule. Choose any of the four levels in the **Priority**
field (also reachable with **Shift+P**), or press **P** to cycle none → low → normal → high → none. The
Important star still toggles high priority. Click a step's title to rename it, or use the step's up,
down and delete buttons (visible on hover or keyboard focus on desktop, always visible on phones).
The Undo toast restores a deleted step.

**Reminder** offers **In 1 hour**, **Tomorrow at 09:00**, **In a week at 09:00**, and **Custom…** for a
specific date and time. Use the arrows to browse presets, Enter or Space to choose, and Escape to close
and return focus to the row. In the custom editor, Tab moves through the fields and buttons. Times use the
browser's time zone; a skipped time at a clock change is refused, and a newly chosen repeated time uses
the earlier occurrence. **Remove reminder** clears the shown reminder only. If a task has several, the
shared intent shows the first from local today onward, or the latest if all are older. If the reminder
changes elsewhere while the picker is open, close and reopen it before editing. Adding, changing and
removing a reminder all offer Undo. The web app does not deliver reminder alerts.

**Repeat** (or **R**) offers Daily, Weekdays, Weekly, Monthly and Yearly, plus **Custom…** for an
interval, unit and days of the week. Use the arrows to browse the presets and Enter or Space to choose.
**Don’t repeat** keeps the due date. Setting repeat on an undated task picks the first matching date on
or after today, in the same undoable change. The summary uses the shared repeat description.

**Tags** (or **T**) opens the tag picker. Select a tag to put it on or take it off the task without
closing the picker. Type to filter, or create a new tag with that name and attach it. Undo detaches it;
the new tag remains available for other tasks. Open a tag’s chevron (or press **Right** on the tag) to
rename, recolour or delete it. A delete confirmation rechecks the tag and its live task count on confirm
and asks again if they changed. If the task the picker came from is deleted, the confirmation closes.
Deleting hides that tag on every task; Undo restores it and its memberships unless another tag has
taken its name.

In **Links**, type a web address (or press **L** to focus the field). For a bare host like `example.com`
the shared builder adds `https://`. Remove a web link with its remove button; Undo brings it back.

On a narrower viewport, the detail takes focus, limits Tab to the pane and makes the rest of the app inert. Click the medium-width
scrim, use Close, or press Esc to dismiss it and return focus to the selected row (or the room heading).
The scrim and pane use the overlay motion tokens; reduced motion uses a short fade without a slide.
While the detail is modal, Undo and other toast feedback appear in its own bottom strip, above the scrim.

In **Attachments**, choose **Add file** or drop files anywhere on the task's detail pane. Dropped text
and folders are ignored. A file selected on a new task waits in the open tab for the task to sync;
it isn't stored in the offline outbox. Files are sent one at a time (**Uploading…**). After the server
accepts them, **Waiting to sync…** stays visible until they reach this tab's replica. If that takes
more than 30 seconds, the app says the files uploaded but haven't appeared here yet. Upload failures
use plain wording without quoting the server. Reopening the detail clears an old alert. Click a file
to download it. **Remove** hides the attachment at once, and **Undo** restores it.

Finished My Day shows the done count and **all clear** beside the dusk scene. It keeps quick-add and a
**Completed n** disclosure, and links to Planned via **Plan <weekday>** even when tomorrow has no tasks.
Click a hand-over task's title, or use **J/K**, to open its detail. Its checkbox or **Space** on a selected
row completes it in place and offers the usual Undo. The My Day count stays put:
these tasks belong to tomorrow or later. The list scrolls separately from its toast strip and phone composer.

**The app icon** is Daybreak, from `assets/brand` (see its README); the build copies it into `dist/brand/`.
The favicon is the 16 px drawing (`small-16.svg`, since a tab shows it at 16 CSS px and the master must never be
scaled down that far) with the generated 16 and 32 px PNGs, plus the 180 px `apple-touch-icon.png`. The
sidebar's mark is the same 16 px drawing, as shipped. There is no web manifest: installing the app (PWA) is out
of scope for now, and the generated 192 and 512 px and maskable icons are ready for it.

Keyboard: **J/K** or the arrows move the selection, **Space** toggles a selected task from the list, **E**
edits the detail title, **D** edits its due date, **P** cycles priority (**Shift+P** focuses the picker),
**R** opens repeat, **T** opens tags, **L** focuses the link field, **Alt+Up/Down** reorders in a list,
and **N** focuses quick-add. **/** or **Ctrl+K** opens the room filter. **Esc** closes
the active editor, overlay or filter. App shortcuts don't run while typing and held keys don't repeat actions.
The status bar shows shortcuts only when their actions are available.

## Tests

- **Unit tests** (`cargo test` in `apps/web`): the view-model against the fixtures (counts, chips, due dates,
  paths, the detail, tomorrow's plan) and the landscape markup (no colours or inline styles in it; the sun
  where the count says).
- **Browser tests** (`e2e/tests`, Playwright, Chromium): the fixtures build served by `e2e/serve.mjs` the way
  the server serves it, with the server's exact CSP (read from `spa.rs`). They add and complete tasks, undo,
  edit titles, steps and toggles, use the keyboard and the address bar, follow the system theme, and fail on
  any console error or CSP violation. Reminder tests cover keyboard navigation and focus return, custom time
  validation, cancelling a draft, and Undo. Set `FIXTURE_PORT` to use a different port from the default 4173.
- **The screenshot gallery** (`e2e/tests/gallery.spec.mjs`): every screen in both themes — desktop My Day with
  the detail, due-date editor, step controls and delete Undo, attachments (empty, uploading, failed, listed
  and removed with Undo), a collapsed collection, sidebar and room menus,
  rename and delete dialogs, a delete Undo toast, collection creation, list moves, list and collection swatches,
  repeat presets and custom repeat, reminder presets and the phone custom reminder editor, the tag picker
  and its creation, management, rename, colour and delete views,
  the link field and removal, the phone Not synced list, an active query filter, the finished
  day with a hand-over selection and with Completed closed and expanded, a finished day with no tasks tomorrow,
  an empty list, Activity (unfiltered, agent-filtered, empty and refused undo), the desktop and phone quick-add date chip, the storage-error and read-only alerts
  (desktop and phone), the medium modal, and the phone's list, detail and drawer. The PNGs land in
  `apps/web/e2e/gallery/`; CI uploads them as `web-gallery`. Compare them with the
  approved mockups (kept outside this repository) after every UI change.
- **Live tests** (`e2e/live`, `live.config.mjs`): a real `todo-server` (from `target/debug`, or
  `$TODO_SERVER_BIN`) with a fresh data directory on loopback, and the real build from `apps/web/dist`. A
  virtual WebAuthn authenticator registers the first passkey with the setup code. The test then uses the app:
  the session + CSRF API, a change from a second tab arriving over the event stream, an edit made while the
  API is unreachable that survives a reload and is sent afterwards, completing, and a request without the CSRF
  header being refused. Activity reads confirmed edits from another tab and undoes one through the session.
  The suite also uploads, downloads and removes a file with the web session, sets a repeat rule and sees
  the next occurrence on completion, adds and removes tags and links across tabs, checks tag delete confirmation
  against a live change in membership, and moves a list between collections. It verifies synced reminder
  edits and Undo, including clock-change validation and a stale editor that cannot overwrite a change
  from another tab. Then:
  exact Undo of a confirmed recurring completion (including removal of its
  next occurrence), including Undo before the sending tab has a receipt and late feedback preserving a
  newer Undo toast; a live sidebar change leaving the highlighted menu action on the same list;
  Dismiss all preserving a refusal that arrived after opening Not synced;
  two tabs queueing edits offline, both closed, and every edit sent exactly once
  on the next start; a tab whose session was replaced by a sign-in elsewhere still sending its queued edit,
  once; a note being typed surviving a rename that arrives over the event stream, then saving; a rename in a
  stalled sender tab and a later one from a second tab landing in that order, and the lock passing on when
  the sender closes; a sender closing with an older rename queued, and the tab taking over with a newer one
  sending the older one first; an edit whose `BroadcastChannel` message is dropped (injected) being found by
  the sender's periodic read, and a failing read (injected) showing the alert until it works; an IndexedDB
  write failure (injected) that shows the alert, sends nothing, asks before closing, and sends the edit once
  after storage recovers; an upgrade blocked by an old connection showing its notice and going ahead; a tab
  whose write is failing keeping a newer version's upgrade waiting, with its alert, until the write is stored
  (the newer database then holds the edit); a tab whose reload after an upgrade does nothing (the reload
  answers 204) staying read-only: the alert shows, the controls are disabled, and neither the keyboard nor
  a quick-add forced past its disabled field changes anything; and a connection closing (and the tab
  reloading) for a newer version. Tests wait for
  IndexedDB commits by reading the `outbox` store, never for a fixed time. The port is `LIVE_PORT` (default
  27412).
- **Server tests** (`crates/todo-server/tests/web_app.rs`): the session path of the API (reads and writes as
  the `web` device, missing or wrong CSRF tokens, cross-origin and originless writes, expired and ended
  sessions, session upload success, CSRF and origin refusals, upload size limit, bearer precedence, MCP and
  imported content fill staying bearer-only), the `/app` routes and headers, gzip,
  `no-store, no-transform` on everything that carries a secret, `/auth/session` (same origin only, and never
  extending the session), refused requests not extending the session, and the event stream closing on
  sign-out. `tests/attachments.rs` checks `private, no-store, no-transform` on downloads.
- **Client tests** (`crates/todo-client/tests/mirror.rs`): the journal rebuilds the storage; two tabs' outbox
  entries both survive in any order; an unreadable snapshot, event, outbox or failed record is repaired or set
  aside without losing unsent work; the version 1 arrays are split into records; a batch stays queued until
  committed, and an entry isn't sent before it is durable. `tests/replica.rs`: adopted entries slot in by
  submission time behind what was sent, a new sender's own queue merges with what another tab tried by
  submission time, settled or refused ones aren't adopted, and a replayed command that is already confirmed
  just leaves the outbox. `tests/sync.rs`: one
  client sending for another, in submission order, against a real server. `src/session.rs`: two concurrent
  `403 csrf` refusals both retry with the new token, and a refusal that persists is retried once, then later.

## Not yet

- **Offline start.** There is no service worker, so the app can't load without the server (Web Push and PWA
  install are out of scope for this version, and would come with it).
- **Damaged records** are set aside and counted, but the app can't show or export them yet.
- **An idle tab is signed out.** The event stream doesn't extend the session, so a tab left alone for longer
  than `session_idle_timeout_mins` is sent to sign-in when the session lapses. Pending changes are kept.
- **Moving tasks between lists** isn't available.
- **The wasm size.** `wasm-opt` and Leptos's `erase_components` haven't been tried in the release build yet.
