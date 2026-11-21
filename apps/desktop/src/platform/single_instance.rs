//! One running copy per user and session: a named mutex says who is first, a named event lets a second
//! launch wake the first one up.
//!
//! The second launch signals the event and exits; the first one shows and focuses its window. Both objects
//! live in the session's `Local\` namespace, carry the user's SID in their names (so two accounts in one
//! session, as with "Run as different user", never meet) and have a DACL that admits only that user.

#![allow(unsafe_code)] // Win32 kernel objects and the token. Each call is a plain FFI call with checked results.

use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, INFINITE, OpenProcessToken, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

/// The tag the real app uses. Tests use their own, so they never collide with a running app.
pub const APP_TAG: &str = "app";

/// Names of the mutex and the event for `user` (a SID string) and `tag`.
pub fn object_names(user: &str, tag: &str) -> (String, String) {
    let base = format!(r"Local\to-do-or-not-to-do.desktop.{user}.{tag}");
    (format!("{base}.instance"), format!("{base}.activate"))
}

/// The SDDL of the objects' security descriptor: full access for `user` only, nothing inherited.
pub fn owner_only_sddl(user: &str) -> String {
    format!("D:P(A;;GA;;;{user})")
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

/// Memory from `LocalAlloc` (as the SDDL conversions return), freed on drop.
struct Local(*mut core::ffi::c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Win32 API that allocates with LocalAlloc; freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The current user's SID as a string (`S-1-5-21-...`).
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of the current process needs no closing; `token` receives a real handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token as isize);
    let mut len = 0u32;
    // SAFETY: a size query with a null buffer; it fails with ERROR_INSUFFICIENT_BUFFER and sets `len`.
    unsafe { GetTokenInformation(token.raw(), TokenUser, std::ptr::null_mut(), 0, &mut len) };
    // u64 words keep the buffer aligned for TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER whose SID points into the same buffer.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` is valid while `buf` lives; `text` receives a LocalAlloc'd NUL-terminated string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let text = Local(text.cast());
    let ptr = text.0.cast::<u16>();
    // SAFETY: NUL-terminated UTF-16 from the call above.
    let len = (0..).take_while(|&i| unsafe { *ptr.add(i) } != 0).count();
    // SAFETY: `len` code units are initialised.
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(ptr, len)
    }))
}

/// Security attributes that admit only `user`. Keep the returned owner alive while the attributes are used.
fn owner_only(user: &str) -> io::Result<(SECURITY_ATTRIBUTES, Local)> {
    let sddl = wide(&owner_only_sddl(user));
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd self-relative descriptor.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    Ok((attrs, Local(sd)))
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

/// Claims the single-instance mutex for `tag` and the current user. A second claimant signals the first
/// and gets `Secondary`.
pub fn acquire(tag: &str) -> io::Result<Instance> {
    acquire_with(tag, || {})
}

/// [`acquire`], with a hook that runs right after this process won the mutex: the earliest moment a
/// second launch can find it. Tests use it to launch that second copy there.
fn acquire_with(tag: &str, after_claim: impl FnOnce()) -> io::Result<Instance> {
    let user = current_user_sid()?;
    let (mutex_name, event_name) = object_names(&user, tag);
    let (attrs, _sd) = owner_only(&user)?;
    // The event first, and keep it open: a second copy that sees our mutex must find this very event.
    // Opened after the mutex, the event could be created, signalled and destroyed by a second copy in
    // between, and we would wait on a fresh, unsignalled one.
    let event_w = wide(&event_name);
    // SAFETY: `event_w` is NUL-terminated and `attrs` (with its descriptor) outlive the call. Auto-reset,
    // initially unset.
    let event = unsafe { CreateEventW(&attrs, 0, 0, event_w.as_ptr()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = Handle(event as isize);
    let mutex_w = wide(&mutex_name);
    // SAFETY: as above; we don't request initial ownership (existence is the signal).
    let mutex = unsafe { CreateMutexW(&attrs, 0, mutex_w.as_ptr()) };
    // Read the error before any other API call can overwrite it.
    // SAFETY: plain thread-local read.
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if mutex.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mutex = Handle(mutex as isize);
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
    after_claim();
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
    fn names_are_session_local_and_per_user_and_tag() {
        let (m, e) = object_names("S-1-5-21-1-2-3-1001", "app");
        assert!(m.starts_with(r"Local\") && e.starts_with(r"Local\"));
        assert!(m.contains("S-1-5-21-1-2-3-1001"));
        assert_ne!(m, e);
        assert_ne!(object_names("S-1", "a").0, object_names("S-1", "b").0);
        // Two accounts in one session never share an instance.
        assert_ne!(
            object_names("S-1-5-21-1", "app"),
            object_names("S-1-5-21-2", "app")
        );
    }

    #[test]
    fn the_objects_admit_only_the_user() {
        assert_eq!(owner_only_sddl("S-1-5-21-9"), "D:P(A;;GA;;;S-1-5-21-9)");
        let sid = current_user_sid().expect("sid");
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert!(owner_only(&sid).is_ok());
    }

    #[test]
    fn a_launch_right_after_the_claim_is_not_lost() {
        // The second copy runs at the earliest moment it can see our mutex, signals and exits (dropping its
        // handles). Its signal must still reach us.
        let tag = unique_tag("race");
        let Instance::Primary(primary) = acquire_with(&tag, || {
            assert!(matches!(
                acquire(&tag).expect("second"),
                Instance::Secondary
            ));
        })
        .expect("first") else {
            panic!("the first claim must be primary");
        };
        assert!(
            primary
                .wait_for_activation(Some(Duration::from_millis(0)))
                .expect("wait")
        );
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
