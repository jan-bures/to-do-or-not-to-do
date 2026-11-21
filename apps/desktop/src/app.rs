//! The running app: the main window, the hidden quick-add window, the tray, the hotkey, single instance and
//! Start with Windows. Also the `--bench` modes, which run exactly this code and time it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, AppContext, AsyncApp, Bounds, TitlebarOptions, WindowAppearance, WindowBounds,
    WindowHandle, WindowOptions, px, size,
};
use gpui_platform::application;
use gpui_windows::todo_patch;

use crate::args::{BenchKind, BenchOptions, RunOptions};
use crate::assets::Assets;
use crate::fixtures;
use crate::metrics::{Metrics, summarize};
use crate::platform::autostart::Autostart;
use crate::platform::single_instance::{self, Instance};
use crate::platform::tray::{self, TrayAction};
use crate::platform::{hotkey, win};
use crate::theme::Theme;
use crate::ui::quick_add::{self, QuickAddEvent, QuickAddView};
use crate::ui::shell::{self, ShellView};
use crate::{fonts, ui};

/// Things that happen outside GPUI (tray, hotkey, a second launch) and are handled on the UI thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellEvent {
    /// A second copy was launched: show the main window.
    Activate,
    Tray(TrayAction),
    /// The hotkey already showed the quick-add window; focus its input.
    QuickAddShown,
}

/// Native handles of the two windows.
#[derive(Clone, Copy, Debug)]
struct Hwnds {
    main: isize,
    quick: isize,
}

/// Shows the quick-add window straight from the hotkey handler, before GPUI's next frame, and tells the UI
/// thread to focus its input.
fn on_hotkey(quick_hwnd: isize, tx: &UnboundedSender<ShellEvent>) {
    win::show_and_focus(quick_hwnd);
    let _ = tx.unbounded_send(ShellEvent::QuickAddShown);
}

fn theme_for(appearance: WindowAppearance) -> Theme {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::Light,
    }
}

fn main_window_options(cx: &App, show: bool) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("To-do".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        focus: show,
        show,
        window_min_size: Some(size(px(960.), px(600.))),
        app_id: Some("to-do-or-not-to-do".into()),
        ..WindowOptions::default()
    }
}

/// Today's eyebrow from the local clock.
fn today() -> (String, String, String) {
    let (y, m, d) = win::local_date();
    fixtures::eyebrow_for(y, m, d)
}

/// Runs the app. Returns once the user quits (or immediately, when another copy is already running).
pub fn run(opts: RunOptions, bench: Option<BenchOptions>) -> anyhow::Result<()> {
    let tag = if bench.is_some() {
        format!("bench-{}", std::process::id())
    } else {
        single_instance::APP_TAG.to_string()
    };
    let primary = match single_instance::acquire(&tag)? {
        Instance::Primary(p) => p,
        Instance::Secondary => {
            tracing::info!("another copy is running; asked it to show itself");
            return Ok(());
        }
    };
    let mut metrics = Metrics::default();
    if bench.is_some() {
        metrics.ms("main_entered_ms", win::process_age_ms());
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("todo-desktop.exe"));
    let (tx, mut rx) = unbounded::<ShellEvent>();
    let background = opts.background || bench.as_ref().is_some_and(|b| b.kind == BenchKind::Hotkey);

    application().with_assets(Assets).run(move |cx: &mut App| {
        if bench.is_some() {
            metrics.ms("gpui_ready_ms", win::process_age_ms());
        }
        let missing = fonts::load(cx, opts.fonts_dir.as_deref());
        if !missing.is_empty() {
            tracing::warn!("fonts missing: {:?}", missing);
        }
        cx.bind_keys(ui::input::key_bindings());
        cx.bind_keys(shell::key_bindings());
        let theme = theme_for(cx.window_appearance());
        let (weekday, date, week) = today();
        let startup = fixtures::startup(weekday, date, week);

        let main = match cx.open_window(main_window_options(cx, !background), |window, cx| {
            cx.new(|cx| ShellView::new(startup.clone(), theme, window, cx))
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("opening the main window failed: {e:#}");
                cx.quit();
                return;
            }
        };
        let quick = match cx.open_window(quick_add::window_options(cx, false), |window, cx| {
            cx.new(|cx| QuickAddView::new(&startup.room, theme, None, window, cx))
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("opening the quick-add window failed: {e:#}");
                cx.quit();
                return;
            }
        };
        let hwnds = Hwnds {
            main: main
                .update(cx, |_, window, _| win::hwnd_of(window))
                .unwrap_or_default(),
            quick: quick
                .update(cx, |_, window, _| win::hwnd_of(window))
                .unwrap_or_default(),
        };
        wire_windows(main, quick, hwnds, cx);

        // Tray, hotkey and second launches all feed one channel, drained on the UI thread.
        let autostart_enabled = Autostart::current_user().is_enabled(&exe);
        let tray_tx = tx.clone();
        let tray = tray::create(autostart_enabled, move |action| {
            let _ = tray_tx.unbounded_send(ShellEvent::Tray(action));
        })
        .map_err(|e| tracing::error!("tray icon failed: {e:#}"))
        .ok();
        let hotkey_tx = tx.clone();
        let hotkey = hotkey::register(move || on_hotkey(hwnds.quick, &hotkey_tx))
            .map_err(|e| tracing::warn!("Ctrl+Alt+Space is unavailable: {e:#}"))
            .ok();
        let activate_tx = tx.clone();
        primary.listen(move || {
            let _ = activate_tx.unbounded_send(ShellEvent::Activate);
        });

        let exe_for_events = exe.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            // The tray and the hotkey registration live as long as this loop.
            let _hotkey = hotkey;
            while let Some(event) = rx.next().await {
                let quit = cx.update(|cx| {
                    handle_event(
                        event,
                        hwnds,
                        main,
                        quick,
                        tray.as_ref(),
                        &exe_for_events,
                        cx,
                    )
                });
                if quit {
                    break;
                }
            }
        })
        .detach();

        if let Some(bench) = bench {
            let tx = tx.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                run_bench(bench, metrics, hwnds, tx, cx).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
    Ok(())
}

/// Close hides to the tray; the quick-add window reports what was typed and hides when it loses focus.
fn wire_windows(
    main: WindowHandle<ShellView>,
    quick: WindowHandle<QuickAddView>,
    hwnds: Hwnds,
    cx: &mut App,
) {
    let _ = main.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, move |_, _| {
            win::hide(hwnds.main);
            false
        });
    });
    let _ = quick.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, move |_, _| {
            win::hide(hwnds.quick);
            false
        });
        let view = cx.entity();
        cx.subscribe(&view, move |_, _, event: &QuickAddEvent, cx| {
            if let QuickAddEvent::Submitted(title) = event {
                let title = title.clone();
                let _ = main.update(cx, move |shell, _, cx| {
                    shell.add_task(title);
                    cx.notify();
                });
            }
            win::hide(hwnds.quick);
        })
        .detach();
        cx.observe_window_activation(window, move |_, window, _| {
            if !window.is_window_active() {
                win::hide(hwnds.quick);
            }
        })
        .detach();
    });
}

