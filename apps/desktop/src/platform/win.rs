//! Small Win32 helpers: window visibility and focus, the console, process timing. No input injection.

#![allow(unsafe_code)] // Plain Win32 calls on window handles we own; each is commented.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{FILETIME, HWND};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};

/// The native handle of a GPUI window, as an integer (0 if unavailable).
pub fn hwnd_of(window: &Window) -> isize {
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
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

/// Release builds are GUI-subsystem executables. When started from a terminal, write `--help`, errors
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
