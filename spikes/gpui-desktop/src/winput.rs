//! Win32 helpers: window state queries, show/hide (no input injection).
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() as isize }
}
pub fn is_iconic(h: isize) -> bool {
    unsafe { IsIconic(h as HWND) != 0 }
}
pub fn is_visible(h: isize) -> bool {
    unsafe { IsWindowVisible(h as HWND) != 0 }
}
pub fn show_and_focus(h: isize) {
    unsafe {
        let h = h as HWND;
        ShowWindow(h, SW_SHOW);
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}
pub fn hide(h: isize) {
    unsafe {
        ShowWindow(h as HWND, SW_HIDE);
    }
}

pub fn title_of(h: isize) -> String {
    unsafe {
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(h as HWND, buf.as_mut_ptr(), 128);
        let mut cls = [0u16; 64];
        let m = GetClassNameW(h as HWND, cls.as_mut_ptr(), 64);
        format!("title={:?} class={:?}", String::from_utf16_lossy(&buf[..n as usize]), String::from_utf16_lossy(&cls[..m as usize]))
    }
}
