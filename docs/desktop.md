# Desktop app

`apps/desktop` is the GPUI desktop client. It is Windows-only for now: on other platforms the binary builds
and prints that it isn't supported yet. It pairs with the owner's server, syncs through `todo-client`, keeps
a local cache so it starts with data on screen, and shows and edits the real lists and tasks in the F2
"Playful" layout. Around that sit the tray, the window lifecycle, the global quick-add hotkey and
autostart.

## Building and running

```sh
cd apps/desktop
cargo run --release                    # the app; --background starts it in the tray
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
pwsh scripts/gallery.ps1               # screenshot gallery, see below
```

### Its own workspace

`apps/desktop` is a separate Cargo workspace with its own `Cargo.lock`. The root workspace excludes it.

- **GPUI is pinned to a Zed git revision** (`2f808b3c`, `gpui` 0.2.2), and it brings a large dependency
  tree. Keeping it out of the root lock means the server and the shared crates never resolve against it.
- **It patches crates**: a vendored `gpui_windows` (below) and `async-task` (a waker identity fix,
  smol-rs/async-task#108, that GPUI needs). Patches apply to a whole workspace, and the server shouldn't
  carry them.
- **The Linux CI jobs stay as they were**: `cargo … --workspace` at the root doesn't see the desktop app. A
  separate Windows job builds, lints and tests it.

It depends on `todo-design` (with `embed-fonts`), `todo-core` and `todo-client` (with `sqlite`) by path. The
shared crates stay portable, so that works from this workspace too. Its tests also use `todo-server` by
path, as an in-process server.

The toolchain comes from the root `rust-toolchain.toml` (1.98.1), which is pinned to what GPUI needs.

### Vendored `gpui_windows`

`vendor/gpui_windows` is upstream `gpui_windows` at the pinned revision plus a few patches, each marked
`TO-DO PATCH`. `vendor/gpui_windows/VENDORED.md` lists them and explains how to upgrade:

- **WARP fallback.** If no hardware adapter can create a Direct3D 11 device (CI runners, VMs, remote
  desktop), the renderer uses the WARP software adapter instead of failing. `TODO_FORCE_WARP=1` forces it.
- **Frame capture.** A window can request a readback of its next frame, which is taken from its own
  render target before it's presented. The screenshot mode uses it.
- **Hidden capture windows.** With the screenshot scale override set, the renderer sizes the hidden HWND
  with `SWP_NOACTIVATE`, skips activation and keeps the pointer outside the fixture. It requests lower
  DXGI device scheduling priority and paints on demand instead of at every vsync.
- **Present timestamps.** The time of each window's first and latest present, for the benchmarks.
- **Scale factor override.** Lets the screenshot mode render at 150 % on a 100 % monitor. While it's set,
  the window keeps the bounds it asked for even when they don't fit on the display, and
  `WM_GETMINMAXINFO` lifts the maximum track size, so a 2160 × 1350 capture works on a 1024 × 768 CI
  screen.

## gpui-component: not adopted

[gpui-component](https://github.com/longbridge/gpui-component) has a lot of ready-made widgets (inputs,
lists, menus, dock), so it was the first thing evaluated. The decision is to **build our own small widgets
on top of GPUI** and revisit the question later.

- **It doesn't build at our GPUI pin.** v0.10.0 depends on `gpui-pre =0.3.12` from crates.io, which is a
  snapshot of a different Zed revision. Patching `gpui-pre` to our git revision doesn't work: Cargo
  accepts the patch but never uses it because the versions differ (0.2.2 against 0.3.12). That leaves two
  copies of GPUI in the tree, and their types don't mix.
- **Adopting it means re-pinning GPUI to its snapshot**, then following its weekly snapshot bumps. Our pin
  was chosen in the GPUI spike together with the vendored Windows fixes and the toolchain, so moving it is
  a separate decision.
- **It's heavy.** At its own pin it builds about 500 crates and takes about 2.5 minutes for a clean release
  build.
- **F2 needs its own look anyway.** The design uses round checkboxes, pills only for list chips, Fraunces
  room titles and the painted header landscape. We'd restyle most components rather than use them as they
  are.

What we use instead: the text input from GPUI's `input` example (`src/ui/input.rs`, IME, selection and
clipboard included), a multi-line version of it for notes (`src/ui/text_area.rs`, wrapping with GPUI's
text system), GPUI's own `uniform_list` for the virtualized task list, `anchored` overlays for menus, and
plain `div`s in `src/ui/kit.rs`. **Revisit** when the app needs a date picker or richer menus. If
gpui-component has moved onto a Zed revision that we can pin to (or onto the same snapshot crates),
adopting it then is cheap.