/// Returns whether the app should quit.
fn handle_event(
    event: ShellEvent,
    hwnds: Hwnds,
    main: WindowHandle<ShellView>,
    quick: WindowHandle<QuickAddView>,
    tray: Option<&tray::Tray>,
    exe: &std::path::Path,
    cx: &mut App,
) -> bool {
    match event {
        ShellEvent::Activate => win::show_and_focus(hwnds.main),
        ShellEvent::Tray(TrayAction::ToggleWindow) => {
            if win::is_shown(hwnds.main) {
                win::hide(hwnds.main);
            } else {
                win::show_and_focus(hwnds.main);
            }
        }
        ShellEvent::Tray(TrayAction::QuickAdd) => {
            win::show_and_focus(hwnds.quick);
            focus_quick_add(quick, cx);
        }
        ShellEvent::QuickAddShown => focus_quick_add(quick, cx),
        ShellEvent::Tray(TrayAction::ToggleAutostart) => {
            let autostart = Autostart::current_user();
            let enable = !autostart.is_enabled(exe);
            if let Err(e) = autostart.set_enabled(exe, enable) {
                tracing::error!("changing Start with Windows failed: {e}");
            }
            if let Some(tray) = tray {
                tray.set_autostart_checked(autostart.is_enabled(exe));
            }
        }
        ShellEvent::Tray(TrayAction::Quit) => {
            let _ = main.update(cx, |_, window, _| window.remove_window());
            cx.quit();
            return true;
        }
    }
    false
}

fn focus_quick_add(quick: WindowHandle<QuickAddView>, cx: &mut App) {
    let _ = quick.update(cx, |view, window, cx| {
        let handle = view.input().read(cx).focus_handle.clone();
        window.focus(&handle, cx);
        window.refresh();
    });
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Waits until `hwnd` presents a frame after `since`, up to `timeout`.
async fn wait_for_present(
    cx: &AsyncApp,
    hwnd: isize,
    since: Instant,
    timeout: Duration,
) -> Option<Instant> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(t) = todo_patch::last_present(hwnd).filter(|t| *t > since) {
            return Some(t);
        }
        if Instant::now() > deadline {
            return None;
        }
        sleep(cx, 1).await;
    }
}

async fn run_bench(
    opts: BenchOptions,
    mut metrics: Metrics,
    hwnds: Hwnds,
    tx: UnboundedSender<ShellEvent>,
    cx: &mut AsyncApp,
) {
    match opts.kind {
        BenchKind::Startup => {
            // Cold start: process creation to the main window's first presented frame.
            let started = Instant::now();
            let mut first = None;
            while first.is_none() && started.elapsed() < Duration::from_secs(10) {
                first = todo_patch::first_present(hwnds.main);
                sleep(cx, 5).await;
            }
            match first {
                Some(at) => {
                    let age_now = win::process_age_ms();
                    metrics.ms(
                        "cold_start_first_frame_presented_ms",
                        age_now - at.elapsed().as_secs_f64() * 1000.0,
                    );
                }
                None => metrics.log("cold_start_first_frame_presented_ms", "timed out"),
            }
        }
        BenchKind::Hotkey => {
            // Let startup settle with both windows hidden, like an app that has been sitting in the tray.
            sleep(cx, 1500).await;
            let mut samples = Vec::new();
            for _ in 0..20 {
                let t0 = Instant::now();
                // Exactly what the WM_HOTKEY handler does.
                on_hotkey(hwnds.quick, &tx);
                match wait_for_present(cx, hwnds.quick, t0, Duration::from_secs(2)).await {
                    Some(at) => samples.push(at.duration_since(t0).as_secs_f64() * 1000.0),
                    None => metrics.log("hotkey_sample", "no frame within 2 s"),
                }
                sleep(cx, 150).await;
                win::hide(hwnds.quick);
                sleep(cx, 350).await;
            }
            metrics.log(
                "hotkey_handler_to_quick_add_presented_ms",
                summarize(&samples),
            );
            metrics.log(
                "hotkey_note",
                "measured from the WM_HOTKEY handler; DWM shows the frame at the next vblank",
            );
        }
    }
    if let Err(e) = metrics.write(&opts.metrics) {
        tracing::error!("writing {} failed: {e}", opts.metrics.display());
    }
}
