# Pairing devices from a signed-in app

In desktop account settings, web Device settings, or Android drawer Settings, choose **Pair a new
device** and then **Create invite**. On Android, tap **Scan invite QR code** on the signed-out screen
and point the camera at the invite. On desktop, paste the link into **Invite code or link**. To type
an invite code instead, also enter the server address. In a new browser, open the link, name the
browser, and register a passkey. No existing passkey is needed on that browser.

Keep the invite card open to see what signed in. **Cancel invite** makes both the code and link
unusable unless redemption has already won. The owner can see native devices and browser passkeys
under **Devices** in the server's admin pages. Share an invite only with a device you're setting up:
anyone with its code or link can sign in as the owner while it is valid.

If the new app is already showing a pairing request instead, choose **Approve a device** on an
existing signed-in app. Enter its code (or scan its QR on Android), check the device name, then
confirm. The old browser approval link still works. Android uses an on-device ZXing decoder and
requests camera permission only when the Scan button is used. Typing works without that permission.

## Protocol and security

- Only bearer tokens from actual paired devices and web sessions can manage invites or review
  pending pairings. Issuing and approving require a fresh passkey verification on web. Every
  cookie-authenticated API request checks `X-CSRF-Token`, and writes also check `Origin`. Admin API,
  agent, ingest and development owner tokens cannot hand out credentials through these routes.
- Invites are kept in memory, expire within five minutes and disappear on restart. A link carries an
  independently generated 256-bit secret in its fragment; a native client can instead use a
  12-character random code from the unambiguous alphabet. The server stores the SHA-256 digests of
  both and compares fixed-size digests. The first successful claim invalidates both. There are at
  most 32 live records; creation and redemption are limited by address, and issuer status uses the
  polling rate limit. The issuer must still be signed in or paired when redemption is committed.
- The server's logs use route templates, not raw URLs or request bodies. The browser strips the
  fragment from its history entry before starting registration. Audit records identify the issuer
  (device ID or session row ID) and the new device's name, not the invite, passkey or token.
- Browser registration uses the same WebAuthn options and verification as adding a passkey. The
  browser's separate one-time ceremony handle is also stored hashed and can't outlive the invite.
  Starting a ceremony does not consume the invite: cancellation or native redemption can still win.
  The passkey and session are saved together. Browser start and finish require the site's `Origin`;
  they do not authorize with a cookie. A browser invite does not create a bearer token.

The public OpenAPI document describes the JSON pairing API:

| Method and path | Purpose |
|---|---|
| `POST /api/v1/pair/invites` | Create an invite (`id`, `code`, `link`, `expires_in`). |
| `GET /api/v1/pair/invites/{id}` | Issuer-only status: `pending`, `used` with `device_name`, or `cancelled`. |
| `DELETE /api/v1/pair/invites/{id}` | Cancel an unused invite, from its issuing device or session. |
| `POST /api/v1/pair/redeem` | Send `secret` (typed code or fragment) and `device_name`; store the returned `device_id` and `token` as for normal pairing. |
| `POST /api/v1/pair/review` | Send `code` to preview the pending device; send `code` and its returned `created_at` as `approve_at` to confirm. |

Browser links open `/pair/invite`; the page submits the fragment secret and device name in the JSON
body of `POST /auth/invite/start`, then sends the ceremony handle and attestation to
`POST /auth/invite/finish`. Do not put the invite code or link secret in a URL path or query.
