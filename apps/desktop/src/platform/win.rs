//! Small Win32 helpers: window visibility and focus, the console, process timing. No input injection.

#![allow(unsafe_code)] // Plain Win32 calls on window handles we own; each is commented.

use gpui::{Pixels, Size, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{FILETIME, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
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

fn popup_rect(monitor: RECT, size: Size<Pixels>, dpi: u32, top_fraction: f32) -> RECT {
    let size = size.to_device_pixels(dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32);
    let left = monitor.left + (monitor.right - monitor.left - size.width.0) / 2;
    let top = monitor.top + ((monitor.bottom - monitor.top) as f32 * top_fraction).round() as i32;
    RECT {
        left,
        top,
        right: left + size.width.0,
        bottom: top + size.height.0,
    }
}

/// Position a frameless popup on the cursor's monitor, falling back to the foreground
/// window's monitor and then the primary one. Do not show it or change the foreground.
pub fn position_popup(hwnd: isize, size: Size<Pixels>, top_fraction: f32) -> anyhow::Result<()> {
    // SAFETY: the monitor queries only read desktop state and write to initialized locals.
    let (monitor_rect, dpi) = unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        let mut monitor = if GetCursorPos(&mut cursor) != 0 {
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONULL)
        } else {
            std::ptr::null_mut()
        };
        if monitor.is_null() {
            monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONULL);
        }
        if monitor.is_null() {
            monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let (mut x, mut y) = (0, 0);
        windows::core::HRESULT(GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y))
            .ok()?;
        (info.rcMonitor, x)
    };
    let rect = popup_rect(monitor_rect, size, dpi, top_fraction);
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

    #[test]
    fn popup_centers_in_physical_monitor_coordinates_at_each_dpi() {
        let size = gpui::size(gpui::px(640.), gpui::px(196.));
        let monitor = RECT {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        };
        for (dpi, width, height) in [(96, 640, 196), (120, 800, 245), (144, 960, 294)] {
            let r = popup_rect(monitor, size, dpi, 0.2);
            assert_eq!((r.left + r.right) / 2, -960);
            assert_eq!(r.top, -864);
            assert_eq!((r.right - r.left, r.bottom - r.top), (width, height));
        }
    }
}