## Pairing, config and sync

**Pairing.** On first run (no saved pairing) the window shows the pairing screen (`src/ui/pairing.rs`). The
owner types the server's address, for example `todo.example.com`; `https://` is added, and plain `http://`
is refused except for `localhost`, so the token never crosses a network unencrypted. The app calls
`POST /api/v1/pair/start` with the device name ("To-do on <computer name>", read at run time), shows the
user code, opens the approval page in the browser and polls `POST /api/v1/pair/poll` at the interval the
server asks for. The address is checked with the transport's own parser (`todo_client::native::server_origin`):
https, or plain http only for this computer, and no user name, query or fragment. The approval page is built
from that address (`<server>/pair?code=…`); the `verification_uri` the server sends is never opened. The
transport follows no redirects and gives every finite request 30 s. Rate limits wait for `Retry-After`; a run of network failures, a denial or an expired code
go back to the address with a message. Nothing spins while it waits. **The server address is never built
into the app**: it is learnt here and saved in the user's config.

**Where things are kept** (`src/config.rs`):

| File | Where | Contents |
|---|---|---|
| `config.json` | `%APPDATA%\to-do-or-not-to-do\` | the server URL, the device ID and a random pairing ID |
| `device-token.bin` | same | the device token, sealed with DPAPI (`CryptProtectData`, current user, with app-specific entropy) |
| `pairings\<pairing ID>\cache.sqlite` | `%LOCALAPPDATA%\to-do-or-not-to-do\` | the sync engine's replica and outbox |
| `pairings\<pairing ID>\ui.json` | same | local sidebar preferences (collapsed collections, never synced) |
| `pairings\<pairing ID>\attachments\<random>\` | same | an attachment downloaded to be opened: a new owner-only directory for every open |
| `pairings\<pairing ID>\damaged\<ms>\` | same | damaged data set aside by **Start fresh**, kept until sign-out |
| `pairings\<pairing ID>.signed-out` | same | the record that the pairing beside it was signed out and its data is to be removed |

`TODO_DESKTOP_HOME` puts everything in one directory instead (tests, benchmarks, a second profile). The token
is never written in plain text and never logged. DPAPI's keys belong to the user's profile: they protect the
token from other accounts and from a copy taken off the machine, not from programs running as the same user,
and with a roaming profile they travel with it. Each pairing's data lives under its own pairing ID, so a new
pairing never picks up an old cache and cleaning up an old one can't touch the current one.

**Sign out** (the gear in the sidebar footer) runs in order: syncing stops and downloads in flight are
cancelled and waited for; the record `pairings\<pairing ID>.signed-out` is written beside the directory and
flushed to disk (if it can't be, nothing is removed: the app says so and stays signed in, syncing again, and
signing out can be tried again); the token and the config are removed (if that fails, the record is
withdrawn, and the app says so and stays signed in the same way); the shell lets go of the cache, and the directory (cache, sidebar preferences and downloads) is removed; the
record goes last. Only then is pairing shown. A directory still in use is removed at the next start, which
only ever removes directories that have a record, named exactly by a pairing ID, and never the current one.

Removal never leaves our own data. The app directory and `pairings` must be plain directories (a junction or
symlink there stops all removal); a link at a pairing's directory, or anywhere inside it, is removed as a
link and never followed. The server has no revoke endpoint for a device yet; revoke it there too if the
computer is lost.

**Sync** (`src/session.rs`). The app runs `todo_client::SyncClient` with the native HTTP transport and the
SQLite storage on a small tokio runtime of its own (two worker threads). Initial load, catch-up, the SSE
event stream, the offline outbox and reconnecting with backoff are the client's; the UI thread only calls
its synchronous `view`, `submit` and `status` and awaits its change signal, coalesced to one frame. A
command applies to the view at once and is queued for the server; one the domain refuses shows a message in
the status bar. Attachments download on the runtime too.

**Startup.** A loader thread reads the config, unseals the token and opens the cache while GPUI sets up
Direct3D and DirectWrite, so a warm start draws real data in its first frame. Opening never deletes anything:

- **A damaged cache** (SQLite reports it malformed or not a database, or stored data doesn't decode) is
  recovered in steps, each recorded in `recovery.json` before the next one starts: the cache and its
  write-ahead log are copied aside as `cache.damaged-<ms>.sqlite` and flushed, then the original is removed.
  The copy's outbox and "couldn't sync" list are read back (`SqliteStorage::salvage_all`, which fails on any
  read error rather than come back short) into a new cache built as `cache.recovering.sqlite`, which then
  replaces `cache.sqlite`; the record goes last. A copy that can't be read stops the recovery behind the
  "couldn't be opened" screen, keeping the record and the copy for the next try. A crash or a
  failure at any step leaves the record, and the next start finishes the recovery from there, so an unsent
  edit is never lost. The confirmed state is loaded from the server again. The status bar says how many
  unsent changes were kept.
- **A recovery that can't finish** (the copy can never be read, say) doesn't lock the app: while
  `recovery.json` is there, the screen also offers **Start fresh (keep the damaged copy)**, after asking,
  since it gives up on the unsent changes. It first records the destination, a new `damaged\<ms>\` folder in
  the pairing's directory, in `start-fresh.json`, then moves each damaged database with its log files, then
  `recovery.json`, then `start-fresh.json` last (nothing is deleted), and opens an empty cache that loads
  from the server. It refuses, moving nothing, if the pairing's directory or any of those files is a link or
  not a plain file. While `start-fresh.json` is there, nothing opens or salvages the half-moved files: the
  screen offers only **Finish starting fresh**, which continues into the same folder. The status bar says the unsent changes
  weren't recovered, with **Open folder** for the kept copy. Kept copies are removed with the pairing's data
  at sign-out.
- **A cache that can't be opened now** (another copy of the app holds it, no permission, disk full) is left
  as it is. A screen says what happened, with **Try again**.
- **A saved sign-in that can't be used** goes back to pairing with the reason. The old data stays.

## The views

The views draw a `model::Shell`, which `model::derive` computes from a `todo_core::State`, the window's UI
state (room, selection, filter, Completed open, tasks being completed), the sync status and the clock. It is
pure and unit-tested; the running app and the screenshot fixtures go through the same code.

| Part | What it does |
|---|---|
| Sidebar | Smart lists (My Day, Important, Planned, All, Completed, Agent activity) with counts from the query language's built-in lists, collections and lists in server order with open counts. A collection's chevron or context menu hides or shows its lists without changing the room. A collapsed header shows the selected look when it hides the current list; navigating to that list reveals it. List colours snap to the design's swatches; glyphs come from the list's name (English and Czech keywords). Double-click or right-click renames; the context menu moves a list up or down or deletes it (after asking); "New list" adds one and starts renaming it. Agents seen in the last week, and the sync state. |
| Task list | Open tasks in server order (by list, then position, in a collection), virtualized; Completed below, newest first, behind a toggle. Checkbox completes or reopens; a completed task holds its place (fill, tick, dimmed title), fades and leaves. The priority mark cycles on click. **J/K** or arrows move, **Space** completes, **E** edits the title, **D** the due date, **Alt+Up/Down** moves a task in a list. The title-bar field filters (**Ctrl+K**): a query (`tag:docs`, `due:today`) or plain words. |
| Quick add | In the room (**Ctrl+N**) and global (**Ctrl+Alt+Space**, into My Day). A trailing date (`tomorrow`, `fri`, `+3d`, `2026-12-01`, `zítra`, `pátek`) and priority (`!`, `!!`, `!!!`) are read off the title; the date shows as a chip while typing. My Day, Important and Planned shape the new task. |
| Detail | Title and notes edited in place (notes are multi-line plain text), steps added, completed, renamed, moved and deleted, My Day and Important toggles, the due date typed in. Reminders, repeats and tags are shown but not edited yet. A title or notes save that fails keeps the text in the editor, and the task stays selected until it saves. Only `http` and `https` links open, in the browser; anything else (`file:`, UNC paths, other protocols) is shown as text. An attachment is saved where the owner chooses (a save dialog in Downloads): it is written to a temporary file beside the destination, marked, and then moved over it, so a failed save leaves an existing file as it was. Only documents and images (pdf, png, jpg, jpeg, gif, webp, txt, md, csv, docx, xlsx, pptx, eml, msg, decided on the cleaned file name) also offer **Open**, from a new owner-only directory for each open. Every downloaded file gets the Mark of the Web (`Zone.Identifier`, zone 3), so Office uses Protected View and SmartScreen checks it. A drive that can't hold it (FAT, exFAT) still gets a saved file, with a warning that Windows won't warn when it's opened; a file to open is refused without it. |
| Header | The sky, hills and sun from real counts: *done of total* and open. No estimate (tasks don't have one). My Day all done turns the sky to dusk and hands over to tomorrow. |

## The shell

| Feature | Where | Notes |
|---|---|---|
| Tray icon and menu | `src/platform/tray.rs` | Show or hide, quick add, "Start with Windows" (checkbox), quit. A left click toggles the window. The icon is the app icon from the exe's icon resource, loaded at the small-icon size for the display scale (16 px at 100 %, 24 px at 150 %); without the resource it falls back to the embedded 32 px render. |
| Close hides to tray | `src/app.rs` | Closing the main window hides it. Only "Quit" ends the app. If the tray icon can't be created, the window is shown even with `--background`, and closing it quits, so the process is never left running invisibly. |
| Single instance | `src/platform/single_instance.rs` | A named mutex marks the running instance, and a named event activates it. A second launch signals the event, allows the first to take the foreground and exits. The names carry the user's SID and live in the session's `Local\` namespace, so each user gets their own instance, and both objects have a DACL that admits only that user. The event is created before the mutex is claimed, so a launch that races the first one's startup is never lost. If the check can't be set up (an elevated copy created the objects, say), the app logs it and runs standalone: visible, and not listening for second launches. |
| Global quick add | `src/platform/hotkey.rs`, `src/ui/quick_add.rs` | **Ctrl+Alt+Space** opens a small always-on-top window, already rendered and hidden, so showing it is immediate. Enter adds the task to My Day (before pairing, it shows the pairing window instead). Esc or clicking away dismisses it. If another app owns the shortcut, the tray item says so and the quick-add window drops its shortcut hint; the tray item still opens it. |
| Start with Windows | `src/platform/autostart.rs` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `To-do or not to do`, `"<exe>" --background`. Per user, so no admin rights are needed. It's toggled from the tray. |
| Main window | `src/ui/root.rs`, `src/ui/shell.rs` | Pairing until paired, then the custom title bar with the filter, sidebar, list, detail pane and status bar, as in F2. |
| Header landscape | `src/ui/landscape.rs` | The sky with two flat hills and the sun on its arc at *done / total*, or the tall dusk scene once My Day is done. The geometry is `todo_design::landscape`; the paths are built once per size and progress, and are cached. |

### Design tokens

Every visual value comes from `todo-design` (see `docs/design-language.md`): palettes per theme and room
hue, list and agent colours, the dusk scene, text styles, spacing, radii and the desktop geometry.
`src/theme.rs` is a thin adapter that turns token colours into GPUI colours and applies text styles; the
views never hard-code a colour or a size. GPUI has no letter spacing at the pinned revision, so tracking is
only faked where it matters (the priority marks). A checked box on a selected row uses `check_selected`.

Icons are the shared Lucide subset and list glyphs (`todo_design::Icon` and `Glyph`), served to GPUI by
`src/assets.rs`.

### Fonts

The fonts are the shared static instances in `assets/fonts`, embedded in the binary through
`todo_design::fonts::FACES` (the `embed-fonts` feature) and registered at startup. The app never uses system
fonts. The families are Fraunces Display and Fraunces Text (room titles, italic eyebrows), Schibsted
Grotesk (UI) and Geist Mono (numbers and dates), all under the OFL.

## Screenshots

```sh
todo-desktop --screenshot-all <out-dir> [--overwrite]  # all ten fixtures × both themes × 100 % and 150 %
todo-desktop --screenshot <out.png> --fixture <my-day|work|collapsed-collection|long-sidebar|empty|finished|quick-add|pairing|pairing-code|open-failed|open-failed-confirm> [--theme light|dark] [--scale 1|1.25|1.5|2] [--overwrite]
```

Both modes use the same below-normal-priority fixture runner. It never enters interactive startup: no
SID-scoped instance mutex or activation event, profile/cache, network, tray, global hotkey or autostart.
It cannot activate an existing instance or receive a handoff. The batch loads GPUI and the fonts once,
then opens and closes **one never-shown HWND per shot**. GPUI's Windows headless platform has no real
text system or Direct3D window renderer. Instead the runner sizes a hidden window without activating it,
requests paint messages and reads its own render target. No visible window, taskbar entry, sound or input.
A capture never samples the live pointer: unlike the old visible gallery, it cannot accidentally show a
hovered row. To render a subset in one process, add `--fixture my-day,quick-add --theme dark --scale 1,1.5`
to `--screenshot-all`. For visual checks, use these modes, not the interactive app.

A fixture is a `todo_core::State` built from invented sample data by running commands through the domain
(as the owner and as agents, at fixed times), plus UI state, a fake sync status and a fixed clock. The
window is the real shell on a local store (`src/store.rs`). `pairing` and `pairing-code` show the pairing
screen before and after asking for a code. `open-failed` and `open-failed-confirm` show the "couldn't be
opened" screen after a recovery that can't finish, before and while confirming **Start fresh**.

The runner requests five settling frames before reading back the next frame. It doesn't capture the
screen. It refuses to replace an existing file unless `--overwrite` is given. It checks that each image
is the window's logical size times the scale. Exit code 0 means all PNGs were written and flushed to disk,
3 means a font family is missing, 4 means the output exists, and 1 is any other failure (including a
wrong image size).
If a shot fails mid-batch, the runner exits 1 and leaves the partial gallery on disk; earlier PNGs are
not rolled back.

`scripts/gallery.ps1` renders every fixture in both themes at 100 % and 150 % into
`apps/desktop/target/gallery/` (`<fixture>-<theme>-<pct>.png`, git ignores it), passing `--overwrite`. It
samples the foreground owner during the run and fails if it sees the renderer take focus (the owner can still
switch between their own apps). It prints the render and validation time, excluding the build. It also
checks that each PNG is complete (the signature and the final `IEND` chunk), and fails if its size isn't
1440 × 900 (main window) or 640 × 196 (quick add) times the scale. CI runs it and uploads the gallery as the
`desktop-gallery` artifact. Compare the images with the F2 mockups before calling a UI change done.

## Benchmarks

```sh
todo-desktop --bench startup --metrics <file>   # process start to the main window's first presented frame
todo-desktop --bench hotkey --metrics <file>    # hotkey handler to the quick-add window's presented frame, 20 samples
```

Both write `key = value` lines. `startup` measures from the process creation time that the kernel records
(`GetProcessTimes`), so the loader and runtime startup are included. `hotkey` runs exactly what the
`WM_HOTKEY` handler runs, against windows that have been hidden in the tray. It measures up to the present
call; DWM shows the frame at the next vblank, which adds up to one refresh interval.

To measure with real data, build a paired home with a warm cache of 600 tasks (mostly completed) from an
in-process server, then start the app on it (the server is gone, so it starts offline from the cache):

```sh
TODO_BENCH_HOME=<dir> cargo test --release make_a_bench_home -- --ignored
TODO_DESKTOP_HOME=<dir> todo-desktop --bench startup --metrics <file>
```

`data_loaded_ms` is when the loader thread's config, token and cache are ready, before the first window
opens.

Measured on a desktop with a discrete GPU, two 1080p monitors at 100 %, a release build, warm file caches
and the 600-task cache:

| Metric | Target | Measured |
|---|---|---|
| Cold start to first frame, with data | < 500 ms | 365–451 ms over 6 runs (median ≈ 401 ms). About 300–350 ms of it is GPUI's own platform setup (Direct3D device, DirectWrite); the cache is loaded in parallel and ready about 3 ms after it. |
| Hotkey to quick-add presented | < 100 ms | median 2.1 ms, p95 3.4 ms, max 6.2 ms (20 samples), plus at most one vblank |
