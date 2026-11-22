//! Clients' shared invite and approval operations. Credentials go in request bodies, never in paths.

use crate::pairing::{PairError, body, parse, refused};
use crate::platform::{Method, Request, Transport};
use serde::Deserialize;
use serde_json::json;
use std::{fmt, time::Duration};
use todo_core::DeviceId;
use todo_core::pairing::{
    APPROVAL_CODE_LEN, APPROVAL_PAGE, INVITE_CODE_LEN, INVITE_PAGE, link_secret, typed_code,
};
use url::Url;

const INVITES: &str = "/api/v1/pair/invites";
const REVIEW: &str = "/api/v1/pair/review";
const REDEEM: &str = "/api/v1/pair/redeem";

/// Delay between status checks (not the countdown refresh interval).
pub const STATUS_INTERVAL: Duration = Duration::from_secs(5);

/// An invitation displayed on the issuer only. Its link and code grant access; do not log them.
#[derive(Clone, Deserialize)]
pub struct Invitation {
    /// Non-secret identifier for status and cancellation.
    pub id: String,
    /// Short code for typing on a native client.
    pub code: String,
    /// Canonical invite link including the fragment.
    pub link: String,
    /// Remaining lifetime in seconds when created.
    pub expires_in: u32,
}

impl fmt::Debug for Invitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invitation")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Invitation status as seen by its issuer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InviteStatus {
    /// Waiting for a new device.
    Pending,
    /// The invite was redeemed.
    Used {
        /// The name chosen on the new device.
        device_name: String,
    },
    /// The invite was cancelled.
    Cancelled,
}

/// The credentials received when redeeming a native invite.
#[derive(Clone, Deserialize)]
pub struct Credential {
    /// Device ID.
    pub device_id: DeviceId,
    /// Bearer token; store as for a normal approved pairing.
    pub token: String,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// A pending device, for the owner to review before approving it.
#[derive(Clone, Debug, Deserialize)]
pub struct PendingDevice {
    /// The name supplied by the caller.
    pub device_name: String,
    /// Binds approval to the request the owner saw.
    pub created_at: i64,
}

/// An invite as entered by the owner. A link can supply a server; a code needs a separate address.
#[derive(Clone)]
pub struct InviteInput {
    /// Canonical origin if the input was an invite link. Apply local HTTPS policy before use.
    pub origin: Option<String>,
    /// Secret or normalized typed invite code.
    pub secret: String,
}

impl fmt::Debug for InviteInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteInput")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// The entered link or code was malformed or did not match the paired server.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("Enter a valid code or link for this server.")]
pub struct InvalidCode;

fn safe_url(input: &str) -> Result<Url, InvalidCode> {
    if input.len() > 1024 {
        return Err(InvalidCode);
    }
    let url = Url::parse(input.trim()).map_err(|_| InvalidCode)?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(InvalidCode);
    }
    Ok(url)
}

/// Parse an invite code or link. Never log the input.
pub fn invite_input(input: &str) -> Result<InviteInput, InvalidCode> {
    if let Some(code) = typed_code(input, INVITE_CODE_LEN) {
        return Ok(InviteInput {
            origin: None,
            secret: code,
        });
    }
    let url = safe_url(input)?;
    let secret = url
        .fragment()
        .filter(|s| link_secret(s))
        .ok_or(InvalidCode)?;
    if url.path() != INVITE_PAGE || url.query().is_some() {
        return Err(InvalidCode);
    }
    Ok(InviteInput {
        origin: Some(url.origin().ascii_serialization()),
        secret: secret.to_owned(),
    })
}

/// Parse a pending code or QR link for the already paired server. A QR scan never changes servers.
pub fn approval_code(input: &str, server: &str) -> Result<String, InvalidCode> {
    if let Some(code) = typed_code(input, APPROVAL_CODE_LEN) {
        return Ok(code);
    }
    let url = safe_url(input)?;
    if url.path() != APPROVAL_PAGE
        || url.fragment().is_some()
        || url.origin() != safe_url(server)?.origin()
    {
        return Err(InvalidCode);
    }
    let mut pairs = url.query_pairs();
    let (k, v) = pairs.next().ok_or(InvalidCode)?;
    if k != "code" || pairs.next().is_some() {
        return Err(InvalidCode);
    }
    typed_code(&v, APPROVAL_CODE_LEN).ok_or(InvalidCode)
}

/// Creates an invitation from an authenticated, paired transport. Refuse links to other origins.
pub async fn create<T: Transport>(transport: &T, server: &str) -> Result<Invitation, PairError> {
    let invite: Invitation = parse(&transport.send(Request::post(INVITES, vec![])).await?)?;
    let expected = safe_url(server)
        .ok()
        .map(|url| url.origin().ascii_serialization());
    let parsed = invite_input(&invite.link).map_err(|_| bad_response())?;
    if expected.is_none()
        || parsed.origin != expected
        || typed_code(&invite.code, INVITE_CODE_LEN).is_none()
    {
        return Err(bad_response());
    }
    Ok(invite)
}

