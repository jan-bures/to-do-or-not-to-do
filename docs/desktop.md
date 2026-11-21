# Desktop app

`apps/desktop` is the GPUI desktop client. It is Windows-only for now: on other platforms the binary builds
and prints that it isn't supported yet. This step is the **shell**: the tray, the window lifecycle, the
global quick-add hotkey, autostart, and the F2 "Playful" window skeleton with invented sample data. It
doesn't talk to the server yet.

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

It depends on `todo-design` by path (with `embed-fonts`), and will depend on `todo-core` and `todo-client`
the same way once it syncs. The shared crates stay portable, so that works from this workspace too.

The toolchain comes from the root `rust-toolchain.toml` (1.98.1), which is pinned to what GPUI needs.

### Vendored `gpui_windows`

`vendor/gpui_windows` is upstream `gpui_windows` at the pinned revision plus a few patches, each marked
`TO-DO PATCH`. `vendor/gpui_windows/VENDORED.md` lists them and explains how to upgrade:

- **WARP fallback.** If no hardware adapter can create a Direct3D 11 device (CI runners, VMs, remote
  desktop), the renderer uses the WARP software adapter instead of failing. `TODO_FORCE_WARP=1` forces it.
- **Frame capture.** A window can request a readback of its next frame, which is taken from its own
  render target before it's presented. The screenshot mode uses it.
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
clipboard included) and plain `div`s in `src/ui/kit.rs`. **Revisit** when the app needs a virtualized list,
a menu or a date picker. If gpui-component has moved onto a Zed revision that we can pin to (or onto the
same snapshot crates), adopting it then is cheap.

## The shell

| Feature | Where | Notes |
|---|---|---|
| Tray icon and menu | `src/platform/tray.rs` | Show or hide, quick add, "Start with Windows" (checkbox), quit. A left click toggles the window. The icon is drawn in code (sun over hills). |
| Close hides to tray | `src/app.rs` | Closing the main window hides it. Only "Quit" ends the app. If the tray icon can't be created, the window is shown even with `--background`, and closing it quits, so the process is never left running invisibly. |
| Single instance | `src/platform/single_instance.rs` | A named mutex marks the running instance, and a named event activates it. A second launch signals the event, allows the first to take the foreground and exits. The names carry the user's SID and live in the session's `Local\` namespace, so each user gets their own instance, and both objects have a DACL that admits only that user. The event is created before the mutex is claimed, so a launch that races the first one's startup is never lost. If the check can't be set up (an elevated copy created the objects, say), the app logs it and runs standalone: visible, and not listening for second launches. |
| Global quick add | `src/platform/hotkey.rs`, `src/ui/quick_add.rs` | **Ctrl+Alt+Space** opens a small always-on-top window, already rendered and hidden, so showing it is immediate. Enter adds the task to the main window's list. Esc or clicking away dismisses it. If another app owns the shortcut, the tray item says so and the quick-add window drops its shortcut hint; the tray item still opens it. |
| Start with Windows | `src/platform/autostart.rs` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `To-do or not to do`, `"<exe>" --background`. Per user, so no admin rights are needed. It's toggled from the tray. |
| Window skeleton | `src/ui/shell.rs` | Custom title bar with search, sidebar, list, detail pane and status bar, as in F2. |
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
todo-desktop --screenshot <out.png> --fixture <my-day|work|empty|finished|quick-add> [--theme light|dark] [--scale 1|1.25|1.5|2] [--overwrite]
```

The screenshot mode opens one window with a fixture (invented sample data and a fixed date), waits for it to
settle, reads back its next frame from the window's own render target and exits. It doesn't capture the
screen and doesn't send any input. It refuses to replace an existing file unless `--overwrite` is given,
and it checks that the captured image is the window's logical size times the scale. Exit code 0 means the
PNG was written and flushed to disk, 3 means a font family is missing, 4 means the output exists, and 1 is
any other failure (including a wrong image size).

`scripts/gallery.ps1` renders every fixture in both themes at 100 % and 150 % into
`apps/desktop/target/gallery/` (`<fixture>-<theme>-<pct>.png`, git ignores it), passing `--overwrite`. It
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

Measured on a desktop with a discrete GPU, two 1080p monitors at 100 %, a release build and warm file
caches:

| Metric | Target | Measured |
|---|---|---|
| Cold start to first frame | < 500 ms | 373–496 ms over 6 runs (median ≈ 410 ms). About 300–400 ms of it is GPUI's own platform setup (Direct3D device, DirectWrite). |
| Hotkey to quick-add presented | < 100 ms | median 2.5–2.8 ms, p95 ≤ 3.0 ms, max 3.4 ms (2 × 20 samples), plus at most one vblank |
