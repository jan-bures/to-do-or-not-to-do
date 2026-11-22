//! Small Win32 helpers: window visibility and focus, the console, process timing. No input injection.

#![allow(unsafe_code)] // Plain Win32 calls on window handles we own; each is commented.

use gpui::{Pixels, Size, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{FILETIME, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetCurrentThread, GetProcessTimes,
    SetPriorityClass, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible, PostMessageW,
    SPI_GETCLIENTAREAANIMATION, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow, SystemParametersInfoW, USER_DEFAULT_SCREEN_DPI,
    WM_PAINT,
};

/// The native handle of a GPUI window, as an integer (0 if unavailable).
pub fn hwnd_of(window: &Window) -> isize {
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// Lower priority before GPUI starts any worker threads (also for direct CLI captures).
pub fn lower_process_priority() -> std::io::Result<()> {
    // SAFETY: the pseudo-handle refers to this process, not the owner's interactive instance.
    if unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Lower CPU and I/O priority on a dedicated worker for the rest of its lifetime.
pub fn background_thread() -> std::io::Result<()> {
    // SAFETY: background mode can only be set on the calling thread's own pseudo-handle.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Hidden windows need not receive WM_PAINT from vsync. Post (never send) one so the
/// normal GPUI frame callback runs after the app borrow is released, with no user input.
pub fn request_hidden_frame(hwnd: isize) -> anyhow::Result<()> {
    // SAFETY: the caller keeps the window alive; WM_PAINT has no pointer parameters.
    unsafe {
        anyhow::ensure!(
            IsWindowVisible(hwnd as HWND) == 0,
            "capture window is visible"
        );
        if PostMessageW(hwnd as HWND, WM_PAINT, 0, 0) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

/// Physical screen bounds of a window, including its frame if it has one.
pub fn window_rect(hwnd: isize) -> std::io::Result<RECT> {
    // SAFETY: the out-pointer is valid and a stale HWND fails harmlessly.
    let mut rect = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd as HWND, &mut rect) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(rect)
}

/// The work area in physical screen pixels and the effective DPI of a popup's destination.
#[allow(missing_debug_implementations)] // The Win32 RECT has no Debug implementation.
#[derive(Clone, Copy)]
pub struct PopupTarget {
    pub work_area: RECT,
    pub dpi: u32,
}

impl PopupTarget {
    pub fn scale(self) -> f32 {
        self.dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32
    }
}

/// Centre horizontally and anchor from the top of the work area, shifting or shrinking as needed.
pub fn popup_rect(target: PopupTarget, size: Size<Pixels>, top_fraction: f32) -> RECT {
    let work = target.work_area;
    let size = size.to_device_pixels(target.scale());
    let work_width = work.right - work.left;
    let work_height = work.bottom - work.top;
    let width = size.width.0.min(work_width);
    let height = size.height.0.min(work_height);
    // The centred x offset always fits after capping the width; the y anchor may need moving up.
    let left = work.left + (work_width - width) / 2;
    let top = work.top
        + ((work_height as f32 * top_fraction).round() as i32).clamp(0, work_height - height);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// The cursor's monitor, falling back to the foreground window's monitor, then the primary one.
pub fn cursor_popup_target() -> anyhow::Result<PopupTarget> {
    // SAFETY: the monitor queries only read desktop state and write to initialized locals.
    let monitor = unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        let mut monitor = if GetCursorPos(&mut cursor) != 0 {
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONULL)
        } else {
            std::ptr::null_mut()
        };
        if monitor.is_null() {
            monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONULL);
        }
        monitor
    };
    if monitor.is_null() {
        primary_popup_target()
    } else {
        popup_target(monitor)
    }
}

/// The primary monitor, independently of the cursor or foreground window.
pub fn primary_popup_target() -> anyhow::Result<PopupTarget> {
    // SAFETY: point (0, 0) is on the primary monitor; the query has no side effects.
    popup_target(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
}

fn popup_target(monitor: HMONITOR) -> anyhow::Result<PopupTarget> {
    // SAFETY: these monitor queries read desktop state and write to initialized locals.
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let (mut x, mut y) = (0, 0);
        windows::core::HRESULT(GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y))
            .ok()?;
        Ok(PopupTarget {
            work_area: info.rcWork,
            dpi: x,
        })
    }
}

/// Move and size a frameless popup without showing it or changing the foreground.
pub fn position_popup(hwnd: isize, rect: RECT) -> anyhow::Result<()> {
    let h = hwnd as HWND;
    let place = || {
        // SAFETY: we own the HWND. No pointer parameters, activation, or changes to Z order.
        if unsafe {
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    // SAFETY: GetDpiForWindow only queries the HWND, which we own.
    let old_dpi = unsafe { GetDpiForWindow(h) };
    place()?;
    // WM_DPICHANGED may use Windows' suggested origin during the move. Once on the
    // destination DPI, reapply the exact anchor instead of keeping the suggested one.
    if unsafe { GetDpiForWindow(h) } != old_dpi {
        place()?;
    }
    Ok(())
}

/// Shows, restores and focuses a window.
pub fn show_and_focus(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let h = hwnd as HWND;
    // SAFETY: `h` is a window handle owned by this process; these calls tolerate stale handles.
    unsafe {
        ShowWindow(h, SW_SHOW);
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}

/// Hides a window (it stays alive; the tray brings it back).
pub fn hide(hwnd: isize) {
    if hwnd != 0 {
        // SAFETY: see `show_and_focus`.
        unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
    }
}

/// Whether a window is visible and not minimized.
pub fn is_shown(hwnd: isize) -> bool {
    // SAFETY: see `show_and_focus`.
    hwnd != 0 && unsafe { IsWindowVisible(hwnd as HWND) != 0 && IsIconic(hwnd as HWND) == 0 }
}

/// Whether the owner asked for reduced motion: Settings > Accessibility > Visual effects > Animation
/// effects turned off (`SPI_GETCLIENTAREAANIMATION`). Cheap, so it's read each time an animation starts,
/// which also picks up a change made while the app runs.
pub fn reduced_motion() -> bool {
    let mut animate: i32 = 1;
    // SAFETY: the out-pointer references a live BOOL, as SPI_GETCLIENTAREAANIMATION expects.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animate).cast(), 0)
    };
    ok != 0 && animate == 0
}

/// App builds are GUI-subsystem executables. When started from a terminal, write `--help`, errors
/// and benchmark output there.
pub fn attach_parent_console() {
    // SAFETY: plain FFI call; fails harmlessly when there is no parent console.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn filetime_100ns(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

/// Milliseconds since the OS created this process, which is what "cold start" is measured from.
pub fn process_age_ms() -> f64 {
    // SAFETY: the out-pointers reference live, properly sized locals.
    unsafe {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return f64::NAN;
        }
        let mut now = zero;
        GetSystemTimePreciseAsFileTime(&mut now);
        filetime_100ns(now).saturating_sub(filetime_100ns(created)) as f64 / 10_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(work: (i32, i32, i32, i32), dpi: u32) -> (i32, i32, i32, i32) {
        let (left, top, right, bottom) = work;
        let target = PopupTarget {
            work_area: RECT {
                left,
                top,
                right,
                bottom,
            },
            dpi,
        };
        let size = gpui::size(gpui::px(640.), gpui::px(196.));
        let r = popup_rect(target, size, 0.2);
        (r.left, r.top, r.right, r.bottom)
    }

    #[test]
    fn popup_centers_on_a_negative_origin_monitor_at_each_dpi() {
        for (dpi, width, height) in [(96, 640, 196), (120, 800, 245), (144, 960, 294)] {
            let left = -960 - width / 2;
            assert_eq!(
                place((-1920, -1080, 0, 0), dpi),
                (left, -864, left + width, -864 + height)
            );
        }
    }

    #[test]
    fn popup_caps_width_on_a_narrow_high_dpi_monitor() {
        assert_eq!(place((0, 0, 1080, 1920), 192), (0, 384, 1080, 776));
    }

    #[test]
    fn popup_anchors_inside_the_work_area_with_a_left_or_top_taskbar() {
        assert_eq!(place((64, 0, 1920, 1080), 96), (672, 216, 1312, 412));
        assert_eq!(place((0, 64, 1920, 1080), 96), (640, 267, 1280, 463));
    }

    #[test]
    fn popup_clamps_the_anchor_and_caps_height_in_a_short_work_area() {
        assert_eq!(place((0, 64, 1920, 288), 96), (640, 92, 1280, 288));
        assert_eq!(place((0, 64, 1920, 288), 192), (320, 64, 1600, 288));
    }
}
