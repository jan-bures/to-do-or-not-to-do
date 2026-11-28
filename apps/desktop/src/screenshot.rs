//! Render one or more fixtures, sequentially, without starting the interactive app.
//!
//! Deterministic: the fixture data and the clock are fixed, the scale factor is forced, and
//! the image is read back from the window's own render target (`todo_patch::request_capture`) instead of
//! capturing the screen. Control-state fixtures send synthetic pointer events to the hidden window.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, MouseButton, MouseDownEvent,
    MouseMoveEvent, PlatformInput, QuitMode, TitlebarOptions, WindowBounds, WindowOptions, point,
    px, size,
};
use gpui_platform::application;
use gpui_windows::todo_patch;

use crate::args::ScreenshotOptions;
use crate::assets::Assets;
use crate::model;
use crate::platform::win;
use crate::store::Store;
use crate::theme::{Material, Theme, desktop};
use crate::ui::failed::FailedView;
use crate::ui::kit::CaptionPreview;
use crate::ui::pairing::PairingView;
use crate::ui::quick_add::{self, QuickAddView};
use crate::ui::shell::{self, ShellView};
use crate::updates::{self, State as UpdateState};
use crate::{fixtures, fonts, ui};

/// The main window's size in the fixtures: the design's reference window, 1440 × 900 (the three panes side by
/// side).
const MAIN_SIZE: (f32, f32) = (desktop::SIDEBAR + desktop::LIST + desktop::DETAIL, 900.);

/// The logical window size a fixture renders at: the reference window, except the quick-add window, the
/// narrowest main window (`long-names-narrow`, `narrow-detail`), a short one (`finished-short`) and the
/// smallest (`short-sidebar`), or the large wide-detail showcase.
pub fn window_size(fixture: &str) -> (f32, f32) {
    match fixture {
        "popout-pinned" | "popout-unpinned" => model::popouts::SIZE,
        "quick-add" => (
            f32::from(quick_add::SIZE.width),
            f32::from(quick_add::SIZE.height),
        ),
        "long-names-narrow" | "narrow-detail" | "narrow-undo-toast" => {
            (crate::app::MIN_WINDOW.0, MAIN_SIZE.1)
        }
        "finished-short" => (MAIN_SIZE.0, 700.),
        "detail-wide-1920" => (1920., 1080.),
        "short-sidebar" | "short-sidebar-new-list" | "short-sidebar-new-collection" => {
            crate::app::MIN_WINDOW
        }
        _ => MAIN_SIZE,
    }
}

/// Typed into the quick-add fixture, to show its focused state.
const QUICK_ADD_TYPED: &str = "Call the venue about Friday's room";

fn fixture_window_options(width: f32, height: f32) -> WindowOptions {
    WindowOptions {
        // Top-left of the primary display; the window may be larger than the screen at 150 %.
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(width), px(height)),
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("To-do (screenshot)".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        focus: false,
        show: false,
        window_background: Material::Acrylic.window_background(),
        is_resizable: false,
        ..WindowOptions::default()
    }
}

/// Exit code when the fonts are missing.
pub const EXIT_FONTS: i32 = 3;
/// Exit code when the output exists and `--overwrite` wasn't given.
pub const EXIT_EXISTS: i32 = 4;

/// The image size a window of `logical` size renders to at `scale`.
pub fn expected_size(logical: (f32, f32), scale: f32) -> (u32, u32) {
    let px = |v: f32| (v * scale).round() as u32;
    (px(logical.0), px(logical.1))
}

