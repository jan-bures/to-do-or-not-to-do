# Desktop app

`apps/desktop` is the GPUI desktop client. It is Windows-only for now: on other platforms the binary builds
and prints that it isn't supported yet. This step is the **shell**: the tray, the window lifecycle, the
global quick-add hotkey, autostart, and the F2 "Playful" window skeleton with invented sample data. It
doesn't talk to the server yet.

## Building and running

```sh
cd apps/desktop
python scripts/fetch_fonts.py          # once; needs fontTools (pip install fonttools==4.65.0)
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

The app will depend on `todo-core` and `todo-client` by path once it syncs. Both crates stay portable, so
that works from this workspace too.

The toolchain comes from the root `rust-toolchain.toml` (1.98.1), which is pinned to what GPUI needs.

### Vendored `gpui_windows`

`vendor/gpui_windows` is upstream `gpui_windows` at the pinned revision plus a few patches, each marked
`TO-DO PATCH`. `vendor/gpui_windows/VENDORED.md` lists them and explains how to upgrade:

- **WARP fallback.** If no hardware adapter can create a Direct3D 11 device (CI runners, VMs, remote
  desktop), the renderer uses the WARP software adapter instead of failing. `TODO_FORCE_WARP=1` forces it.
- **Frame capture.** A window can request a readback of its next frame, which is taken from its own
  render target before it's presented. The screenshot mode uses it.
- **Present timestamps.** The time of each window's first and latest present, for the benchmarks.
- **Scale factor override.** Lets the screenshot mode render at 150 % on a 100 % monitor, and lets the
  window grow larger than the screen.

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
| Close hides to tray | `src/app.rs` | Closing the main window hides it. Only "Quit" ends the app. |
| Single instance | `src/platform/single_instance.rs` | A named mutex marks the running instance, and a named event activates it. A second launch signals the event, allows the first to take the foreground and exits. Both live in the session's `Local\` namespace. |
| Global quick add | `src/platform/hotkey.rs`, `src/ui/quick_add.rs` | **Ctrl+Alt+Space** opens a small always-on-top window, already rendered and hidden, so showing it is immediate. Enter adds the task to the main window's list. Esc or clicking away dismisses it. |
| Start with Windows | `src/platform/autostart.rs` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `To-do or not to do`, `"<exe>" --background`. Per user, so no admin rights are needed. It's toggled from the tray. |
| Window skeleton | `src/ui/shell.rs` | Custom title bar with search, sidebar, list, detail pane and status bar, as in F2. |
| Header landscape | `src/ui/landscape.rs`, `src/geometry.rs` | The sky with two flat hills and the sun on its arc at *done / total*. The paths are built once per size and progress, and are cached. |

### Design tokens

Every visual value (colours per theme and room hue, type sizes, weights, spacing, radii) lives in
`src/theme.rs`, so the switch to the shared design-tokens crate is a change to one file. The palettes are
the F2 values for the rooms that the fixtures use (My Day, Work, Reading, Personal). Other hues fall back
to My Day until the token crate computes them.

### Fonts

The app loads its fonts from files at startup and never uses system fonts. The families are Fraunces (room
titles and italic eyebrows), Schibsted Grotesk (UI) and Geist Mono (numbers and dates), all under the OFL.
The directory is resolved in this order:

1. `--fonts-dir <dir>`;
2. the `TODO_FONTS_DIR` environment variable;
3. an `assets/fonts` directory next to the executable or in any directory above it. This is the shared
   assets directory, once it lands;
4. `apps/desktop/target/fonts`, which `scripts/fetch_fonts.py` fills in the meantime.

The script downloads the variable fonts from a pinned google/fonts commit and cuts the static instances the
design uses (for example, Fraunces at opsz 144, SOFT 100, weight 360). It also deletes the `STAT` table:
with it, DirectWrite names the family "Fraunces 144pt SuperSoft NonWonky".

## Screenshots

```sh
todo-desktop --screenshot <out.png> --fixture <my-day|work|empty|quick-add> [--theme light|dark] [--scale 1|1.25|1.5|2]
```

The screenshot mode opens one window with a fixture (invented sample data and a fixed date), waits for it to
settle, reads back its next frame from the window's own render target and exits. It doesn't capture the
screen and doesn't send any input. Exit code 0 means the PNG was written, and 3 means fonts are missing.

`scripts/gallery.ps1` renders every fixture in both themes at 100 % and 150 % into
`apps/desktop/target/gallery/` (`<fixture>-<theme>-<pct>.png`, git ignores it). CI runs it and uploads the
gallery as the `desktop-gallery` artifact. Compare the images with the F2 mockups before calling a UI change
done.

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
