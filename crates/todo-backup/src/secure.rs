//! Owner-only files and directories, and durable file replacement.
//!
//! - **Windows:** objects are created with a protected DACL (`D:P`) holding a single full-control ACE for the
//!   current token user, so nothing is inherited from the parent and no other account (not even
//!   Administrators or SYSTEM) is granted access. Directory ACEs are inheritable, so files created inside
//!   later get the same protection. Replacement uses `MoveFileExW` with `MOVEFILE_WRITE_THROUGH`, which
//!   returns only once the rename has been flushed to disk.
//! - **Unix:** files are created with mode 0600 and directories with 0700 (the umask can only narrow them).
//!   Replacement is `rename` followed by an fsync of the parent directory.
//!
//! The security descriptor is applied when the object is created, so there is never a moment where the
//! object exists with broader access.

use std::fs::File;
use std::io;
use std::path::Path;

/// Creates a new file (failing if it exists) that only the current user can access.
pub fn create_private_file(path: &Path) -> io::Result<File> {
    imp::create_private_file(path)
}

/// Creates one new directory (failing if it exists) that only the current user can access.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    imp::create_private_dir(path)
}

/// Renames `from` over `to` and makes the rename durable before returning.
pub fn replace_durably(from: &Path, to: &Path) -> io::Result<()> {
    imp::replace_durably(from, to)
}

/// A description of who can access `path`, for tests: the DACL as SDDL on Windows, `mode=0o...` on Unix.
pub fn describe_access(path: &Path) -> io::Result<String> {
    imp::describe_access(path)
}

/// The SDDL a private object must have on Windows; `None` elsewhere.
pub fn expected_private_sddl(dir: bool) -> io::Result<Option<String>> {
    imp::expected_private_sddl(dir)
}

#[cfg(unix)]
mod imp {
    use std::fs::{DirBuilder, File, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        DirBuilder::new().mode(0o700).create(path)
    }

    pub fn replace_durably(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)?;
        let parent = match to.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        File::open(parent)?.sync_all()
    }

    pub fn describe_access(path: &Path) -> io::Result<String> {
        let mode = std::fs::symlink_metadata(path)?.permissions().mode() & 0o777;
        Ok(format!("mode={mode:o}"))
    }

    pub fn expected_private_sddl(_dir: bool) -> io::Result<Option<String>> {
        Ok(None)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    //! Win32 FFI. Every pointer handed to the API is either a valid, NUL-terminated wide string owned by the
    //! caller for the duration of the call, or a buffer this module allocated with the size the API reported.
    //! Memory the API allocates (`LocalAlloc`) is released by [`LocalBox`], handles by [`HandleGuard`].

    use std::ffi::{OsStr, c_void};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_BACKUP_SEMANTICS, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct LocalBox(*mut c_void);

    impl Drop for LocalBox {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from an API that allocates with LocalAlloc, and is freed once.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by this module and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Reads a NUL-terminated wide string.
    ///
    /// SAFETY: `p` must point to a valid NUL-terminated UTF-16 string.
    unsafe fn from_wide(p: *const u16) -> String {
        let mut n = 0;
        // SAFETY: guaranteed by the caller.
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: the `n` elements before the terminator are readable.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
    }

    /// The current token user's SID, as `S-1-5-...`.
    fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is a valid out pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _token = HandleGuard(token);
        let mut len = 0u32;
        // SAFETY: a size query with a null buffer of length 0; it fails and reports the needed size.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        // u64 elements keep the buffer suitably aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `len` bytes.
        if unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success the buffer starts with a TOKEN_USER whose SID points into the same buffer.
        let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
        let mut s: *mut u16 = null_mut();
        // SAFETY: the SID is valid while `buf` lives; `s` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut s) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _s = LocalBox(s.cast());
        // SAFETY: the API returned a NUL-terminated string.
        Ok(unsafe { from_wide(s) })
    }

    pub fn expected_private_sddl(dir: bool) -> io::Result<Option<String>> {
        let sid = current_user_sid()?;
        // P: protected, nothing inherited from the parent. FA: full access. OICI: inherited by children.
        Ok(Some(if dir {
            format!("D:P(A;OICI;FA;;;{sid})")
        } else {
            format!("D:P(A;;FA;;;{sid})")
        }))
    }

    fn private_descriptor(dir: bool) -> io::Result<LocalBox> {
        let sddl = expected_private_sddl(dir)?.unwrap_or_default();
        let w = wide(OsStr::new(&sddl));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `w` is NUL-terminated; `sd` is a valid out pointer; the size out pointer may be null.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalBox(sd))
    }

    fn attributes(sd: &LocalBox) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        }
    }

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        let sd = private_descriptor(false)?;
        let sa = attributes(&sd);
        let w = wide(path.as_os_str());
        // SAFETY: `w` is NUL-terminated and `sa` (with the descriptor it points to) outlives the call.
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                &sa,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `h` is a valid file handle that nothing else owns.
        Ok(unsafe { File::from_raw_handle(h) })
    }

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        let sd = private_descriptor(true)?;
        let sa = attributes(&sd);
        let w = wide(path.as_os_str());
        // SAFETY: as in create_private_file.
        if unsafe { CreateDirectoryW(w.as_ptr(), &sa) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn replace_durably(from: &Path, to: &Path) -> io::Result<()> {
        let (f, t) = (wide(from.as_os_str()), wide(to.as_os_str()));
        // SAFETY: both strings are NUL-terminated and live for the call.
        if unsafe {
            MoveFileExW(
                f.as_ptr(),
                t.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn describe_access(path: &Path) -> io::Result<String> {
        // Backup semantics lets the same call open directories.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the handle is open for reading (which includes READ_CONTROL); out pointers are valid or null.
        let rc = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let _sd = LocalBox(sd);
        let mut s: *mut u16 = null_mut();
        // SAFETY: `sd` is a valid descriptor; `s` is a valid out pointer.
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut s,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _s = LocalBox(s.cast());
        // SAFETY: the API returned a NUL-terminated string.
        Ok(unsafe { from_wide(s) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn assert_private(path: &Path, dir: bool) {
        let got = describe_access(path).expect("describe");
        match expected_private_sddl(dir).expect("sddl") {
            Some(sddl) => assert_eq!(got, sddl),
            None => assert_eq!(got, if dir { "mode=700" } else { "mode=600" }),
        }
    }

    #[test]
    fn private_file_and_dir_are_owner_only_and_never_overwrite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("private");
        create_private_dir(&dir).expect("dir");
        assert_private(&dir, true);
        assert!(create_private_dir(&dir).is_err());

        let file = dir.join("secret");
        create_private_file(&file)
            .expect("file")
            .write_all(b"x")
            .expect("write");
        assert_private(&file, false);
        assert_eq!(
            create_private_file(&file).expect_err("exists").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&file).expect("read"), b"x");
    }

    #[test]
    fn replace_durably_replaces() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::write(&a, b"new").expect("write");
        std::fs::write(&b, b"old").expect("write");
        replace_durably(&a, &b).expect("replace");
        assert_eq!(std::fs::read(&b).expect("read"), b"new");
        assert!(!a.exists());
    }
}