fn bad_response() -> PairError {
    PairError::Protocol("invalid pairing response".into())
}

fn invite_path(id: &str) -> Result<String, PairError> {
    if id.len() != 22
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(bad_response());
    }
    Ok(format!("{INVITES}/{id}"))
}

/// Checks invite status, with the issuing client's authenticated transport.
pub async fn status<T: Transport>(transport: &T, id: &str) -> Result<InviteStatus, PairError> {
    parse(&transport.send(Request::get(invite_path(id)?)).await?)
}

/// Cancels an unused invite.
pub async fn cancel<T: Transport>(transport: &T, id: &str) -> Result<(), PairError> {
    let res = transport
        .send(Request {
            method: Method::Delete,
            path: invite_path(id)?,
            body: None,
        })
        .await?;
    if res.status == 204 {
        Ok(())
    } else {
        Err(refused(&res))
    }
}

/// Redeems a code or fragment on the selected server, with an anonymous transport.
pub async fn redeem<T: Transport>(
    transport: &T,
    secret: &str,
    name: &str,
) -> Result<Credential, PairError> {
    parse(
        &transport
            .send(Request::post(
                REDEEM,
                body(&json!({ "secret": secret, "device_name": name }))?,
            ))
            .await?,
    )
}

/// Previews or confirms the pending pairing. Confirm with the preview's timestamp.
pub async fn review<T: Transport>(
    transport: &T,
    code: &str,
    approve_at: Option<i64>,
) -> Result<PendingDevice, PairError> {
    parse(
        &transport
            .send(Request::post(
                REVIEW,
                body(&json!({ "code": code, "approve_at": approve_at }))?,
            ))
            .await?,
    )
}

/// A black-on-light QR matrix with its required four-module quiet zone included.
#[derive(Clone)]
pub struct Qr {
    /// Square tile width in modules, including its quiet zone.
    pub width: usize,
    /// Row-major dark modules; true is dark. Includes quiet zone pixels.
    pub dark: Vec<bool>,
}

impl fmt::Debug for Qr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Qr")
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl Qr {
    /// Encodes the exact link shown on the card. No platform or image library required.
    #[must_use]
    pub fn encode(link: &str) -> Option<Self> {
        if link.len() > 1024 {
            return None;
        }
        let qr = qrcode::QrCode::new(link.as_bytes()).ok()?;
        const QUIET: usize = 4;
        let width = qr.width() + QUIET * 2;
        let mut dark = vec![false; width * width];
        for y in 0..qr.width() {
            for x in 0..qr.width() {
                dark[(y + QUIET) * width + x + QUIET] = qr[(x, y)] == qrcode::Color::Dark;
            }
        }
        Some(Self { width, dark })
    }

    /// Integer-coordinate SVG path data for the dark modules. Contains no link or markup.
    #[must_use]
    pub fn path(&self) -> String {
        use fmt::Write as _;
        let mut path = String::new();
        for (idx, _) in self.dark.iter().enumerate().filter(|(_, dark)| **dark) {
            let _ = write!(path, "M{} {}h1v1h-1z", idx % self.width, idx / self.width);
        }
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_strict_and_codes_are_normalized() {
        let server = "https://todo.example.com";
        let secret = "a".repeat(43);
        let invite = invite_input(&format!("{server}/pair/invite#{secret}")).unwrap();
        assert_eq!(invite.origin.as_deref(), Some(server));
        assert_eq!(invite.secret, secret);
        assert_eq!(
            invite_input(" abcdef-ghjkmn ").unwrap().secret,
            "ABCDEFGHJKMN"
        );
        assert!(invite_input(&format!("{server}/pair/invite?secret={secret}")).is_err());
        assert!(
            invite_input(&format!(
                "https://user@todo.example.com/pair/invite#{secret}"
            ))
            .is_err()
        );
        assert_eq!(
            approval_code(&format!("{server}/pair?code=abcd-efgh"), server).unwrap(),
            "ABCDEFGH"
        );
        assert!(approval_code("https://other.example.com/pair?code=abcd-efgh", server).is_err());
        assert!(
            approval_code(
                &format!("{server}/pair?code=abcd-efgh&code=abcd-efgj"),
                server
            )
            .is_err()
        );
    }

    #[test]
    fn qr_includes_a_full_quiet_zone() {
        let qr = Qr::encode("https://todo.example.com/pair?code=ABCD-EFGH").unwrap();
        assert!(qr.dark.iter().any(|b| *b));
        for (i, &dark) in qr.dark.iter().enumerate() {
            let (x, y) = (i % qr.width, i / qr.width);
            if x < 4 || y < 4 || x >= qr.width - 4 || y >= qr.width - 4 {
                assert!(!dark);
            }
        }
    }
}
