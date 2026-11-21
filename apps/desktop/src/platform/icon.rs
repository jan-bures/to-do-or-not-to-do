//! Icon sizes for a display scale, and a watcher that tells the tray when the notification area's scale changes.
//!
//! The .ico has a hand-tuned image for each scale (16/32 px at 100 %, 20/40 at 125 %, 24/48 at 150 %, 32/64 at
//! 200 %), so every icon is loaded at the size its DPI asks for instead of being scaled by Windows. The main
//! window does the same in the vendored `gpui_windows`, per monitor.

#![allow(unsafe_code)] // Plain Win32 calls on a window this module owns; each is commented.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW,
    KillTimer, MSGFLT_ALLOW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SetTimer,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_POPUP,
};

/// The DPI at 100 % scaling.
const BASE_DPI: u32 = 96;

/// The small (16 px at 100 %) and large (32 px at 100 %) icon sizes for a DPI, rounded like `MulDiv`. This is
/// what `GetSystemMetricsForDpi(SM_CXSMICON / SM_CXICON, dpi)` returns: 96 gives 16/32, 120 gives 20/40, 144
/// gives 24/48 and 192 gives 32/64.
pub const fn icon_sizes_for_dpi(dpi: u32) -> (u32, u32) {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    (
        (16 * dpi + BASE_DPI / 2) / BASE_DPI,
        (32 * dpi + BASE_DPI / 2) / BASE_DPI,
    )
}

/// The tray icon size for a DPI: what Windows reports for it, or the same arithmetic if it reports nothing.
pub fn small_icon_size_for_dpi(dpi: u32) -> u32 {
    // SAFETY: GetSystemMetricsForDpi has no preconditions.
    let reported = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    u32::try_from(reported)
        .ok()
        .filter(|size| *size > 0)
        .unwrap_or(icon_sizes_for_dpi(dpi).0)
}

/// The DPI the notification area is drawn at: the taskbar's own, which can differ from the main window's
/// monitor and from the system DPI. Falls back to the system DPI when there is no taskbar to ask.
pub fn notification_area_dpi() -> u32 {
    let class = wide("Shell_TrayWnd");
    // SAFETY: `class` is a NUL-terminated UTF-16 string that outlives the call.
    let taskbar = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    // SAFETY: both calls tolerate a null or stale handle and return 0 for it.
    let dpi = if taskbar.is_null() {
        0
    } else {
        unsafe { GetDpiForWindow(taskbar) }
    };
    if dpi != 0 {
        return dpi;
    }
    // SAFETY: no preconditions.
    match unsafe { GetDpiForSystem() } {
        0 => BASE_DPI,
        dpi => dpi,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Calls back when the notification area's scale may have changed: a DPI or display change, a system setting
/// change, or Explorer restarting. It owns one hidden top-level window, which is what receives those broadcasts
/// (the tray library's own window handles none of them). Create it on the UI thread and keep it alive.
pub struct DpiWatch {
    hwnd: HWND,
}

impl std::fmt::Debug for DpiWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DpiWatch").finish_non_exhaustive()
    }
}

/// The re-check timer's id and delay. The taskbar can take a moment to adopt a new scale after the broadcast,
/// so each change is checked at once and again a second later.
const RECHECK_TIMER: usize = 1;
const RECHECK_MS: u32 = 1000;

thread_local! {
    /// What the window procedure runs; the window lives on the UI thread, so a thread local is enough.
    static ON_CHANGE: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// Set while the callback runs. Windows can deliver another change message from inside the callback (the
    /// tray's `set_icon` talks to the taskbar, which can send messages back), and the tray library must not be
    /// re-entered while it is busy.
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// A change arrived while the callback was busy; it is run once more after the callback returns.
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// The most times one outer call re-runs the callback for changes that arrived meanwhile; the re-check timer
/// covers anything beyond that.
const MAX_RERUNS: u32 = 3;

fn taskbar_created_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| {
        let name = wide("TaskbarCreated");
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

/// Clears `BUSY` however the callback ends, even by panicking.
struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.with(|busy| busy.set(false));
    }
}

