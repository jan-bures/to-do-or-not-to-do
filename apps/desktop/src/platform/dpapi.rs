//! Windows DPAPI (`CryptProtectData`) for the device token: encrypted with a key derived from the
//! current user's logon credentials, so only this user on this machine can decrypt it. No admin rights,
//! no prompts.
#![allow(unsafe_code)] // Two plain FFI calls with checked results; buffers are owned by the OS and freed once.

use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Extra entropy mixed into the key, so another app running as the same user can't decrypt the blob
/// with a plain `CryptUnprotectData` call. Not a secret by itself.
const ENTROPY: &[u8] = b"to-do-or-not-to-do/device-token/v1";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).unwrap_or(u32::MAX),
        pbData: data.as_ptr().cast_mut(),
    }
}

/// Takes the OS-allocated output blob, copies it out and frees it.
fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if out.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: on success the API returns `cbData` bytes at `pbData`, allocated with LocalAlloc.
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    // SAFETY: freed exactly once, with the allocator the API documents.
    unsafe { LocalFree(out.pbData as HLOCAL) };
    bytes
}

/// Encrypts `plain` for the current user.
pub fn protect(plain: &[u8]) -> std::io::Result<Vec<u8>> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: the input blobs point at live slices for the duration of the call; `out` is written by
    // the API.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(take(out))
}

/// Decrypts a blob from [`protect`]. Fails for another user, another machine, or tampered data.
pub fn unprotect(sealed: &[u8]) -> std::io::Result<Vec<u8>> {
    let input = blob(sealed);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(take(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_hides_the_plaintext() {
        let secret = b"device-token-for-a-test-0123456789";
        let sealed = protect(secret).expect("protect");
        assert!(!sealed.windows(secret.len()).any(|w| w == secret));
        assert_eq!(unprotect(&sealed).expect("unprotect"), secret);
        // Tampering is detected.
        let mut bad = sealed.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(unprotect(&bad).is_err());
        assert!(unprotect(b"not a dpapi blob").is_err());
    }
}