/// Writes the PNG, refusing to replace an existing file unless `overwrite`.
fn save(image: &image::RgbaImage, out: &std::path::Path, overwrite: bool) -> anyhow::Result<()> {
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(!overwrite)
        .create(overwrite)
        .truncate(overwrite)
        .open(out)
        .with_context(|| format!("creating {}", out.display()))?;
    let mut w = std::io::BufWriter::new(file);
    image
        .write_to(&mut w, image::ImageFormat::Png)
        .with_context(|| format!("writing {}", out.display()))?;
    // Dropping a BufWriter swallows the last flush's error, so flush explicitly, then make it durable:
    // a truncated PNG must not be reported as written.
    let file = w
        .into_inner()
        .map_err(|e| e.into_error())
        .with_context(|| format!("writing {}", out.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", out.display()))?;
    Ok(())
}

/// One GPUI platform and font set per process. This entry point has no interactive app,
/// instance mutex/event, profile loader, tray, hotkey or sync session.
pub fn run(shots: Vec<ScreenshotOptions>) -> anyhow::Result<i32> {
    win::lower_process_priority()?;
    if let Some(opts) = shots
        .iter()
        .find(|opts| !opts.overwrite && opts.out.exists())
    {
        eprintln!(
            "error: {} exists; pass --overwrite to replace it",
            opts.out.display()
        );
        return Ok(EXIT_EXISTS);
    }
    let code = Rc::new(Cell::new(1));
    let code_in = code.clone();

    application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            let missing = fonts::load(cx);
            if !missing.is_empty() {
                eprintln!("error: fonts missing: {missing:?}");
                code_in.set(EXIT_FONTS);
                cx.quit();
                return;
            }
            cx.bind_keys(ui::input::key_bindings());
            cx.bind_keys(ui::text_area::key_bindings());
            cx.bind_keys(shell::key_bindings());
            cx.spawn(async move |cx| {
                if shots.iter().any(|opts| opts.fixture == "quick-add")
                    && let Err(e) = check_native_quick_add(cx).await
                {
                    eprintln!("error checking native quick add: {e:#}");
                    cx.update(|cx| cx.quit());
                    return;
                }
                for opts in shots {
                    if let Err(e) = render(cx, &opts).await {
                        eprintln!("error rendering {}: {e:#}", opts.out.display());
                        cx.update(|cx| cx.quit());
                        return;
                    }
                }
                code_in.set(0);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(code.get())
}

async fn render(cx: &mut AsyncApp, opts: &ScreenshotOptions) -> anyhow::Result<()> {
    let fixture = fixtures::by_name(&opts.fixture)
        .with_context(|| format!("unknown fixture {}", opts.fixture))?;
    todo_patch::set_scale_factor_override(Some(opts.scale));
    let theme = opts.theme;
    let quick = opts.fixture == "quick-add";
    let caption = match opts.fixture.as_str() {
        "caption-hover" => CaptionPreview::MaximizeHover,
        "caption-close-hover" => CaptionPreview::CloseHover,
        _ => CaptionPreview::Live,
    };
    // The pairing screen, before (`pairing`) and after (`pairing-code`) asking for a code.
    let pairing = match opts.fixture.as_str() {
        "pairing" => Some(false),
        "pairing-code" => Some(true),
        _ => None,
    };
    // The failed-open screen after a recovery couldn't finish, before (`open-failed`) and while
    // (`open-failed-confirm`) confirming Start fresh.
    let failed = match opts.fixture.as_str() {
        "open-failed" => Some(false),
        "open-failed-confirm" => Some(true),
        _ => None,
    };
    let logical = window_size(&opts.fixture);
    let expected = expected_size(logical, opts.scale);
    let main_options = || {
        let mut options = fixture_window_options(logical.0, logical.1);
        if fixture.ui.compact {
            options.is_resizable = true;
        }
        options
    };
    let opened_at = Instant::now();
    let handle: AnyWindowHandle = cx.update(|cx| {
        if quick {
            let mut opts = quick_add::window_options(cx);
            opts.window_background = Material::Acrylic.window_background();
            cx.open_window(opts, |window, cx| {
                cx.new(|cx| {
                    let mut view = QuickAddView::new(
                        model::my_day_progress(&fixture.state, fixture.now.today()),
                        theme,
                        Some(QUICK_ADD_TYPED),
                        true,
                        window,
                        cx,
                    );
                    view.set_material(Material::Acrylic, window, cx);
                    view
                })
            })
            .map(Into::into)
        } else if let Some(waiting) = pairing {
            cx.open_window(main_options(), |window, cx| {
                cx.new(|cx| {
                    let mut view = PairingView::fixture(
                        theme,
                        fixtures::PAIRING_SERVER,
                        fixtures::PAIRING_CODE,
                        waiting,
                        window,
                        cx,
                    );
                    view.set_material(Material::Acrylic, cx);
                    view
                })
            })
            .map(Into::into)
        } else if let Some(confirming) = failed {
            cx.open_window(main_options(), |_, cx| {
                cx.new(|cx| {
                    let mut view = FailedView::fixture(theme, confirming, cx);
                    view.set_material(Material::Acrylic, cx);
                    view
                })
            })
            .map(Into::into)
        } else {
            let store = Rc::new(Store::fixture(&fixture));
            cx.open_window(main_options(), |window, cx| {
                let view = cx.new(|cx| {
                    let mut shell = ShellView::new(
                        store,
                        fixture.ui.clone(),
                        theme,
                        fixture.quick_add_text,
                        window,
                        cx,
                    );
                    shell.set_material(Material::Acrylic, cx);
                    shell.preview_caption(caption);
                    if fixture.ui.compact {
                        shell.preview_popout(opts.fixture == "popout-pinned", cx);
                    }
                    shell.preview_splitter_hover(opts.fixture == "resize-handle-hover");
                    shell.preview_email_drop(opts.fixture == "email-drop");
                    shell.still();
                    if let Some(moment) = fixture.moment {
                        shell.freeze(moment, window, cx);
                    }
                    shell.preview_menu(&opts.fixture, window, cx);
                    if let Some(state) = update_fixture(&opts.fixture, fixture.now.at) {
                        shell.set_updates(updates::Controller::fixture(state), cx);
                        if opts.fixture != "updates-indicator"
                            && !opts.fixture.starts_with("updates-footer-")
                        {
                            shell.open_updates(window, cx);
                        }
                    }
                    shell.preview_assistant(&opts.fixture, window, cx);
                    shell
                });
                // GPUI's own focus, inside the window: nothing is activated or shown.
                let focus = view.read(cx).fixture_focus(cx);
                window.focus(&focus, cx);
                view
            })
            .map(Into::into)
        }
    })?;
    let hwnd = handle.update(cx, |_, window, _| win::hwnd_of(window))?;
    anyhow::ensure!(hwnd != 0, "no native window handle");
    if fixture.ui.compact {
        // Test both transitions on a real, never-shown normal HWND. The pixels alone can't
        // tell us whether Win32 honored the pin state.
        let pinned = opts.fixture == "popout-pinned";
        for state in [!pinned, pinned] {
            win::set_topmost(hwnd, state)?;
            anyhow::ensure!(
                win::is_topmost(hwnd) == state,
                "list window pin state did not change"
            );
        }
    }
    let pointer = match opts.fixture.as_str() {
        "ai-button-hover" => Some(("assistant-settings", false)),
        "detail-button-pressed" => Some(("close-detail", true)),
        _ => None,
    };
    let mut image = capture(cx, handle, hwnd, opened_at, pointer).await?;
    handle.update(cx, |_, window, _| window.remove_window())?;
    let got = image.dimensions();
    // A window the system clamped (to a small screen, say) must not pass as the real thing.
    anyhow::ensure!(got == expected, "captured {got:?}, expected {expected:?}");
    composite_acrylic_backdrop(&mut image, theme);
    save(&image, &opts.out, opts.overwrite)?;
    println!("wrote {} ({}x{})", opts.out.display(), got.0, got.1);
    Ok(())
}

/// Composites translucent pixels read back from a hidden acrylic window over a calm simulated
/// Windows 11 DWM system acrylic backdrop so gallery PNGs reflect on-screen depth and contrast
/// regardless of the image viewer's background.
fn composite_acrylic_backdrop(image: &mut image::RgbaImage, theme: Theme) {
    let (w, h) = image.dimensions();
    let max_x = (w.saturating_sub(1)).max(1) as f32;
    let max_y = (h.saturating_sub(1)).max(1) as f32;
    // Soft Windows 11-inspired blurred acrylic backdrop corners (top-left, top-right, bottom-left,
    // bottom-right), staying inside DWM's light/dark acrylic luminosity range.
    let (tl, tr, bl, br): ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) = match theme {
        Theme::Light => (
            [219., 229., 242.],
            [240., 228., 220.],
            [228., 224., 238.],
            [216., 232., 236.],
        ),
        Theme::Dark => (
            [32., 42., 56.],
            [48., 38., 44.],
            [40., 36., 52.],
            [28., 44., 48.],
        ),
    };
    for (x, y, px) in image.enumerate_pixels_mut() {
        let a = f32::from(px[3]) / 255.;
        if a >= 1. {
            continue;
        }
        let u = x as f32 / max_x;
        let v = y as f32 / max_y;
        let inv_a = 1. - a;
        for c in 0..3 {
            let top = tl[c] + (tr[c] - tl[c]) * u;
            let bot = bl[c] + (br[c] - bl[c]) * u;
            let bg = top + (bot - top) * v;
            px[c] = (f32::from(px[c]) * a + bg * inv_a).round().clamp(0., 255.) as u8;
        }
        px[3] = 255;
    }
}

fn update_fixture(name: &str, now: todo_core::time::Timestamp) -> Option<UpdateState> {
    use todo_core::time::Timestamp;
    let earlier =
        |minutes: i64| Timestamp::from_second(now.as_second() - 60 * minutes).unwrap_or(now);
    Some(match name {
        "updates-disabled" => UpdateState::Disabled,
        "updates-up-to-date" | "updates-footer-current" => UpdateState::UpToDate {
            checked_at: earlier(5),
        },
        "updates-checking" | "updates-footer-checking" => UpdateState::Checking { manual: true },
        "updates-downloading" | "updates-footer-downloading" => {
            UpdateState::Downloading { progress: Some(42) }
        }
        "updates-ready" | "updates-indicator" => UpdateState::Ready {
            version: "0.1.0-build.147".into(),
        },
        "updates-footer-skipped" => UpdateState::Skipped { build: 147 },
        "updates-unverified" => UpdateState::Failed {
            kind: updates::Failure::Verify,
            at: earlier(2),
        },
        "updates-failed" | "updates-footer-failed" => UpdateState::Failed {
            kind: updates::Failure::Check,
            at: earlier(2),
        },
        _ => return None,
    })
}

/// Check the actual HWND before overriding the renderer's scale. The old capture-only
/// sizing path filled in the requested bounds and masked a bug in native startup.
async fn check_native_quick_add(cx: &mut AsyncApp) -> anyhow::Result<()> {
    // Pick a destination without consulting the live pointer. Start away from its anchor so
    // this checks a real native move even on a single monitor at a fixed DPI.
    let target = win::primary_popup_target()?;
    let expected = win::popup_rect(
        target,
        quick_add::SIZE,
        desktop::QUICK_ADD_POPUP_TOP_FRACTION,
    );
    let start_x = if expected.left == 0 { 8. } else { 0. };
    let handle = cx.update(|cx| {
        let mut opts = quick_add::window_options(cx);
        if let Some(WindowBounds::Windowed(bounds)) = &mut opts.window_bounds {
            bounds.origin = point(px(start_x), px(0.));
        }
        cx.open_window(opts, |_, cx| cx.new(|_| gpui::Empty))
    })?;
    let hwnd = handle.update(cx, |_, window, _| win::hwnd_of(window))?;
    let result = (|| {
        let before = win::window_rect(hwnd)?;
        let got = (
            (before.right - before.left) as u32,
            (before.bottom - before.top) as u32,
        );
        let startup_size = expected_size(window_size("quick-add"), target.scale());
        anyhow::ensure!(
            got == startup_size,
            "hidden quick-add HWND starts at size {got:?}, expected {startup_size:?}"
        );
        anyhow::ensure!(!win::is_shown(hwnd), "quick-add HWND starts visible");
        anyhow::ensure!(
            (before.left, before.top) != (expected.left, expected.top),
            "native quick-add probe must start away from the destination anchor"
        );
        quick_add::position(hwnd, target)?;
        let after = win::window_rect(hwnd)?;
        let got = (after.left, after.top, after.right, after.bottom);
        let want = (expected.left, expected.top, expected.right, expected.bottom);
        anyhow::ensure!(
            got == want,
            "hidden quick-add HWND moved to {got:?}, expected {want:?} at {} DPI",
            target.dpi
        );
        anyhow::ensure!(!win::is_shown(hwnd), "quick-add HWND became visible");
        Ok(())
    })();
    // Let the backend deliver any display-change callbacks queued by the move before removing the window.
    cx.foreground_executor().spawn(async {}).await;
    handle.update(cx, |_, window, _| window.remove_window())?;
    result
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Waits for the window to settle (a few presented frames, so fonts and icons are rasterized), then asks
/// the renderer for a readback of the next frame.
async fn capture(
    cx: &mut AsyncApp,
    handle: AnyWindowHandle,
    hwnd: isize,
    opened_at: Instant,
    pointer: Option<(&str, bool)>,
) -> anyhow::Result<image::RgbaImage> {
    let deadline = Instant::now() + Duration::from_secs(20);
    // The OS can recycle HWNDs within a batch. Don't count a previous fixture's present.
    while todo_patch::last_present(hwnd).is_none_or(|t| t < opened_at) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "the window never presented a frame"
        );
        win::request_hidden_frame(hwnd)?;
        sleep(cx, 10).await;
    }
    // Vsync does not repaint hidden windows. Give layout and next-frame callbacks time
    // to settle by pumping frames, rather than just waiting for time to pass.
    for _ in 0..5 {
        handle.update(cx, |_, window, _| window.refresh())?;
        win::request_hidden_frame(hwnd)?;
        sleep(cx, 16).await;
    }
    if let Some((control, pressed)) = pointer {
        handle.update(cx, |view, window, cx| {
            let position = view
                .downcast::<ShellView>()
                .ok()
                .and_then(|view| view.read(cx).control_center(control))
                .with_context(|| format!("no painted target for {control}"))?;
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    modifiers: Default::default(),
                    pressed_button: None,
                }),
                cx,
            );
            if pressed {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            }
            anyhow::Ok(())
        })??;
    }
    todo_patch::request_capture(hwnd);
    loop {
        if let Some(image) = todo_patch::take_capture(hwnd) {
            return image;
        }
        // Ask GPUI for a fresh frame; the renderer reads it back before presenting it.
        handle.update(cx, |_, window, _| window.refresh())?;
        win::request_hidden_frame(hwnd)?;
        anyhow::ensure!(Instant::now() < deadline, "no frame was captured");
        sleep(cx, 16).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_fixture_window_does_not_request_show_or_focus() {
        let opts = fixture_window_options(MAIN_SIZE.0, MAIN_SIZE.1);
        assert!(!opts.show && !opts.focus);
        assert_eq!(
            opts.window_background,
            Material::Acrylic.window_background()
        );
    }

    #[test]
    fn acrylic_backdrop_preserves_opaque_pixels_and_flattens_translucent_ones() {
        let mut img = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([40, 80, 120, 255])
            } else {
                image::Rgba([247, 241, 236, 168])
            }
        });
        composite_acrylic_backdrop(&mut img, Theme::Light);
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([40, 80, 120, 255]));
        let blended = img.get_pixel(1, 0);
        assert_eq!(blended[3], 255);
        assert!(blended[0] > 230 && blended[1] > 225 && blended[2] > 220);
    }

    #[test]
    fn expected_sizes_follow_the_scale() {
        assert_eq!(expected_size(MAIN_SIZE, 1.0), (1440, 900));
        assert_eq!(expected_size(MAIN_SIZE, 1.5), (2160, 1350));
        assert_eq!(expected_size((640., 196.), 1.25), (800, 245));
        assert_eq!(window_size("my-day"), (1440., 900.));
        assert_eq!(window_size("quick-add"), (640., 196.));
        assert_eq!(window_size("long-names-narrow"), (960., 900.));
        assert_eq!(window_size("narrow-detail"), (960., 900.));
        assert_eq!(window_size("narrow-undo-toast"), (960., 900.));
        assert_eq!(window_size("finished-short"), (1440., 700.));
        assert_eq!(window_size("detail-wide"), (1440., 900.));
        assert_eq!(window_size("detail-wide-1920"), (1920., 1080.));
        assert_eq!(window_size("short-sidebar"), (960., 600.));
    }

    #[test]
    fn save_refuses_to_overwrite_unless_asked() {
        let dir = std::env::temp_dir().join(format!("todo-shot-{}", std::process::id()));
        let out = dir.join("x.png");
        let _ = std::fs::remove_file(&out);
        let img = image::RgbaImage::new(2, 2);
        save(&img, &out, false).expect("first write");
        assert!(save(&img, &out, false).is_err());
        save(&img, &out, true).expect("overwrite");
        // The file is complete when save returns: it ends with IEND and decodes.
        let bytes = std::fs::read(&out).expect("read back");
        assert!(bytes.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
        let back = image::load_from_memory(&bytes).expect("decodes");
        assert_eq!((back.width(), back.height()), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
