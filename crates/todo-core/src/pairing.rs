//! Shared device-pairing protocol constants and validation (no I/O).

/// Page used to approve a pairing started by a new client.
pub const APPROVAL_PAGE: &str = "/pair";
/// Page used to redeem an invitation. Its secret is in the fragment, never the query string.
pub const INVITE_PAGE: &str = "/pair/invite";
/// Unambiguous alphabet for codes read aloud or typed.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Characters in the pending pairing code, excluding separators.
pub const APPROVAL_CODE_LEN: usize = 8;
/// Characters in the typed invite code, excluding separators.
pub const INVITE_CODE_LEN: usize = 12;
/// Unpadded base64url length of a 256-bit link secret.
pub const INVITE_SECRET_LEN: usize = 43;

/// Uppercase a typed code, ignoring whitespace and dashes.
#[must_use]
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Normalizes a typed code and rejects incorrect lengths or characters.
#[must_use]
pub fn typed_code(input: &str, len: usize) -> Option<String> {
    if input.len() > 64 {
        return None;
    }
    let code = normalize_code(input);
    (code.len() == len && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

/// Checks the shape of a secret, not whether it is an active invitation.
#[must_use]
pub fn link_secret(input: &str) -> bool {
    input.len() == INVITE_SECRET_LEN
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
