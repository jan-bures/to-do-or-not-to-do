//! Tells the app when the clock jumps: the system time or time zone was changed, or the computer woke
//! from sleep. Reminders are planned again then, rather than waiting for a timer that measured the
//! old time (or slept through the suspension).

#![allow(unsafe_code)] // Plain Win32 calls on a window this module owns; each is commented.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WM_POWERBROADCAST,
    WM_TIMECHANGE, WNDCLASSW, WS_POPUP,
};

/// `WM_POWERBROADCAST`'s resume events: woken by the system, or by the user.
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

thread_local! {
    /// What the window procedure runs; the window lives on the UI thread, so a thread local is enough.
    static ON_JUMP: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Whether a message to a top-level window says the clock jumped.
fn is_jump(msg: u32, wparam: WPARAM) -> bool {
    msg == WM_TIMECHANGE
        || (msg == WM_POWERBROADCAST
            && matches!(wparam, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC))
}

unsafe extern "system" fn watch_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if is_jump(msg, wparam)
        && let Some(callback) = ON_JUMP.with(|slot| slot.borrow().clone())
        && catch_unwind(AssertUnwindSafe(|| callback())).is_err()
    {
        // It must not unwind through the window procedure (that would abort the process).
        tracing::error!("the clock change callback panicked");
    }
    // SAFETY: the arguments are the ones Windows passed in.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// One hidden top-level window that receives the time-change and resume broadcasts. Create it on the
/// UI thread and keep it alive; `on_jump` runs there, inside the message loop, so it should only hand
/// the news on (a channel), not act on it.
pub struct ClockWatch {
    hwnd: HWND,
}

impl std::fmt::Debug for ClockWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockWatch").finish_non_exhaustive()
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

impl ClockWatch {
    pub fn new(on_jump: impl Fn() + 'static) -> anyhow::Result<Self> {
        let class_name = wide("TodoDesktopClockWatch");
        // SAFETY: a null module name gives this exe's handle.
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(watch_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        // SAFETY: `class` and the name it points to are valid for the call. Registering the class twice
        // fails harmlessly, and the window below then uses the first registration.
        unsafe { RegisterClassW(&class) };
        // SAFETY: a hidden pop-up window with no parent; every pointer is null or outlives the call.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            anyhow::bail!("could not create the clock watch window");
        }
        ON_JUMP.with(|slot| *slot.borrow_mut() = Some(Rc::new(on_jump)));
        Ok(Self { hwnd })
    }
}

impl Drop for ClockWatch {
    fn drop(&mut self) {
        ON_JUMP.with(|slot| *slot.borrow_mut() = None);
        // SAFETY: the window was created by `new` on this thread and is destroyed once.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_NULL, WM_SETTINGCHANGE};

    #[test]
    fn the_watch_reports_time_changes_and_resumes_only() {
        let jumps = Rc::new(Cell::new(0));
        let seen = jumps.clone();
        let watch = ClockWatch::new(move || seen.set(seen.get() + 1)).expect("watch");
        // SAFETY: the window is this test's own and alive; these messages have no pointer arguments.
        let send = |msg, wparam| unsafe { SendMessageW(watch.hwnd, msg, wparam, 0) };
        send(WM_NULL, 0);
        send(WM_SETTINGCHANGE, 0);
        // A suspend (PBT_APMSUSPEND) isn't a jump; the resumes are.
        send(WM_POWERBROADCAST, 0x0004);
        assert_eq!(jumps.get(), 0);
        send(WM_TIMECHANGE, 0);
        assert_eq!(jumps.get(), 1);
        send(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC);
        send(WM_POWERBROADCAST, PBT_APMRESUMESUSPEND);
        assert_eq!(jumps.get(), 3);
        drop(watch);
    }
}
