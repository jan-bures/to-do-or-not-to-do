//! `--screenshot`: render a fixture into its own window, save that window to a PNG and exit.
//!
//! Deterministic and input-free: the fixture data and the clock are fixed, the scale factor is forced, and
//! the image is read back from the window's own render target (`todo_patch::request_capture`) instead of
//! capturing the screen. Nothing is clicked or typed.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::{
    App, AppContext, AsyncApp, Bounds, TitlebarOptions, WindowBounds, WindowKind, WindowOptions,
    point, px, size,
};
use gpui_platform::application;
use gpui_windows::todo_patch;

use crate::args::ScreenshotOptions;
use crate::assets::Assets;
use crate::platform::win;
use crate::ui::quick_add::{self, QuickAddView};
use crate::ui::shell::{self, ShellView};
use crate::{fixtures, fonts, ui};

/// The main window's size in the fixtures: the mockups' 1440 × 900.
const MAIN_SIZE: (f32, f32) = (1440., 900.);

/// Typed into the quick-add fixture, to show its focused state.
const QUICK_ADD_TYPED: &str = "Call the venue about Friday's room";

fn options(width: f32, height: f32, quick: bool) -> WindowOptions {
    WindowOptions {
        // Top-left of the primary display; the window may be larger than the screen at 150 %.
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(width), px(height)),
        ))),
        titlebar: if quick {
            None
        } else {
            Some(TitlebarOptions {
                title: Some("To-do (screenshot)".into()),
                appears_transparent: true,
                ..Default::default()
            })
        },
        focus: false,
        show: true,
        kind: if quick {
            WindowKind::PopUp
        } else {
            WindowKind::Normal
        },
        is_resizable: false,
        ..WindowOptions::default()
    }
}

/// Renders the fixture and writes the PNG. Returns the process exit code.
pub fn run(opts: ScreenshotOptions) -> anyhow::Result<i32> {
    let shell_data = fixtures::by_name(&opts.fixture)
        .with_context(|| format!("unknown fixture {}", opts.fixture))?;
    todo_patch::set_scale_factor_override(Some(opts.scale));
    let code = Arc::new(AtomicI32::new(1));
    let code_in = code.clone();

    application().with_assets(Assets).run(move |cx: &mut App| {
        let missing = fonts::load(cx, opts.fonts_dir.as_deref());
        if !missing.is_empty() {
            eprintln!(
                "error: fonts missing: {:?}. Run apps/desktop/scripts/fetch_fonts.py or pass --fonts-dir.",
                missing
            );
            code_in.store(3, Ordering::SeqCst);
            cx.quit();
            return;
        }
        cx.bind_keys(ui::input::key_bindings());
        cx.bind_keys(shell::key_bindings());
        let theme = opts.theme;
        let quick = opts.fixture == "quick-add";
        let opened = if quick {
            let (w, h) = (f32::from(quick_add::SIZE.width), f32::from(quick_add::SIZE.height));
            cx.open_window(options(w, h, true), |window, cx| {
                cx.new(|cx| QuickAddView::new(&shell_data.room, theme, Some(QUICK_ADD_TYPED), window, cx))
            })
            .map(|w| w.update(cx, |_, window, _| win::hwnd_of(window)).unwrap_or_default())
        } else {
            cx.open_window(options(MAIN_SIZE.0, MAIN_SIZE.1, false), |window, cx| {
                cx.new(|cx| ShellView::new(shell_data.clone(), theme, window, cx))
            })
            .map(|w| w.update(cx, |_, window, _| win::hwnd_of(window)).unwrap_or_default())
        };
        let hwnd = match opened {
            Ok(h) if h != 0 => h,
            Ok(_) => {
                eprintln!("error: no native window handle");
                cx.quit();
                return;
            }
            Err(e) => {
                eprintln!("error: opening the window failed: {e:#}");
                cx.quit();
                return;
            }
        };
        let out = opts.out.clone();
        let code = code_in.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = capture(cx, hwnd).await.and_then(|image| {
                if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)?;
                }
                image.save(&out).with_context(|| format!("writing {}", out.display()))?;
                Ok((image.width(), image.height()))
            });
            match result {
                Ok((w, h)) => {
                    println!("wrote {} ({w}x{h})", out.display());
                    code.store(0, Ordering::SeqCst);
                }
                Err(e) => eprintln!("error: {e:#}"),
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(code.load(Ordering::SeqCst))
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Waits for the window to settle (a few presented frames, so fonts and icons are rasterized), then asks
/// the renderer for a readback of the next frame.
async fn capture(cx: &mut AsyncApp, hwnd: isize) -> anyhow::Result<image::RgbaImage> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while todo_patch::first_present(hwnd).is_none() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "the window never presented a frame"
        );
        sleep(cx, 10).await;
    }
    sleep(cx, 400).await;
    todo_patch::request_capture(hwnd);
    loop {
        // Ask GPUI for a fresh frame; the renderer reads it back before presenting it.
        cx.update(|cx| {
            for window in cx.windows() {
                let _ = window.update(cx, |_, window, _| window.refresh());
            }
        });
        if let Some(image) = todo_patch::take_capture(hwnd) {
            return image;
        }
        anyhow::ensure!(Instant::now() < deadline, "no frame was captured");
        sleep(cx, 16).await;
    }
}