/// Runs the callback, never re-entrantly: a change that arrives while it runs is coalesced into one more run
/// after it returns. A panic in the callback is logged and stops here; it must not unwind through the window
/// procedure (that would abort the process).
fn notify() {
    if BUSY.with(|busy| busy.replace(true)) {
        PENDING.with(|pending| pending.set(true));
        return;
    }
    let _guard = BusyGuard;
    for _ in 0..=MAX_RERUNS {
        PENDING.with(|pending| pending.set(false));
        // Clone the callback out, so no borrow is held while it runs.
        let Some(callback) = ON_CHANGE.with(|slot| slot.borrow().clone()) else {
            return;
        };
        if catch_unwind(AssertUnwindSafe(|| callback())).is_err() {
            tracing::error!("the tray icon scale callback panicked");
            return;
        }
        if !PENDING.with(Cell::get) {
            return;
        }
    }
}

unsafe extern "system" fn watch_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DPICHANGED
        || msg == WM_DISPLAYCHANGE
        || msg == WM_SETTINGCHANGE
        || msg == taskbar_created_message()
    {
        notify();
        // SAFETY: `hwnd` is this procedure's own window.
        unsafe { SetTimer(hwnd, RECHECK_TIMER, RECHECK_MS, None) };
    } else if msg == WM_TIMER && wparam == RECHECK_TIMER {
        // SAFETY: as above.
        unsafe { KillTimer(hwnd, RECHECK_TIMER) };
        notify();
    }
    // SAFETY: the arguments are the ones Windows passed in.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl DpiWatch {
    /// Creates the watcher. `on_change` runs on this thread, inside the message loop.
    pub fn new(on_change: impl Fn() + 'static) -> anyhow::Result<Self> {
        let class_name = wide("TodoDesktopDpiWatch");
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
        // SAFETY: `class` and the name it points to are valid for the call. Registering the class twice fails
        // harmlessly, and the window below then uses the first registration.
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
            anyhow::bail!("could not create the DPI watch window");
        }
        ON_CHANGE.with(|slot| *slot.borrow_mut() = Some(Rc::new(on_change)));
        // An elevated app only receives messages from lower-integrity processes if they are allowed. Explorer
        // announces a restart with `TaskbarCreated`, and the tray library lets it through for the same reason.
        // SAFETY: `hwnd` is the window created above; a null struct pointer is allowed.
        let allowed = unsafe {
            ChangeWindowMessageFilterEx(
                hwnd,
                taskbar_created_message(),
                MSGFLT_ALLOW,
                std::ptr::null_mut(),
            )
        };
        if allowed == 0 {
            tracing::debug!("TaskbarCreated could not be allowed through the message filter");
        }
        Ok(Self { hwnd })
    }
}

#[cfg(test)]
impl DpiWatch {
    fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for DpiWatch {
    fn drop(&mut self) {
        ON_CHANGE.with(|slot| *slot.borrow_mut() = None);
        // SAFETY: the window was created by `new` on this thread and is destroyed once.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_sizes_follow_the_display_scale() {
        assert_eq!(icon_sizes_for_dpi(96), (16, 32)); // 100 %
        assert_eq!(icon_sizes_for_dpi(120), (20, 40)); // 125 %
        assert_eq!(icon_sizes_for_dpi(144), (24, 48)); // 150 %
        assert_eq!(icon_sizes_for_dpi(192), (32, 64)); // 200 %
        assert_eq!(icon_sizes_for_dpi(0), (16, 32), "an unknown DPI is 100 %");
    }

    #[test]
    fn windows_reports_the_same_sizes_for_the_common_scales() {
        for dpi in [96, 120, 144, 192] {
            assert_eq!(
                small_icon_size_for_dpi(dpi),
                icon_sizes_for_dpi(dpi).0,
                "{dpi} dpi"
            );
        }
    }

    #[test]
    fn the_notification_area_has_a_dpi() {
        assert!(notification_area_dpi() >= BASE_DPI);
    }

    /// The pixel width of the exe's icon resource loaded at `size` (what the tray and the window ask for).
    fn resource_icon_width(size: i32) -> Option<i32> {
        use windows_sys::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DestroyIcon, GetIconInfo, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
        };
        // SAFETY: plain Win32 calls; the handles are freed before returning.
        unsafe {
            let icon = LoadImageW(
                GetModuleHandleW(std::ptr::null()),
                std::ptr::without_provenance::<u16>(1), // MAKEINTRESOURCE(1)
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            );
            if icon.is_null() {
                return None;
            }
            let mut info: ICONINFO = std::mem::zeroed();
            let mut width = None;
            if GetIconInfo(icon, &mut info) != 0 {
                let mut bitmap: BITMAP = std::mem::zeroed();
                if GetObjectW(
                    info.hbmColor,
                    std::mem::size_of::<BITMAP>() as i32,
                    (&mut bitmap as *mut BITMAP).cast(),
                ) != 0
                {
                    width = Some(bitmap.bmWidth);
                }
                // GetIconInfo hands over copies of both bitmaps; free them on every path.
                DeleteObject(info.hbmMask);
                DeleteObject(info.hbmColor);
            }
            DestroyIcon(icon);
            width
        }
    }

