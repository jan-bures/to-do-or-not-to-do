//! Android bindings for the shared invitation protocol and QR matrix.

use crate::FfiError;
use crate::pairing::normalize_server_address;
use crate::runtime::runtime;
use todo_client::device_pairing::{self as pair, InviteStatus, Qr};
use todo_client::native::{Cleartext, HttpTransport};
use todo_client::pairing::PairError;

fn transport(
    server: &str,
    token: Option<&str>,
    allow_http: bool,
) -> Result<HttpTransport, FfiError> {
    let cleartext = if allow_http {
        Cleartext::Loopback
    } else {
        Cleartext::Refuse
    };
    HttpTransport::with_cleartext(server, token, cleartext).map_err(|e| FfiError::BadAddress {
        reason: e.to_string(),
    })
}

fn refused(e: PairError) -> FfiError {
    match e {
        PairError::Expired | PairError::Refused { status: 404, .. } => FfiError::Expired,
        PairError::Transport(e) => e.into(),
        PairError::Refused {
            status,
            code,
            retry_after,
            ..
        } => FfiError::Refused {
            status,
            code,
            retry_after_secs: retry_after.map(|d| d.as_secs()),
            detail: "Couldn't complete device pairing. Try again.".into(),
        },
        PairError::Protocol(_) => FfiError::Protocol {
            detail: "Invalid device pairing reply.".into(),
        },
    }
}

/// A rendered QR matrix from the shared encoder, including the four-module quiet zone.
#[allow(missing_debug_implementations)]
#[derive(uniffi::Record)]
pub struct PairingQr {
    /// Square width in modules.
    pub width: u32,
    /// Row-major dark modules.
    pub dark: Vec<bool>,
}

/// Encodes a pairing link without a network or a platform dependency.
#[uniffi::export]
#[must_use]
pub fn pairing_qr(link: String) -> Option<PairingQr> {
    let qr = Qr::encode(&link)?;
    Some(PairingQr {
        width: u32::try_from(qr.width).ok()?,
        dark: qr.dark,
    })
}

/// Code and link displayed once, only to their issuer. Do not log them.
#[allow(missing_debug_implementations)]
#[derive(uniffi::Record)]
pub struct DeviceInvitation {
    /// Identifier for status and cancellation.
    pub id: String,
    /// Typed code.
    pub code: String,
    /// Link with the secret in the fragment.
    pub link: String,
    /// Remaining seconds.
    pub expires_in: u32,
}

/// Creates an invite from a paired Android phone. Blocking.
#[uniffi::export]
pub fn device_invite_create(
    server: String,
    token: String,
    allow_http: bool,
) -> Result<DeviceInvitation, FfiError> {
    let t = transport(&server, Some(&token), allow_http)?;
    let i = runtime()?
        .block_on(pair::create(&t, &server))
        .map_err(refused)?;
    Ok(DeviceInvitation {
        id: i.id,
        code: i.code,
        link: i.link,
        expires_in: i.expires_in,
    })
}

/// A pending, used or cancelled device invitation.
#[derive(Debug, uniffi::Enum)]
pub enum DeviceInviteStatus {
    /// Still waiting for the other device.
    Pending,
    /// The invite was cancelled.
    Cancelled,
    /// The invite was used.
    Used {
        /// Name chosen on the new device.
        device_name: String,
    },
}

/// Reads invite status. Blocking.
#[uniffi::export]
pub fn device_invite_status(
    server: String,
    token: String,
    id: String,
    allow_http: bool,
) -> Result<DeviceInviteStatus, FfiError> {
    let t = transport(&server, Some(&token), allow_http)?;
    Ok(
        match runtime()?
            .block_on(pair::status(&t, &id))
            .map_err(refused)?
        {
            InviteStatus::Pending => DeviceInviteStatus::Pending,
            InviteStatus::Cancelled => DeviceInviteStatus::Cancelled,
            InviteStatus::Used { device_name } => DeviceInviteStatus::Used { device_name },
        },
    )
}

/// The protocol's polling interval in milliseconds.
#[uniffi::export]
#[must_use]
pub fn device_invite_poll_ms() -> u64 {
    pair::STATUS_INTERVAL.as_secs() * 1000
}

/// Cancels an unused invitation. Blocking.
#[uniffi::export]
pub fn device_invite_cancel(
    server: String,
    token: String,
    id: String,
    allow_http: bool,
) -> Result<(), FfiError> {
    let t = transport(&server, Some(&token), allow_http)?;
    runtime()?.block_on(pair::cancel(&t, &id)).map_err(refused)
}

/// A new pairing to seal and persist using the normal pairing path.
#[allow(missing_debug_implementations)]
#[derive(uniffi::Record)]
pub struct RedeemedInvite {
    /// The verified origin.
    pub server: String,
    /// Device ID.
    pub device_id: String,
    /// The bearer token, shown once. Do not log it.
    pub token: String,
}

/// Redeems an entered code (on `server`) or link (on the link's origin). Blocking.
#[uniffi::export]
pub fn device_invite_redeem(
    server: String,
    input: String,
    device_name: String,
    allow_http: bool,
) -> Result<RedeemedInvite, FfiError> {
    let input = pair::invite_input(&input).map_err(|e| FfiError::Invalid {
        detail: e.to_string(),
    })?;
    let server = normalize_server_address(input.origin.unwrap_or(server), allow_http)?;
    let t = transport(&server, None, allow_http)?;
    let r = runtime()?
        .block_on(pair::redeem(&t, &input.secret, &device_name))
        .map_err(refused)?;
    Ok(RedeemedInvite {
        server,
        device_id: r.device_id.to_string(),
        token: r.token,
    })
}

/// The device requesting approval.
#[derive(Debug, uniffi::Record)]
pub struct DevicePairingReview {
    /// Name supplied by the new device.
    pub device_name: String,
    /// Send this timestamp back to approve this exact request.
    pub created_at: i64,
}

/// Previews or approves a pending pairing. The scanned link cannot change the paired server. Blocking.
#[uniffi::export]
pub fn device_pairing_review(
    server: String,
    token: String,
    input: String,
    approve_at: Option<i64>,
    allow_http: bool,
) -> Result<DevicePairingReview, FfiError> {
    let code = pair::approval_code(&input, &server).map_err(|e| FfiError::Invalid {
        detail: e.to_string(),
    })?;
    let t = transport(&server, Some(&token), allow_http)?;
    let r = runtime()?
        .block_on(pair::review(&t, &code, approve_at))
        .map_err(refused)?;
    Ok(DevicePairingReview {
        device_name: r.device_name,
        created_at: r.created_at,
    })
}
