# Web app

`apps/web` is the browser client. It is Rust compiled to `wasm32-unknown-unknown`, with a
[Leptos](https://leptos.dev) UI rendered on the client (CSR). It uses the same `todo-core` and `todo-client`
as the other clients, so the state, the commands, the smart-list queries and the sync engine are exactly the
desktop's and Android's. `todo-server` serves it at `/app`, and it signs in with the owner's passkey session:
there is no token in the browser.

The web app covers sign-in, smart lists, collections, lists, task detail, adding, completing (with the F2
check animation), deleting, moving and merging tasks, and live updates. Edit titles, steps, notes, due dates, priority, My Day, repeat rules,
reminders, tags and web links in detail. Upload, download and remove attachments. Create, rename, recolour, reorder
and delete lists and collections, and move lists between collections. Workflow lists also support task status,
a four-column Board, start dates, and a Timeline that can also show Planned and eligible collections. Activity shows confirmed changes
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
`web app (wasm build and Rust tests)` lints, tests, and builds it. The `web browser tests` jobs reuse those
builds to run two fixture shards and the serial live-server suite in parallel.

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
| `src/app.rs` | The Leptos components, including the frame that applies the preferred detail width. |
| `src/app/detail_splitter.rs` | Shared docked detail and Assistant splitters, width preferences, pointer capture, and keyboard resizing. |
| `src/app/assistant.rs`, `assistant/`, `assistant_provider.rs` | Assistant chat, trusted proposal review and settings, and direct browser inference. |
| `src/backend.rs` | The live backend (the sync engine) or the fixtures backend, including exact undo and dismissal of refused changes. |
| `src/app/activity.rs` | The activity room, day groups, older pages and per-event undo feedback. |
| `src/app/room_actions.rs` | List and collection menus, move and colour pickers, rename and delete dialogs, and the Not synced list. |
| `src/app/task_actions.rs` | Task context menu and detail overflow, searchable move and merge pickers, and focus hand-off when a row leaves. |
| `src/app/workflow.rs`, `board.rs`, `timeline.rs` | List settings, view selection, status and start-date controls, and the shared Board and Timeline projections with pointer and keyboard input. |
| `src/app/reminder.rs`, `repeat.rs`, `tags.rs`, `links.rs` | Task detail editors using the shared client intents. `pickers.rs` shares the anchored controls and swatches. |
| `src/app/reminder_alerts.rs`, `static/reminder-browser.js`, `static/reminder-worker.js` | Reminder scheduling, opt-in, Web Push notifications and clicks, and durable claims shared with the in-tab fallback. |
| `src/app/attachments.rs` | File picker, drops onto the detail pane, upload status, and download and removal controls. |
| `src/app/email_drop.rs` | The task list's `.eml` drop target and browser file reads. Parsing and commands are shared with desktop. |
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

### Back and Forward

The browser's Back and Forward controls (including mouse side buttons) restore the last room, its
List/Board/Timeline view, and explicitly opened detail. Changing room or view, opening detail, and
closing it push a location; a new visit after Back discards the forward branch. **J/K** and cursor
arrows can preview detail without changing the recorded URL. Filter text, scrolling, selection,
menus and dialogs do not add entries. Automatic fallback after deletion corrects the current URL
with `replaceState`, not a new visit. Back and Forward skip deleted rooms or tasks (including those
hidden by a deleted parent) and views that are no longer enabled.

A URL such as `/app/list/<id>/view/board/task/<id>` opens that view and task directly. Bare room links
choose the synced default view. The tab tracks up to 100 recent locations in memory; earlier browser
entries and entries from before a reload are read as deep links. Browser history is still free to leave
`/app`. All room and detail paths keep the existing passkey session gate.

### Headers

The page gets its own CSP:

```
default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data: blob:;
connect-src 'self' http://127.0.0.1:* http://localhost:*; font-src 'self';
worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self';
object-src 'none'
```

`'wasm-unsafe-eval'` allows compiling the wasm module and nothing else; there is no `'unsafe-inline'` or
`'unsafe-eval'`. Styles come from the two stylesheets. Dynamic values are chiefly classes and `data-*`
attributes, and the landscape's stroke widths and opacities are SVG presentation attributes. Room menus set
only their measured screen coordinates, and the docked grid sets its detail width, via CSSOM custom
properties. Neither needs inline style markup. Browser tests fail on any CSP violation. The rest of the
server's headers (`Referrer-Policy: same-origin`, `X-Content-Type-Options`, ...) apply as on every page. The `connect-src`
exceptions are for direct loopback inference; hosted models and Web Push use the same origin. External
scripts, styles and forms remain disallowed. The reminder worker and its helper script have their own
policy that allows only same-origin scripts, network requests and images.

## AI assistant

Open **Ask AI** beside Filter, or press **Ctrl+J** / **Cmd+J**. On wide screens the assistant docks with
its own resizable divider and remembers the width in this browser. It opens as a drawer on medium windows
and as a bottom sheet on phones. Enter sends, Shift+Enter inserts a newline, and **Stop** aborts the reply.
Closing the panel preserves the chat until reload. Settings are also available under **Settings → AI assistant…**.

**ChatGPT** and **Your private models (via your server)** use the [server relay](hosting-linux.md#ai-assistant-relay).
The browser uses the same-origin owner session and never receives the provider credentials. For a new
ChatGPT account, if device sign-in is enabled, choose **Sign in with ChatGPT**, open the verification page,
and enter the displayed one-time code. Only approve a sign-in you started. After approval, choose the
account and a model. Other paired devices can see the account without signing in again. **Sign out**
removes the account from all devices. **Manage usage** opens ChatGPT's usage settings. Eligible requests
can use the account's plan or available credits; there's no automatic fallback to another provider.

**This device** still supports a direct OpenAI-compatible endpoint on `http://localhost` or
`http://127.0.0.1`. The model must allow the app's exact origin using CORS, including `POST`,
`Content-Type`, and, for an optional key, `Authorization`. The local key can be remembered only in the
current tab. To-do credentials, cookies and referrers are never sent to a direct endpoint. Tailnet
endpoints previously configured directly must be moved to the server relay.

Each list defaults to **Hidden**. **Private only** permits the private relay and loopback models;
**All models** also permits ChatGPT. Lists inherit the collection rule unless overridden. Relay list
rules and ChatGPT approval are shared across paired devices, while loopback access stays in the browser.
The first send to ChatGPT asks to approve plan use and the list scope. It asks again when a newly
eligible list would be shared, including when a list is lowered and later made Cloud-eligible again.
Lowering a list's access does not itself prompt again. The assistant projects the confirmed replica
before tool reads. Changes to the provider or access reset the conversation. Pane width and
model choice are browser-local; conversation text is not persisted or synced.

Model output cannot submit commands. **Approve** submits the exact reviewed effects through the same durable
sync, digest and confirmed-revision checks as desktop; **Reject** submits nothing. Confirmed changes appear
under **Assistant** in Activity and offer exact Undo. If a proposal becomes stale, request it again.

## Signing in and the API

The app uses the owner's existing passkey session. Nothing new is stored in the browser: no token, no
password, no key. Under **Settings → Devices…**, the owner can [invite a new browser or
native app, or approve a pending pairing](device-pairing.md). A stale session asks for passkey
verification first. On a new browser, the invite link opens a passkey-registration form and signs the
browser in, without requiring an existing passkey there.

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

`GET /auth/session` (cookie only, `{"csrf": "...", "owner_scope": "..."}`, a 401 without a session)
answers this origin only. The owner scope is a non-secret browser storage namespace, also in the app page;
a refresh for a different owner is treated as signed out. The endpoint it
refuses a request whose `Sec-Fetch-Site` is anything but `same-origin` (so `same-site`, `cross-site` and a
typed-in `none` too) or whose `Origin`, if present, isn't the server's. And it never counts as activity: it
looks the session up without touching it, so a sibling origin or a forgotten tab can't keep an idle session
alive through it.

When the session is gone (the stream or a request gets a 401), the page's revocation latch triggers one
navigation to `/login?next=<the current path>`, after any pending local writes finish. Changes not yet
sent stay in IndexedDB and go out after signing in. A fresh page starts with a new latch.

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
Every connection listens for `versionchange` before reading the replica. If another tab needs a newer
version during that read, the opening tab closes its connection and reloads rather than starting sync.
If the upgrade is requested after startup, the tab stops syncing and becomes **read-only**.
`backend::submit` refuses every command without applying it (an edit made now would show as saved but
never reach a database), and quick-add, the checkboxes, the toggles and the
editors are disabled. A persistent alert says "A newer version is open in another tab. Reload to keep
editing", with a **Reload** button, in case the automatic reload doesn't happen. The tab writes what it still
has, releases the sender lock, closes the database and reloads (to get the newer build).
It never closes the database while its journal holds a write: if a write is failing, the tab keeps the
connection open (which keeps the upgrade waiting), keeps retrying, and says "This tab has changes it couldn’t
save yet; other tabs will update once they’re saved." (That alert comes before the read-only one: reloading
then would drop those changes.) Once they are stored, it closes and reloads. Version 3 uses the same
stores as version 2; the bump retires older senders before they can read and drop the `from_list`
precondition on another tab's queued task move. Version 4 likewise leaves the stores unchanged but
retires older senders before a tab can queue a bulk task command. Version 5 does the same for the
bulk reorder action. Version 6 adds workflow commands and replays the confirmed cache so older snapshots
cannot hide the new workflow fields. The outbox is kept.

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
page itself still needs the server to load: the reminder service worker does not cache the app.

## Look and layout

Everything follows `docs/design-language.md` and takes its values from `crates/todo-design/generated/tokens.css`.
The page sets `data-theme` (from `prefers-color-scheme`, following changes) and `data-room-hue` on `<html>`,
`data-list-hue` on chips, glyphs and pills, and `data-agent` on letter marks. The finished My Day's header sets
`data-theme="dark"` on itself, since the dusk scene is a night scene in both themes.

Media queries can't read custom properties, so the breakpoints are written in `app.css`, derived from the
tokens:

| Width | Layout |
|---|---|
| ≥ 1200 px | sidebar (`--desktop-sidebar`), list and a resizable docked detail pane (when a task is open) |
| 760–1199 px | sidebar and list; detail opens as a modal pane with a scrim |
| < 760 px | phone: app bar, room pills, 64 px rows, a bottom composer, a drawer for the sidebar, a full-screen detail |

760 is about the sidebar plus the phone's inner-screen list (`--phone-inner-list`) plus padding; 1200 is
the shared `--desktop-three-pane-min` breakpoint. On a phone, dated and undated rows share the same due
slot if any displayed row uses it. On every viewport, agent and priority columns are reserved only when a
visible row uses them. List chip labels and trailing tags ellipsise; steps and mail come before the tags.

The filter narrows open and completed rows in the current room using the shared smart-list query language
(e.g. `tag:docs`), including accent-insensitive text. Room totals are unchanged. Sidebar chevrons show or
hide a collection's lists without changing rooms. The browser remembers folded collections in `localStorage`,
with one key per ID so changes to different collections in separate tabs don't overwrite each other. Open tabs
follow storage changes; after sync, the app drops keys for collections that no longer exist. If storage is
unavailable, folding still works for this visit.

The sidebar uses the desktop navigation metrics on wide screens while keeping touch-sized controls in the
phone drawer. Smart-list and Activity icons keep their own glyph hue regardless of selection. Collection
headers and lists share the desktop's name-to-glyph rules; a name that doesn't match defaults to layers for
collections and to a document for lists. Every collection and list shows its open count, even zero.
**Agent activity** shows a pip for tasks most recently changed by an agent today; it still opens the
seven-day smart list. The settings gear in the pinned sidebar or phone drawer shows the current server.
**Open admin pages** opens the same origin's `/admin` in a new tab, leaving the app open. **Sync now**
retries without waiting for the next automatic attempt. It shows **Syncing…** and can't be activated
while the engine is catching up or sending changes. The menu also offers **Devices…** and **Sign out**.

**+ New → List** creates a sibling of the current list, a child of the current collection, or a top-level
list from a smart room. **+ New → Collection** creates a collection at the end of the sidebar. Both get a swatch.

Right-click a list or collection in the sidebar, press **Shift+F10** on its link, or use its **…** button.
A list or collection header has a **Room menu** too. Use Arrow Up/Down to navigate, Enter to act, or
Escape to close the menu. **Move up** and **Move down** stay within the item's siblings. **Move to…**
chooses the collection for a list, or **No collection** for the top level. Moving into a folded collection
unfolds it. **Colour…** offers the twelve design swatches, with the current swatch ticked. Both open
separate pickers: use the arrows to browse, Enter or Space to choose, or Back to return to the menu.
**Delete** asks before hiding the item and its contents; the scrim blocks other controls and focus starts on
**Cancel**. Undo brings back the deleted item and makes its surviving contents visible again.

**List settings…** is a subpage in both menus for ordinary lists. **Workflow & Board** enables status and
Board; **Timeline & Start dates** enables start dates and date spans. Both are off on a new list, and
turning either off keeps the underlying task data. The switches and **Default view** sync. Saving a
new default changes the next visit, not the view behind the menu. Use **Esc** or the back button to return
to the menu. Enabled views sit in the room eyebrow; press **V** to cycle or **1** / **2** / **3** to jump to
List, Board or Timeline. Below 556 px of room width the tabs keep only their icons and accessible labels.

**Board** groups Backlog, To do, In progress and Done using `todo_client::board`. Done shows at most 20
completions from the last seven local days; **View all … in Completed** opens the same list's full
Completed section. Beside docked detail, Done defaults to a 44 px rail that still accepts drops. Its explicit
expanded or collapsed state is local to the browser. Wells scroll independently; at narrow widths the Board
scrolls horizontally inside the room, with a modal detail. On a phone, swipe across wells and use the card
grip to drag. **+ Add to …** opens the shared composer staged to that column, with no Add button in Done.

Drag a card to reorder it, move it to another stage, or complete/reopen it in Done. The lifted card tracks
the pointer; a bar marks legal placement. Escape or releasing outside a legal target cancels. Use **H/L**
or **Left/Right** to focus columns, **J/K** or **Up/Down** for cards, **Ctrl+Alt+Left/Right** to move between
stages and **Alt+Up/Down** to reorder. **Enter** opens detail, **E** edits its title, **Space** completes or
reopens, and **N** opens the active column's composer. Modified clicks use the same selection as List;
**Status** in the bulk strip changes the selected cards together. Board moves and status edits offer tracked
Undo, and completing a card uses the same check, hold, collapse and sun timing as List.

**Timeline** is available in lists that opt in, collections with an opted-in list, and Planned. Planned
shows every dated open task, including start-only tasks; cross-list bars use each task's list colour.
The sticky date ruler and collapsible task rail frame a scrollable grid. Today marks the current local
calendar day. **Week**, **2W** (default), **Month** and **Quarter** keep the centre date when changing
scale. Even in Quarter, edits snap to days, not weeks. Zoom and the rail and tray disclosures are local
browser preferences. Narrow rooms use compact controls; on phones the toolbar wraps, the rail narrows,
and the date grid scrolls without moving the page. Task detail uses the usual modal layout.

Drag a bar to move its dates or an edge to resize; a due-only milestone can grow a start and a
start-only bar can grow a due date. The live readout previews the new dates. Drag an **Unscheduled**
card onto a date, or choose **Schedule** to open the calendar without dragging. Escape or dropping
outside the calendar cancels. A change saves one tracked edit with **Undo**. Use **J/K** or **Up/Down**
to focus a lane, **Ctrl+Alt+Left/Right** to shift by a day (**Shift+Alt+Left/Right** for a week), **[** / **]** to change
its duration, **D** / **Shift+D** for the due/start calendar, and **0** for Today. **Enter** opens detail;
**Space** toggles completion. **Show completed on Timeline** reveals dated completed tasks.

When the sidebar's sync line says **n not synced**, open it to see the refused changes and plain-language
reasons. **Dismiss** clears one; **Dismiss all** clears only the entries shown during this opening. A refusal
that arrives while the list is open stays for the next opening. Dismissing doesn't retry a change.

**Activity** (`/app/activity`) shows confirmed changes from you, agents and email ingest, newest first
and grouped by local day. Scroll down or choose **Load older activity** for more. Its clock row follows the
**Agent activity** smart list. Up to four agents that created or last edited live tasks today appear indented
below it. Agents without activity today do not use a slot. Each stamp is dated using the browser's UTC
offset at that instant, and the row says what the agent added or edited today. Choose an agent to open
`/app/activity/agent/<agent-id>`. The same links are in the Lists drawer on a phone. Use **Clear filter** to
see all actors again. **Refresh activity** starts from the newest changes again without moving the feed as
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

**Trash** (`/app/trash`) is below Activity and its agents in the sidebar and phone drawer. It shows
explicitly deleted tasks, lists and collections from the synced replica, newest first. **Restore**
returns an item to its original list or collection and offers tracked **Undo**. If that parent is deleted,
restore the parent first. Restoring a list or collection reveals the items hidden with it, but not items
deleted separately. Undo a merge from Activity to recover a merge source. Trash has no automatic expiry
or permanent-delete control. Files are subject to the server's separate attachment blob-retention policy.

Quick-add uses the [shared syntax](desktop.md#quick-add-syntax) to show removable date and priority chips
and to set those fields when you submit. The date chip uses the same short date as desktop and Android, such as
`Sun 22 Nov`. A trailing date can be `tomorrow`, `zítra`, `+3d`, a current or future ISO date, or a weekday (`Sunday`,
`by fri`). Whole phrases work too: `in 2 days`, `za 3 týdny`, `příští týden`, `next Friday`, `o víkendu`,
`end of month`, or a day and month such as `1. prosince` or `1. 12.`. Past ISO dates stay literal. A separate `!!` or `!!!` sets priority. `See you \tomorrow` keeps
`See you tomorrow` as the title. The room supplies defaults: My Day adds today's My Day stamp,
Important defaults to high priority and Planned defaults to due today. New tasks go first in their
list's manual order. If there is no destination list, quick-add creates **Tasks** first, inside the
current collection when applicable. The **Calendar** opens a Monday-first month grid with **Today**,
**Tomorrow**, **Next week** (next Monday) and **No date**. From the grid, arrows move by days or weeks,
Page Up/Down by months, Enter chooses, and Esc closes the picker and returns to its button. The **Flag**
offers the four priority levels. Picking a value removes the matching typed suffix, leaving the other one
alone. Clearing a chip (or choosing **No date** or **None**) overrides the corresponding room default.

Workflow lists show a geometric status mark first in List row metadata, without replacing the completion
checkbox. Cross-list rooms show only Backlog and In progress from workflow lists, after the list chip.
Click a mark or press **S** on a task to pick a stage; **Shift+S** cycles it. The quick-add Status button
and removable chip share the draft with the explicit `status:` / `stav:` suffix. For a destination with
Timeline enabled, use the **Starts** / **Due** tabs in the composer calendar or type a range such as
`tomorrow..+4d`. A start and due appear as a compact span in rows and cards; overdue badges take precedence.

Detail shows **Status** and **Starts** only when their respective list features are enabled. **Shift+D**
opens the Starts calendar. A new start after the current due date shifts Due to the same day as one undoable
edit. In task detail, click **Due** or press **D** to open the same calendar on the task's due date (or today).
Choosing a day or preset saves it right away. For a repeating task, a note below the grid warns that
**No date** also stops the repeat. **Esc** closes without saving and returns focus to Due.

Drop `.eml` files onto the task list to [create tasks from email](desktop.md#drop-an-email-onto-the-task-list).
The area below quick-add highlights and names the destination while dragging, even if the browser
withholds filenames and the file type is unknown. The room header and quick-add are not drop targets.
Only `.eml` files are imported when dropped; Activity, Trash and the read-only app don't accept them.
The same limits, destination, plain-text excerpt, optional Email source and Undo apply as on desktop.
Dropping the email on task detail instead attaches the file to that task.

Right-click a task or press **Shift+F10** on its row, or open **Task actions** (⋯) in the detail pane.
**Delete task** (also **Delete** on the row) acts at once, as on desktop and Android. **Move to…**
(also **M**) puts the task first in another list's manual order. Type to filter by list or collection name,
ignoring case and accents. The current list is marked and disabled. If another device moves the task,
this picker closes instead of overwriting the move. Use Up/Down to go from the filter to an available list,
Enter or Space to choose, and Escape or Cancel to return to the opener. If the task is
still in the current room after moving, it stays selected. If a row leaves, it collapses over
`--duration-row-collapse` (instantly with reduced motion). Focus goes to the next surviving row, the
previous row, or the room heading when neither remains. A modal detail closes so the next row is reachable.

**Ctrl/Cmd+click** toggles rows, **Shift+click** selects a range, and **Shift+Up/Down** extends or
shrinks it from the keyboard cursor. **Ctrl/Cmd+A** on the list selects all visible task rows, including
filtered results and expanded Completed rows. **Esc** or **Clear selection** clears it. Hidden rows
are dropped from the selection, and changing rooms clears it. From a narrow window, start selection
from the list, not inside a detail overlay.

The selection strip above Undo shows the count; when at least two tasks are selected it offers
**Complete** or **Reopen**, importance, My Day, **Move to…**, **Set due…**, and **Delete**. Mixed
selections offer the positive action and skip tasks already in that state. Move to… uses the usual
searchable list picker and keeps the moved tasks in their room order at the front of the destination.
The due editor uses the usual date syntax and **Clear due** clears both the date and the repeat.
The wide detail pane shows a count instead of editing a task. Each successful edit clears selection
and shows one Undo for the whole batch (also **Ctrl+Z** outside text fields).

Grab a row by its six-dot grip to drag it. Grabbing any selected row picks up the whole selection,
in visible order. In an ordinary list, the insertion line shows where the open tasks will land:
before the next remaining visible task or after the last one. This works with a filter; hidden tasks
keep their order relative to each other. Smart lists and collections have no insertion line. Drop on
another list in the sidebar to put tasks at its front, or on **My Day** to plan them for today without
changing their lists. Only valid targets highlight. On a phone, drag to the left edge to open Lists.
The task list and sidebar scroll near their top and bottom edges. Press **Esc** or release outside
an available target to cancel. These pointer gestures don't affect file drops.

**Alt+Up/Down** moves the cursor task or the selected block past the adjacent open visible row.
**M** still opens the task's move picker. A drop or nudge offers one **Undo**, even for several tasks.
The browser gallery captures `drag-reorder-{light,dark}.png` and `drag-destination-{light,dark}.png`.

**Merge into…** searches other open tasks across lists by title, ignoring case and accents. Choose a task
and confirm after reviewing both titles and their lists. The destination keeps its own title, list,
priority and schedule, and takes the source's notes, live steps, links, attachments and reminders.
Tags are combined. The confirmation rechecks both tasks before writing, and **Undo** reverses the
whole merge. The gallery captures the picker and confirmation at desktop and phone widths.

Completing a task runs the F2 choreography: the fill pops (`--duration-check-fill`, `--ease-pop`), the tick
draws, the row holds in place struck through (`--duration-complete-hold`), then collapses
(`--duration-row-collapse`), and only then does the sun move along its arc (`--duration-sun-travel`, eased
with `motion::SUN`) and the count change. The toast offers Undo for `--duration-undo-window`, with a thin line
running out. **Ctrl+Z** outside text fields can still undo the last tracked change after the toast closes.
Completion, reorders, task and list moves, colour changes, repeat and reminder changes, tag edits and membership
changes, web link edits, and removal of a task, list, collection, step or attachment use
`SyncClient::compensate_tracked`. It queues undo of the whole original event batch, waiting for its authoritative
event if necessary. A tab never
cancels the original locally, because another tab may already be sending it. For recurring tasks, Undo reopens
the original and removes the next occurrence it created. A late undo result cannot replace a newer toast;
refusals are also listed in Not synced, in plain language. Reduced motion turns movement into instant changes.

On a wide viewport, drag the divider to give the docked detail more space, or Tab to it and use **Left**
or **Right** (8 px, 32 px with Shift). **End** makes it as wide as possible, while **Home**, **Enter** or a
double-click returns it to the default. **Esc** returns focus to the task list. The detail starts at 392 px,
can shrink to 320 px, and can grow until the list reaches 400 px. As the list narrows below 556 px, its
counts move below the room title and row metadata can sit under the trailing columns. The browser
remembers the preferred width in `localStorage`. A smaller viewport clamps the visible width without
replacing the preference, so the pane can recover its width later. The docked title, notes and steps
stay left aligned at no more than the shared 72 ch reading measure.

The detail pane stays mounted while the same task is shown, and a live update changes its fields in place.
The title and notes editors keep a draft: while it is dirty (typed in and not yet saved) a live update doesn't
touch it, and it saves when the field loses focus (notes) or on Enter (title); fields that aren't being
edited follow the update. Notes render as Markdown until clicked or focused. Move focus away to save
and return to the preview. Links and checklist boxes act without opening the editor. Only absolute
HTTP, HTTPS, and mailto links are active; HTML and images appear as text. Checklist changes offer Undo.

Click **Due** or press **D** to edit a date. The editor accepts the same relative
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
removing a reminder all offer Undo.

**Reminder alerts** can reach an opted-in browser when no app tab is open. If the server has
[Web Push configured](hosting-linux.md#web-push-reminders), choose **Settings → Turn on background reminders**.
The browser asks permission only when you choose this action. Settings shows On, Off, or Blocked by the browser;
if blocked, allow notifications in the browser's site settings. Choose **Send test notification** to check delivery.
On iOS/iPadOS, add the app to the Home Screen and opt in from there.

Reminders keep working after the login session expires. To turn them off, use Settings, sign out explicitly on
that browser, or revoke its **Background reminders** row under **Devices → Browsers**. A registration also ends
if the push service reports it is gone or you don't open the signed-in app on that browser for 30 days.

The server schedules live reminders for incomplete tasks. It uses VAPID and end-to-end encryption (RFC 8291):
the push service can't read the task ID, reminder time, or title (up to 200 characters). Notes are never sent.
The server retries temporary delivery failures with backoff up to 15 minutes after the due time. The service
worker needs no login or API call to show the notification. A click focuses an existing app tab and opens the
task without reloading. If no tab is open, a click opens `/app` on the task, signing in first if needed. The
worker handles no fetches or offline caching.

The in-tab schedule still uses the shared `alarms` list and ignores alarms more than 24 hours old. When background
reminders are on, a visible tab waits eight seconds before offering a toast. The service worker records the same
task and scheduled instant in IndexedDB to suppress the fallback. It always shows a notification for each push,
even if the tab already claimed it. Matching notification tags let the browser coalesce alerts. With Web Push
unavailable, **Settings → Enable reminder notifications** still controls the in-tab system alerts where supported;
otherwise the alert appears in the toast strip. The toast offers **Complete** and **Snooze 10 min** with Undo.

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

Below Attachments, **Activity** shows creation, the latest edit and the current completion, newest
first, from the same `todo_client::task_activity` view model as desktop and Android. The names, actions and
relative times match across clients. A later edit doesn't change the completion's actor; if that actor
wasn't recorded, the line says **Unknown actor**. This is an offline summary, not the full event feed.

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
**Shift+F10** opens the task menu, **M** opens the move picker, **Delete** deletes the task, and **N** focuses
quick-add. **/** or **Ctrl+K** opens the room filter. **Esc** closes
the active editor, overlay or filter. App shortcuts don't run while typing and held keys don't repeat actions.
The status bar shows shortcuts only when their actions are available.

## Tests

- **Unit tests** (`cargo test` in `apps/web`): the view-model against the fixtures (counts, chips, due dates,
  paths, the detail, tomorrow's plan) and the landscape markup (no colours or inline styles in it; the sun
  where the count says).
- **Browser tests** (`e2e/tests`, Playwright, Chromium): the fixtures build served by `e2e/serve.mjs` the way
  the server serves it, with the server's exact CSP (read from `spa.rs`). They add and complete tasks, undo,
  edit titles, steps and toggles, traverse room, view and task history (including deleted entries and a filter
  that adds none), use the keyboard and the address bar, follow the system theme, and fail on
  any console error or CSP violation. Task action tests cover context and detail menus, keyboard and filter
  navigation, focus after deletion and moves, reduced motion and Undo. Workflow tests cover both settings menus,
  defaults, status/range edits and suffixes, pointer and keyboard Board moves, Done, narrow layout and Undo.
  Reminder tests cover keyboard
  navigation and focus return, custom time validation, cancelling a draft, and Undo. With mocked notifications
  and clock, alert tests cover explicit permission, title-only delivery and task navigation, dedupe across tabs
  and reload, rescheduling and removal, and fallback actions. The docked splitter
  tests cover keyboard and pointer resizing, the live accessibility values, persistence through reloads
  and viewport changes, and wide-detail text and list layout. Assistant tests cover focus, responsive layout,
  a real streaming fetch and Stop, loopback validation, relay device sign-in and shared consent, Bearer versus session headers,
  per-owner storage, and exact outbound bodies for the model's list-access gate. Settings tests use the keyboard to open
  the same-origin admin pages from the desktop sidebar and phone drawer. Set `FIXTURE_PORT` to change the port
  from the default 4173.
- **The screenshot gallery** (`e2e/tests/gallery.spec.mjs`): every screen in both themes — desktop My Day with
  the detail, wide and narrow docked widths (including 1920 × 1080) and the hovered splitter, due-date editor,
  step controls and delete Undo, attachments (empty, uploading, failed, listed
  and removed with Undo), the email drop highlight (`email-drop-{light,dark}.png`), a collapsed collection, sidebar and room menus,
  rename and delete dialogs, a delete Undo toast, collection creation, the desktop and phone task move picker,
  list moves, list and collection swatches,
  repeat presets and custom repeat, reminder presets and the phone custom reminder editor, the tag picker
  and its creation, management, rename, colour and delete views,
  the link field and removal, the phone Not synced list, an active query filter, the finished
  day with a hand-over selection and with Completed closed and expanded, a finished day with no tasks tomorrow,
  an empty list, Trash (populated and empty, desktop and phone), Activity (unfiltered, agent-filtered,
  empty and refused undo), task detail activity on desktop and phone, the desktop and phone quick-add
  chips, calendar and priority pickers, the detail Due calendar (`{desktop,phone}-detail-due-calendar-{light,dark}.png`),
  the storage-error and read-only alerts (desktop and phone), the medium modal, and the phone's
  list, detail and drawer. Settings menus are captured on desktop (`sidebar-settings-{light,dark}.png`)
  and phone (`phone-settings-{light,dark}.png`). Reminder permission states are in
  `reminder-settings-{not-enabled,allowed,blocked,unsupported}-{light,dark}.png` and fallback toasts in
  `reminder-toast-{light,dark}.png` and `reminder-toast-phone-{light,dark}.png`.
  Background reminder states are in `web-push-{off,on,blocked}-{light,dark}.png` and the matching
  `-150.png` captures at 150%. The On and Blocked `-menu-250.png` crops show the menu at 250%.
  Workflow captures `board-{light,dark}.png`, `board-detail-{light,dark}.png`, `board-phone-{light,dark}.png`,
  `board-drag-{light,dark}.png`, `board-move-{light,dark}.png`, `list-status-{light,dark}.png` and
  `list-settings-{light,dark}.png` against the same renovation data as desktop.
  Notes fixtures cover the supported Markdown elements and overflowing code at 200% in both themes.
  The PNGs land in `apps/web/e2e/gallery/`, including
  `sidebar-{light,dark}.png` and `sidebar-narrow-{light,dark}.png` crops;
  CI uploads them as `web-gallery-fixtures-1` and `web-gallery-fixtures-2`. Assistant shots (`ai-*.png`)
  capture wide unconfigured, empty and edit states, medium batch and settings, and the narrow creation sheet at 2×.
  Compare them with the approved mockups after every UI change.
- **Live tests** (`e2e/live`, `live.config.mjs`): a real `todo-server` (from `target/debug`, or
  `$TODO_SERVER_BIN`) with a fresh data directory on loopback, and the real build from `apps/web/dist`. A
  virtual WebAuthn authenticator registers the first passkey with the setup code. The test then uses the app:
  the session + CSRF API, an assistant batch approved in part and undone, a rejected edit, a stale review after
  another tab writes, a change from a second tab arriving over the event stream, an edit made while the
  API is unreachable that survives a reload and is sent afterwards, **Sync now** reconnecting the event
  stream without waiting for backoff and being disabled while connecting, completing, and a request without the CSRF
  header being refused. Activity reads confirmed edits from another tab and undoes one through the session.
  The suite also uploads, downloads and removes a file with the web session, sets a repeat rule and sees
  the next occurrence on completion, adds and removes tags and links across tabs, checks tag delete confirmation
  against a live change in membership, and moves a list between collections. A task move, deletion and both
  undos are confirmed by a second tab. The suite also checks that a remote task move invalidates the
  picker and that remote list edits preserve keyboard focus. It verifies synced reminder
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

- **Offline start.** The reminder service worker does not cache assets, so the app can't load without the server.
- **Damaged records** are set aside and counted, but the app can't show or export them yet.
- **An idle tab is signed out.** The event stream doesn't extend the session, so a tab left alone for longer
  than `session_idle_timeout_mins` is sent to sign-in when the session lapses. Pending changes are kept.
- **The wasm size.** `wasm-opt` and Leptos's `erase_components` haven't been tried in the release build yet.