    #[test]
    fn the_exe_resource_has_an_image_for_every_scale() {
        // Every size the tray and the window ask for at 100 %, 125 %, 150 % and 200 % comes out at exactly that size.
        for dpi in [96, 120, 144, 192] {
            let (small, large) = icon_sizes_for_dpi(dpi);
            for size in [small, large] {
                assert_eq!(
                    resource_icon_width(size as i32),
                    Some(size as i32),
                    "{size} px at {dpi} dpi"
                );
            }
        }
    }

    #[test]
    fn the_watch_reports_display_and_setting_changes_only() {
        use std::{cell::Cell, rc::Rc};
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_NULL};

        let count = Rc::new(Cell::new(0));
        let watch = {
            let count = count.clone();
            DpiWatch::new(move || count.set(count.get() + 1)).expect("creates the window")
        };
        // Sent from this thread, so the window procedure runs at once.
        let send = |msg: u32| unsafe { SendMessageW(watch.hwnd(), msg, 0, 0) };
        send(WM_NULL);
        assert_eq!(count.get(), 0, "an unrelated message is ignored");
        send(WM_DISPLAYCHANGE);
        assert_eq!(count.get(), 1);
        send(WM_SETTINGCHANGE);
        assert_eq!(count.get(), 2);
        send(taskbar_created_message());
        assert_eq!(count.get(), 3, "Explorer restarting");
        drop(watch);
    }

    #[test]
    fn a_change_during_the_callback_runs_it_again_afterwards_not_inside() {
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

        let hwnd = Rc::new(Cell::new(0isize));
        let (runs, depth, deepest) = (
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        );
        let watch = {
            let (hwnd, runs, depth, deepest) =
                (hwnd.clone(), runs.clone(), depth.clone(), deepest.clone());
            DpiWatch::new(move || {
                runs.set(runs.get() + 1);
                depth.set(depth.get() + 1);
                deepest.set(deepest.get().max(depth.get()));
                if runs.get() == 1 {
                    // Like the taskbar answering `set_icon` with another change message, twice over.
                    for msg in [WM_DISPLAYCHANGE, WM_SETTINGCHANGE] {
                        // SAFETY: the window is alive; sent from its own thread, so it runs at once.
                        unsafe { SendMessageW(hwnd.get() as HWND, msg, 0, 0) };
                    }
                }
                depth.set(depth.get() - 1);
            })
            .expect("creates the window")
        };
        hwnd.set(watch.hwnd() as isize);
        // SAFETY: as above.
        unsafe { SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 0, 0) };
        assert_eq!(deepest.get(), 1, "the callback was re-entered");
        assert_eq!(
            runs.get(),
            2,
            "the two nested changes coalesce into one more run"
        );
        // The flags are clear again: the next change runs the callback once.
        // SAFETY: as above.
        unsafe { SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 0, 0) };
        assert_eq!(runs.get(), 3);
    }

    #[test]
    fn a_panicking_callback_does_not_unwind_through_the_window_procedure() {
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

        let runs = Rc::new(Cell::new(0));
        let watch = {
            let runs = runs.clone();
            DpiWatch::new(move || {
                runs.set(runs.get() + 1);
                assert!(runs.get() != 1, "the first run fails on purpose");
            })
            .expect("creates the window")
        };
        // SAFETY: the window is alive; sent from its own thread.
        unsafe { SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 0, 0) };
        assert_eq!(runs.get(), 1);
        // The busy flag was released, so the watcher still works.
        // SAFETY: as above.
        unsafe { SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 0, 0) };
        assert_eq!(runs.get(), 2);
    }
}
