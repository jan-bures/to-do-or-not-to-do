//! One running copy per user session: a named mutex says who is first, a named event lets a second launch
//! wake the first one up.
//!
//! The second launch signals the event and exits; the first one shows and focuses its window. Both objects
//! live in the session's `Local\` namespace, so they need no rights beyond the user's own.

#![allow(unsafe_code)] // Win32 kernel objects. Each call is a plain FFI call with checked results.

use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

/// The tag the real app uses. Tests use their own, so they never collide with a running app.
pub const APP_TAG: &str = "app";

/// Names of the mutex and the event for `tag`.
pub fn object_names(tag: &str) -> (String, String) {
    let base = format!(r"Local\to-do-or-not-to-do.desktop.{tag}");
    (format!("{base}.instance"), format!("{base}.activate"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An owned kernel handle, closed on drop. Stored as an integer so it can cross threads.
#[derive(Debug)]
struct Handle(isize);

impl Handle {
    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        unsafe { CloseHandle(self.raw()) };
    }
}

fn create_event(name: &str) -> io::Result<Handle> {
    let name = wide(name);
    // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call. Auto-reset, initially unset.
    let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(h as isize))
}

/// The first running copy. Holding it keeps the mutex owned.
#[derive(Debug)]
pub struct Primary {
    _mutex: Handle,
    event: Handle,
}

/// Whether this process is the first copy.
#[derive(Debug)]
pub enum Instance {
    Primary(Primary),
    /// Another copy runs; it has been asked to show itself.
    Secondary,
}

/// Claims the single-instance mutex for `tag`. A second claimant signals the first and gets `Secondary`.
pub fn acquire(tag: &str) -> io::Result<Instance> {
    let (mutex_name, event_name) = object_names(tag);
    let mutex_w = wide(&mutex_name);
    // SAFETY: `mutex_w` is NUL-terminated and outlives the call; we don't request initial ownership.
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_w.as_ptr()) };
    // Read the error before any other API call can overwrite it.
    // SAFETY: plain thread-local read.
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if mutex.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mutex = Handle(mutex as isize);
    // Both copies create-or-open the event, so the order in which they get here doesn't matter.
    let event = create_event(&event_name)?;
    if already {
        drop(mutex);
        // Let the first copy take the foreground: this process was just launched by the user, so it may.
        // SAFETY: plain FFI call; failure only means Windows may flash the taskbar instead.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        // SAFETY: `event` is a valid event handle.
        if unsafe { SetEvent(event.raw()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(Instance::Secondary);
    }
    Ok(Instance::Primary(Primary {
        _mutex: mutex,
        event,
    }))
}

impl Primary {
    /// Waits up to `timeout` for a second launch. `None` waits forever.
    pub fn wait_for_activation(&self, timeout: Option<Duration>) -> io::Result<bool> {
        let ms = timeout.map_or(INFINITE, |t| {
            u32::try_from(t.as_millis()).unwrap_or(INFINITE - 1)
        });
        // SAFETY: `self.event` stays valid for the duration of the wait.
        match unsafe { WaitForSingleObject(self.event.raw(), ms) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Calls `on_activate` on a background thread every time another copy is launched. The thread runs for
    /// the life of the process.
    pub fn listen(self, on_activate: impl Fn() + Send + 'static) {
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                while let Ok(true) = self.wait_for_activation(None) {
                    on_activate();
                }
            })
            .map(|_| ())
            .unwrap_or_else(|e| tracing::warn!("single-instance listener failed to start: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tag(name: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("test-{name}-{}-{nanos}", std::process::id())
    }

    #[test]
    fn names_are_session_local_and_tagged() {
        let (m, e) = object_names("app");
        assert!(m.starts_with(r"Local\") && e.starts_with(r"Local\"));
        assert_ne!(m, e);
        assert_ne!(object_names("a").0, object_names("b").0);
    }

    #[test]
    fn the_first_claim_wins_and_a_second_one_activates_it() {
        let tag = unique_tag("second");
        let Instance::Primary(primary) = acquire(&tag).expect("first") else {
            panic!("the first claim must be primary");
        };
        assert!(
            !primary
                .wait_for_activation(Some(Duration::from_millis(0)))
                .expect("wait")
        );
        assert!(matches!(
            acquire(&tag).expect("second"),
            Instance::Secondary
        ));
        assert!(
            primary
                .wait_for_activation(Some(Duration::from_secs(2)))
                .expect("wait")
        );
        // Auto-reset: one launch, one activation.
        assert!(
            !primary
                .wait_for_activation(Some(Duration::from_millis(0)))
                .expect("wait")
        );
    }

    #[test]
    fn releasing_the_primary_lets_the_next_launch_be_primary() {
        let tag = unique_tag("release");
        let first = acquire(&tag).expect("first");
        assert!(matches!(first, Instance::Primary(_)));
        drop(first);
        assert!(matches!(
            acquire(&tag).expect("again"),
            Instance::Primary(_)
        ));
    }

    #[test]
    fn listen_reports_each_launch() {
        let tag = unique_tag("listen");
        let Instance::Primary(primary) = acquire(&tag).expect("first") else {
            panic!("the first claim must be primary");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        primary.listen(move || {
            let _ = tx.send(());
        });
        assert!(matches!(
            acquire(&tag).expect("second"),
            Instance::Secondary
        ));
        rx.recv_timeout(Duration::from_secs(2)).expect("activation");
    }
}
