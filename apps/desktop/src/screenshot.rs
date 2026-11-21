//! `--screenshot`: render a fixture into its own window, save that window to a PNG and exit.
//!
//! Deterministic and input-free: the fixture data and the clock are fixed, the scale factor is forced, and
//! the image is read back from the window's own render target (`todo_patch::request_capture`) instead of
//! capturing the screen. Nothing is clicked or typed.

use std::rc::Rc;
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
use crate::model;
use crate::platform::win;
use crate::store::Store;
use crate::theme::desktop;
use crate::ui::pairing::PairingView;
use crate::ui::quick_add::{self, QuickAddView};
use crate::ui::shell::{self, ShellView};
use crate::{fixtures, fonts, ui};

/// The main window's size in the fixtures: the design's reference window, 1440 × 900 (the three panes side by
/// side).
const MAIN_SIZE: (f32, f32) = (desktop::SIDEBAR + desktop::LIST + desktop::DETAIL, 900.);

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

/// Renders the fixture and writes the PNG. Returns the process exit code.
pub fn run(opts: ScreenshotOptions) -> anyhow::Result<i32> {
    let fixture = fixtures::by_name(&opts.fixture)
        .with_context(|| format!("unknown fixture {}", opts.fixture))?;
    if !opts.overwrite && opts.out.exists() {
        eprintln!(
            "error: {} exists; pass --overwrite to replace it",
            opts.out.display()
        );
        return Ok(EXIT_EXISTS);
    }
    todo_patch::set_scale_factor_override(Some(opts.scale));
    let code = Arc::new(AtomicI32::new(1));
    let code_in = code.clone();

    application().with_assets(Assets).run(move |cx: &mut App| {
        let missing = fonts::load(cx);
        if !missing.is_empty() {
            eprintln!("error: fonts missing: {missing:?}");
            code_in.store(EXIT_FONTS, Ordering::SeqCst);
            cx.quit();
            return;
        }
        cx.bind_keys(ui::input::key_bindings());
        cx.bind_keys(ui::text_area::key_bindings());
        cx.bind_keys(shell::key_bindings());
        let theme = opts.theme;
        let quick = opts.fixture == "quick-add";
        // The pairing screen, before (`pairing`) and after (`pairing-code`) asking for a code.
        let pairing = match opts.fixture.as_str() {
            "pairing" => Some(false),
            "pairing-code" => Some(true),
            _ => None,
        };
        let logical = if quick {
            (
                f32::from(quick_add::SIZE.width),
                f32::from(quick_add::SIZE.height),
            )
        } else {
            MAIN_SIZE
        };
        let expected = expected_size(logical, opts.scale);
        let opened = if quick {
            let (w, h) = logical;
            cx.open_window(options(w, h, true), |window, cx| {
                cx.new(|cx| {
                    QuickAddView::new(
                        model::my_day_progress(&fixture.state, &fixture.now),
                        theme,
                        Some(QUICK_ADD_TYPED),
                        true,
                        window,
                        cx,
                    )
                })
            })
            .map(|w| {
                w.update(cx, |_, window, _| win::hwnd_of(window))
                    .unwrap_or_default()
            })
        } else if let Some(waiting) = pairing {
            cx.open_window(options(MAIN_SIZE.0, MAIN_SIZE.1, false), |window, cx| {
                cx.new(|cx| {
                    PairingView::fixture(
                        theme,
                        fixtures::PAIRING_SERVER,
                        fixtures::PAIRING_CODE,
                        waiting,
                        window,
                        cx,
                    )
                })
            })
            .map(|w| {
                w.update(cx, |_, window, _| win::hwnd_of(window))
                    .unwrap_or_default()
            })
        } else {
            let store = Rc::new(Store::fixture(&fixture));
            cx.open_window(options(MAIN_SIZE.0, MAIN_SIZE.1, false), |window, cx| {
                cx.new(|cx| {
                    ShellView::new(
                        store,
                        fixture.ui.clone(),
                        theme,
                        fixture.quick_add_text,
                        window,
                        cx,
                    )
                })
            })
            .map(|w| {
                w.update(cx, |_, window, _| win::hwnd_of(window))
                    .unwrap_or_default()
            })
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
        let overwrite = opts.overwrite;
        let code = code_in.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = capture(cx, hwnd).await.and_then(|image| {
                let got = (image.width(), image.height());
                // A window the system clamped (to a small screen, say) must not pass as the real thing.
                anyhow::ensure!(
                    got == expected,
                    "captured {}x{}, expected {}x{}",
                    got.0,
                    got.1,
                    expected.0,
                    expected.1
                );
                save(&image, &out, overwrite)?;
                Ok(got)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_sizes_follow_the_scale() {
        assert_eq!(expected_size(MAIN_SIZE, 1.0), (1440, 900));
        assert_eq!(expected_size(MAIN_SIZE, 1.5), (2160, 1350));
        assert_eq!(expected_size((640., 196.), 1.25), (800, 245));
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
