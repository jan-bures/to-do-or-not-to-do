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
//!
//! Some file systems can't store access control at all: FAT and exFAT on Windows have no ACLs, and vfat or
//! similar mounts on Unix ignore modes. A private object created there would silently be readable by
//! everyone, so creation refuses instead: on Windows before anything is created, when the volume lacks
//! `FILE_PERSISTENT_ACLS`; on Unix right after, when the new object doesn't have the requested mode.

use std::fs::{File, Metadata};
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

/// A description of who can access `path`, for diagnostics: the DACL as SDDL on Windows, `mode=...` on Unix.
pub fn describe_access(path: &Path) -> io::Result<String> {
    imp::describe_access(path)
}

/// The kind of private object [`verify_private`] checks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Private {
    /// Created by [`create_private_file`].
    File,
    /// Created by [`create_private_dir`].
    Dir,
    /// Created by [`create_private_file`], or created inside a private directory and inheriting its access.
    AnyFile,
}

/// Checks that only the current user can access `path`. Fails with `InvalidData` and a description if
/// anyone else can.
///
/// On Windows this inspects the DACL itself rather than comparing SDDL text, which renders some SIDs as
/// aliases (the built-in Administrator is `LA`): exactly one ACE, an allow ACE for the current token user
/// with full access, protected from inheritance unless the object got it from a private parent.
pub fn verify_private(path: &Path, kind: Private) -> io::Result<()> {
    imp::verify_private(path, kind)
}

/// True for a directory entry that isn't a symlink, junction or any other reparse point.
pub fn is_plain(metadata: &Metadata) -> bool {
    !metadata.file_type().is_symlink() && !imp::is_reparse_point(metadata)
}

fn not_private(path: &Path, why: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} is not owner-only: {why}", path.display()),
    )
}

/// The error for a volume or file system that can't store owner-only access.
fn no_access_control(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{what}, so todo-backup can't make files there owner-only; use a folder on a volume with \
             access control (NTFS or ReFS on Windows)"
        ),
    )
}

