# Vendored: gpui_windows

Copied from zed-industries/zed, `crates/gpui_windows`, commit `2f808b3c2de049ecfdbad0fdb26763cebfcaa3b4`, which is
the GPUI revision `apps/desktop` is pinned to. License: Apache-2.0 (see `LICENSE-APACHE`). `Cargo.toml` has the
workspace inheritance resolved so the crate builds outside Zed's workspace.

Every change is marked `TO-DO PATCH` in the sources:

| Patch | Files | Why |
|---|---|---|
| WARP fallback | `src/directx_devices.rs` | Upstream fails when no hardware adapter gives a Direct3D 11 device. We fall back to WARP, the software rasterizer, so the app runs on VMs, Dev Boxes and CI runners without a GPU. `TODO_FORCE_WARP=1` forces WARP. |
| Frame capture | `src/todo_patch.rs`, `src/directx_renderer.rs` | `todo_patch::request_capture(hwnd)` makes the next `draw` copy the finished render target into a staging texture before presenting it. The `--screenshot` mode uses it; there is no screen capture and no input. (Upstream's `render_to_image` does the same but is compiled only with GPUI's `test-support`, which we don't want in the app.) |
| Fixed popups | `src/window.rs`, `src/events.rs` | Create non-resizable `PopUp` windows with an explicit frameless `WS_POPUP` rect at the target display's DPI. Do not defer sizing to GPUI activation: the native hotkey shows quick add directly and would leave it at `CW_USEDEFAULT` dimensions. On DPI changes, compute the physical size from the original requested logical size. Normal windows retain the upstream placement path. |
| Hidden capture windows | `src/window.rs`, `src/platform.rs`, `src/directx_renderer.rs` | With the screenshot scale override active, apply the requested bounds to a hidden HWND with `SWP_NOACTIVATE`, skip native activation, keep the pointer outside fixture content and request lower DXGI device scheduling priority. Paints are explicitly requested; vsync need not invalidate these HWNDs. Readback does not require showing the window. |
| Present timestamps | `src/todo_patch.rs`, `src/directx_renderer.rs` | `todo_patch::first_present` and `last_present` report when a window presented a frame. The cold-start and hotkey benchmarks measure against them. |
| Scale factor override | `src/todo_patch.rs`, `src/window.rs`, `src/display.rs`, `src/events.rs` | `todo_patch::set_scale_factor_override` lets the screenshot mode render at 150 % on a 100 % monitor, lifts the maximum window size so it may exceed the screen, and keeps the requested bounds even when their centre is off the display. |
| App icon | `src/window.rs`, `src/events.rs` | Upstream registers the window class with only the large icon (`hIcon`, resource id 1, which the exe's `build.rs` embeds from `assets/brand`), loaded once at the system DPI. The class now also gets `hIconSm` (`WNDCLASSEXW`, `RegisterClassExW`), and each window gets its own small and large icons with `WM_SETICON` (`set_window_icons`), loaded at the size `GetSystemMetricsForDpi` gives for the window's DPI: when the window is created and on `WM_DPICHANGED` (before the screenshot scale override returns). `load_icon_resource` caches one icon per size, so repeated DPI changes never leak a handle; it does not use `LR_SHARED`, which can return the first size loaded. |
| Browser navigation input | `src/events.rs` | Client-area `WM_XBUTTONDOWN/UP` already maps to GPUI's `MouseButton::Navigate`; return `TRUE` for these messages as Win32 expects. For the non-client caption, suppress default X-button handling on press and post the corresponding GPUI release event on `WM_NCXBUTTONUP`. `WM_APPCOMMAND` Browser Back/Forward also dispatches a navigation release without changing mouse capture; unrelated commands fall through. |
| File dialog allocations | `src/platform.rs` | Own each `GetDisplayName(SIGDN_FILESYSPATH)` result with a guard that calls `CoTaskMemFree`, including when UTF-16 conversion fails. Both the open/multi-select and save dialogs use it. |

To upgrade GPUI: copy `crates/gpui_windows/src` from the new Zed revision, re-apply the `TO-DO PATCH` hunks (search
for the marker in this copy), update the revision here and in `apps/desktop/Cargo.toml`, and rerun the spike's
benchmark and the screenshot gallery as a regression check.
