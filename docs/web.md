# Web app

`apps/web` is the browser client. It is Rust compiled to `wasm32-unknown-unknown`, with a
[Leptos](https://leptos.dev) UI rendered on the client (CSR). It uses the same `todo-core` and `todo-client`
as the other clients, so the state, the commands, the smart-list queries and the sync engine are exactly the
desktop's and Android's. `todo-server` serves it at `/app`, and it signs in with the owner's passkey session:
there is no token in the browser.

This first version covers sign-in, the smart lists and every collection and list, the task list, the task
detail, adding, completing (with the F2 check animation), editing titles, steps and notes, My Day, and live
updates. It has the desktop layout on wide windows and the phone layout on narrow ones, in both themes.

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

To serve the app, point the server at the build and restart it:

```toml
[web]
dir = "/opt/todo/web"        # a copy of apps/web/dist; or TODO_SERVER_WEB_DIR
```

With a build configured, `/` opens the app; without one, `/` opens the admin pages as before.

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
browsers that accept gzip (the wasm module is about 4.5 MB, 0.7 MB gzipped).

## Layout of the code

| File | What |
|---|---|
| `src/model.rs` | The view-model: sidebar counts, a room's header and rows, the detail pane, tomorrow's plan. Pure Rust over `todo_core::State`, unit-tested on the host. |
| `src/landscape.rs` | The header landscape as SVG markup, from `todo_design::landscape`. Shapes carry classes; `app.css` colours them from the room's tokens. |
| `src/app.rs` | The Leptos components. They set classes and `data-*` hue keys only. |
| `src/backend.rs` | The live backend (the sync engine) or the fixtures backend. |
| `src/platform.rs` | Browser helpers: clock and UTC offset, theme, history, CSRF token, downloads. |
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

`index.html` is sent with `no-store`, and with the session's CSRF token written in place of the marker. The
other files carry an `ETag` and `private, no-cache`, so a deploy is picked up on the next load.

### Headers

The page gets its own CSP:

```
default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data: blob:;
connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self';
object-src 'none'
```

`'wasm-unsafe-eval'` allows compiling the wasm module and nothing else; there is no `'unsafe-inline'` or
`'unsafe-eval'`. Styles come from the two stylesheets only. The app never writes a `style` attribute: dynamic
values are classes and `data-*` attributes, and the landscape's stroke widths and opacities are SVG
presentation attributes. The browser tests fail on any CSP violation. The rest of the server's headers
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
wrong token is a 401 even with a live session cookie. MCP and attachment uploads stay bearer-only. A session
acts as the owner on the device `web` (`Actor::Owner { device: "web" }`), so its changes are attributed like
any other device's. The session's event stream is closed when the session ends (sign-out, expiry), checked
on revocation and on every heartbeat.

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
- Every write also appends a batch of changes to a journal. `IdbFlusher` drains the journal into IndexedDB
  (one transaction per batch, in order), retrying if a write fails.
- At start the image is read back from IndexedDB. If it can't be read, the database is dropped and the
  replica starts again from the server: it is a cache, except for the outbox.

So a change made while the server can't be reached waits in the outbox, survives a reload or a closed tab,
and is sent when the app runs again (the live browser test does exactly this). The page itself still needs
the server to load: there is no service worker yet.

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
| 760–1199 px | sidebar and list; the detail opens over the list with the overlay shadow |
| < 760 px | phone: app bar, room pills, 64 px rows with the due slot, a bottom composer, a drawer for the sidebar, a full-screen detail |

760 is about the sidebar plus the phone's inner-screen list (`--phone-inner-list`) plus padding; 1200 leaves
the list at least 560 px next to both side panes.

Completing a task runs the F2 choreography: the fill pops (`--duration-check-fill`, `--ease-pop`), the tick
draws, the row holds in place struck through (`--duration-complete-hold`), then collapses
(`--duration-row-collapse`), and only then does the sun move along its arc (`--duration-sun-travel`, eased
with `motion::SUN`) and the count change. The toast offers Undo for `--duration-undo-window`, with a thin line
running out; Ctrl+Z works while it's shown. Reduced motion turns movement into instant changes.

Keyboard: **J/K** or the arrows move the selection, **Space** completes, **E** edits the title, **N** focuses
quick-add, **Esc** closes the detail.

## Tests

- **Unit tests** (`cargo test` in `apps/web`): the view-model against the fixtures (counts, chips, due dates,
  paths, the detail, tomorrow's plan) and the landscape markup (no colours or inline styles in it; the sun
  where the count says).
- **Browser tests** (`e2e/tests`, Playwright, Chromium): the fixtures build served by `e2e/serve.mjs` the way
  the server serves it, with the server's exact CSP (read from `spa.rs`). They add and complete tasks, undo,
  edit titles, steps and toggles, use the keyboard and the address bar, follow the system theme, and fail on
  any console error or CSP violation.
- **The screenshot gallery** (`e2e/tests/gallery.spec.mjs`): every screen in both themes — desktop My Day with
  the detail, a collection, the finished day, an empty list, a medium window, and the phone's list, detail and
  drawer. The PNGs land in `apps/web/e2e/gallery/`; CI uploads them as `web-gallery`. Compare them with the
  approved mockups (kept outside this repository) after every UI change.
- **Live tests** (`e2e/live`, `live.config.mjs`): a real `todo-server` (from `target/debug`, or
  `$TODO_SERVER_BIN`) with a fresh data directory on loopback, and the real build from `apps/web/dist`. A
  virtual WebAuthn authenticator registers the first passkey with the setup code. The test then uses the app:
  the session + CSRF API, a change from a second tab arriving over the event stream, an edit made while the
  API is unreachable that survives a reload and is sent afterwards, completing, and a request without the CSRF
  header being refused. The port is `LIVE_PORT` (default 47412).
- **Server tests** (`crates/todo-server/tests/web_app.rs`): the session path of the API (reads and writes as
  the `web` device, missing or wrong CSRF tokens, cross-origin and originless writes, expired and ended
  sessions, bearer precedence, MCP and uploads staying bearer-only), the `/app` routes and headers, gzip, and
  the event stream closing on sign-out.

## Not yet

- **Offline start.** There is no service worker, so the app can't load without the server (Web Push and PWA
  install are out of scope for this version, and would come with it).
- **An idle tab is signed out.** The event stream doesn't extend the session, so a tab left alone for longer
  than `session_idle_timeout_mins` is sent to sign-in when the session lapses. Pending changes are kept.
- **Editing more fields.** Due dates, reminders, repeat rules and tags are shown but not editable yet; there is
  no moving between lists, reordering, search, or creating lists and collections.
- **Attachments** are download links (fetched with the session, saved as a blob); there is no upload.
- **The wasm size.** About 4.5 MB (0.7 MB gzipped). `wasm-opt` and Leptos's `erase_components` haven't been
  tried in the release build yet.