#[cfg(unix)]
mod imp {
    use std::fs::{DirBuilder, File, Metadata, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    use super::Private;

    /// Fails if the file system gave a new object more than the owner bits it was created with.
    fn check_mode(path: &Path, mode: u32) -> io::Result<()> {
        if mode & 0o077 == 0 {
            return Ok(());
        }
        let dir = path.parent().unwrap_or(path);
        Err(super::no_access_control(format_args!(
            "the file system at {} ignores file modes (a new private object got mode {mode:o})",
            dir.display()
        )))
    }

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        let checked = file
            .metadata()
            .and_then(|m| check_mode(path, m.permissions().mode() & 0o777));
        if let Err(e) = checked {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
        Ok(file)
    }

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        DirBuilder::new().mode(0o700).create(path)?;
        let checked = std::fs::symlink_metadata(path)
            .and_then(|m| check_mode(path, m.permissions().mode() & 0o777));
        if let Err(e) = checked {
            let _ = std::fs::remove_dir(path);
            return Err(e);
        }
        Ok(())
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

    pub fn verify_private(path: &Path, kind: Private) -> io::Result<()> {
        let mode = std::fs::symlink_metadata(path)?.permissions().mode() & 0o777;
        let want = if kind == Private::Dir { 0o700 } else { 0o600 };
        if mode == want {
            Ok(())
        } else {
            Err(super::not_private(
                path,
                format_args!("mode {mode:o}, expected {want:o}"),
            ))
        }
    }

    pub fn is_reparse_point(_metadata: &Metadata) -> bool {
        false
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    //! Win32 FFI. Every pointer handed to the API is either a valid, NUL-terminated wide string owned by the
    //! caller for the duration of the call, or a buffer this module allocated with the size the API reported.
    //! Memory the API allocates (`LocalAlloc`) is released by [`LocalBox`], handles by [`HandleGuard`].

    use std::ffi::{OsStr, c_void};
    use std::fs::{File, Metadata};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
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
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetSecurityDescriptorControl, GetTokenInformation, INHERIT_ONLY_ACE, INHERITED_ACE,
        OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, GetVolumeInformationW,
        GetVolumePathNameW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        READ_CONTROL,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::Private;

    /// `FILE_PERSISTENT_ACLS` and `ACCESS_ALLOWED_ACE_TYPE` from `Win32::System::SystemServices`, defined
    /// here rather than enabling that large module.
    const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

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

    /// The current process token's `TOKEN_USER`, in a buffer of `u64`s so that it is suitably aligned.
    struct TokenUserBuf(Vec<u64>);

    // The buffer's element alignment must cover TOKEN_USER's, so reading it in place is aligned.
    const _: () = assert!(std::mem::align_of::<TOKEN_USER>() <= std::mem::align_of::<u64>());

    impl TokenUserBuf {
        fn current() -> io::Result<Self> {
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
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` holds at least `len` bytes.
            if unsafe {
                GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(buf))
        }

        /// The user's SID. It points into the buffer, so it is valid while `self` lives.
        fn sid(&self) -> PSID {
            // SAFETY: the buffer starts with a TOKEN_USER written by GetTokenInformation, and is aligned
            // for it (see the assertion above).
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    /// The current token user's SID, as `S-1-5-...`.
    fn current_user_sid() -> io::Result<String> {
        let user = TokenUserBuf::current()?;
        let mut s: *mut u16 = null_mut();
        // SAFETY: the SID is valid while `user` lives; `s` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(user.sid(), &mut s) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _s = LocalBox(s.cast());
        // SAFETY: the API returned a NUL-terminated string.
        Ok(unsafe { from_wide(s) })
    }

    fn private_sddl(dir: bool) -> io::Result<String> {
        let sid = current_user_sid()?;
        // P: protected, nothing inherited from the parent. FA: full access. OICI: inherited by children.
        Ok(if dir {
            format!("D:P(A;OICI;FA;;;{sid})")
        } else {
            format!("D:P(A;;FA;;;{sid})")
        })
    }

    fn descriptor(sddl: &str) -> io::Result<LocalBox> {
        let w = wide(OsStr::new(sddl));
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

    /// Refuses a volume that can't store ACLs (FAT, exFAT): a DACL set there is silently dropped.
    fn require_persistent_acls(path: &Path) -> io::Result<()> {
        let abs = std::path::absolute(path)?;
        // The object doesn't exist yet; its parent directory does.
        let probe = abs.parent().unwrap_or(&abs);
        let w = wide(probe.as_os_str());
        let mut volume = vec![0u16; 32_768];
        // SAFETY: `w` is NUL-terminated; `volume` is writable for the length passed.
        if unsafe { GetVolumePathNameW(w.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut flags = 0u32;
        let mut fs = [0u16; 261];
        // SAFETY: `volume` was NUL-terminated by the API; `fs` is writable for the length passed; the
        // other out pointers are valid or null with a zero length.
        if unsafe {
            GetVolumeInformationW(
                volume.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if flags & FILE_PERSISTENT_ACLS != 0 {
            return Ok(());
        }
        // SAFETY: both buffers were NUL-terminated by the API.
        let (volume, fs) = unsafe { (from_wide(volume.as_ptr()), from_wide(fs.as_ptr())) };
        Err(super::no_access_control(format_args!(
            "the {fs} volume {volume} doesn't store access control lists"
        )))
    }

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        require_persistent_acls(path)?;
        create_file_with(path, &private_sddl(false)?)
    }

    /// Creates a new file with the given security descriptor. Tests use it for files that aren't private.
    pub(super) fn create_file_with(path: &Path, sddl: &str) -> io::Result<File> {
        let sd = descriptor(sddl)?;
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

    #[cfg(test)]
    pub(super) fn sid_for_tests() -> String {
        current_user_sid().expect("sid")
    }

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        require_persistent_acls(path)?;
        let sd = descriptor(&private_sddl(true)?)?;
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

    /// The DACL of `path`, with the descriptor that owns it. `dacl` points into `_sd`.
    struct Dacl {
        _sd: LocalBox,
        sd: PSECURITY_DESCRIPTOR,
        dacl: *mut ACL,
    }

    fn read_dacl(path: &Path) -> io::Result<Dacl> {
        // Reading the DACL needs only READ_CONTROL, which the owner always has. Backup semantics lets the
        // same call open directories.
        let file = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let mut dacl: *mut ACL = null_mut();
        // SAFETY: the handle is open with READ_CONTROL; out pointers are valid or null.
        let rc = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(Dacl {
            _sd: LocalBox(sd),
            sd,
            dacl,
        })
    }

    pub fn describe_access(path: &Path) -> io::Result<String> {
        let d = read_dacl(path)?;
        let mut s: *mut u16 = null_mut();
        // SAFETY: `d.sd` is a valid descriptor; `s` is a valid out pointer.
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                d.sd,
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

    pub fn verify_private(path: &Path, kind: Private) -> io::Result<()> {
        let fail = |why: String| {
            let sddl = describe_access(path).unwrap_or_default();
            Err(super::not_private(path, format_args!("{why} ({sddl})")))
        };
        let d = read_dacl(path)?;
        if d.dacl.is_null() {
            return fail("there is no DACL, so everyone has access".into());
        }
        let (mut control, mut revision) = (0u16, 0u32);
        // SAFETY: `d.sd` is a valid descriptor; out pointers are valid.
        if unsafe { GetSecurityDescriptorControl(d.sd, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let protected = control & SE_DACL_PROTECTED != 0;
        let mut info = ACL_SIZE_INFORMATION::default();
        // SAFETY: `d.dacl` is a valid ACL; `info` is writable for the size passed.
        if unsafe {
            GetAclInformation(
                d.dacl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if info.AceCount != 1 {
            return fail(format!("{} ACEs, expected 1", info.AceCount));
        }
        let mut ace: *mut c_void = null_mut();
        // SAFETY: index 0 exists (AceCount is 1); `ace` is a valid out pointer.
        if unsafe { GetAce(d.dacl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every ACE starts with an ACE_HEADER; read unaligned so alignment doesn't matter.
        let header = unsafe { std::ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            return fail(format!("ACE type {}, expected allow", header.AceType));
        }
        // SAFETY: an allow ACE is an ACCESS_ALLOWED_ACE followed by the rest of its SID.
        let allowed = unsafe { std::ptr::read_unaligned(ace.cast::<ACCESS_ALLOWED_ACE>()) };
        if allowed.Mask & FILE_ALL_ACCESS != FILE_ALL_ACCESS {
            return fail(format!(
                "access mask {:#x}, expected full access",
                allowed.Mask
            ));
        }
        // SAFETY: the SID starts at SidStart, inside the ACE.
        let sid: PSID = unsafe {
            ace.cast::<u8>()
                .add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart))
                .cast()
        };
        let me = TokenUserBuf::current()?;
        // SAFETY: both SIDs are valid: one inside the DACL, one inside `me`.
        if unsafe { EqualSid(sid, me.sid()) } == 0 {
            return fail("access is granted to another account".into());
        }
        let flags = u32::from(header.AceFlags);
        let ok = match kind {
            Private::File => protected && flags == 0,
            Private::Dir => protected && flags == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
            Private::AnyFile => {
                flags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE) == 0
                    && (protected || flags & INHERITED_ACE != 0)
            }
        };
        if !ok {
            return fail(format!(
                "unexpected ACE flags {flags:#x} (protected: {protected}) for {kind:?}"
            ));
        }
        Ok(())
    }

    pub fn is_reparse_point(metadata: &Metadata) -> bool {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn private_file_and_dir_are_owner_only_and_never_overwrite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("private");
        create_private_dir(&dir).expect("dir");
        verify_private(&dir, Private::Dir).expect("private dir");
        assert!(create_private_dir(&dir).is_err());

        let file = dir.join("secret");
        create_private_file(&file)
            .expect("file")
            .write_all(b"x")
            .expect("write");
        verify_private(&file, Private::File).expect("private file");
        verify_private(&file, Private::AnyFile).expect("private file");
        assert_eq!(
            create_private_file(&file).expect_err("exists").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&file).expect("read"), b"x");

        // On Windows, a file created normally inside the private directory inherits its protection.
        #[cfg(windows)]
        {
            let inner = dir.join("inner");
            std::fs::write(&inner, b"y").expect("write");
            verify_private(&inner, Private::AnyFile).expect("inherited");
            assert!(verify_private(&inner, Private::File).is_err());
        }
    }

    #[test]
    fn broader_access_is_not_reported_private() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let not_private = |path: &Path, kind| {
            let err = verify_private(path, kind).expect_err("not private");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        };
        // A private directory is not a private file, and the other way round.
        let dir = tmp.path().join("d");
        create_private_dir(&dir).expect("dir");
        not_private(&dir, Private::File);
        let file = dir.join("f");
        create_private_file(&file).expect("file");
        not_private(&file, Private::Dir);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let open = tmp.path().join("open");
            std::fs::write(&open, b"x").expect("write");
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).expect("chmod");
            not_private(&open, Private::AnyFile);
        }
        #[cfg(windows)]
        {
            let me = imp::sid_for_tests();
            let cases = [
                // Everyone may also read it.
                format!("D:P(A;;FA;;;{me})(A;;FR;;;WD)"),
                // Granted to someone else (Everyone) only.
                "D:P(A;;FA;;;WD)".to_string(),
                // Only read access for the user.
                format!("D:P(A;;FR;;;{me})"),
                // An extra (deny) ACE beside the user's.
                format!("D:P(D;;FW;;;BG)(A;;FA;;;{me})"),
            ];
            for (i, sddl) in cases.iter().enumerate() {
                let path = tmp.path().join(format!("case-{i}"));
                drop(imp::create_file_with(&path, sddl).expect("create"));
                not_private(&path, Private::File);
                not_private(&path, Private::AnyFile);
            }
        }
    }

    #[test]
    fn missing_access_control_is_a_clear_unsupported_error() {
        let e = no_access_control("the exFAT volume E:\\ doesn't store access control lists");
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        let text = e.to_string();
        assert!(text.contains("exFAT"), "{text}");
        assert!(text.contains("can't make files there owner-only"), "{text}");
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
