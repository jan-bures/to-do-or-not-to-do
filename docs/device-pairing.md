# Pairing devices from a signed-in app

In the sidebar or Android drawer footer, open **Settings → Devices…**. Choose **Pair a new device**
and then **Create invite**. On Android, tap **Scan invite QR code** on the signed-out screen
and point the camera at the invite. On desktop, paste the link into **Invite code or link**. If the
address field is blank, check the host and tap **Pair with &lt;host&gt;** before the app contacts it.
If you've entered an address, a link for a different origin is refused rather than replacing it.
To type an invite code instead, also enter the server address. In a new browser, open the link, name the
browser, and register a passkey. No existing passkey is needed on that browser.

**Close** dismisses an invite without cancelling it, even when offline. Reopen the card to see the
live invite. **Cancel invite** makes the code and link unusable unless redemption has already won.
On Android, the invite code and QR are hidden from screenshots and the recent-apps thumbnail. If
Android receives a credential but can't save it, **Try saving again** retries the local save without
using another invite. Keep the app open: the unsaved credential is only in memory. **Discard and
start over** drops it. Check **Devices** on another signed-in device before pairing again: the phone
may already be listed there even though it could not save its sign-in.

The owner can see native devices and browser passkeys under **Devices** in the server's admin
pages, including the issuing device or session. Share an invite only with a device you're setting
up: anyone with the code or link can sign in as the owner while it is valid. Revoking a device signs
out the other devices and browsers it added, including sign-ins added through them. **Devices** shows
how many other sign-ins this affects. Revoking a passkey also ends its sessions and revokes sign-ins
it issued, directly or indirectly. A newer passkey cannot revoke an older one, even by revoking a
paired device. To revoke a passkey, verify with one registered before it. The oldest passkey can
never be revoked through the UI: there is no earlier passkey to verify with.

If a device cascade is blocked and you cannot verify with the required passkey, revoke the issuing
device from the server host with the [local admin token](admin-api.md#examples). That path is not
subject to passkey ordering. Neither the admin token nor `todo-server` can directly revoke the first
setup passkey. Keep a recoverable copy of that passkey. If it is compromised, the host-side recovery
option is [restoring a trusted backup](hosting-linux.md#putting-a-backup-into-service) that does not
contain it, if one is available. A restore rolls back data as well as credentials.

Agent tokens and share links created from a derived session are not removed by the cascade. Review
and revoke them separately under **Agents** and **Shares**, or through the local admin API.

If the new app is already showing a pairing request instead, choose **Approve a device** on an
existing signed-in app. Enter its code (or scan its QR on Android), then check the device name,
grouped code and source address before confirming. The old browser approval link still works. Android uses an on-device ZXing decoder and
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
  Credentials expire on time; a redemption that began before expiry may still report its terminal
  result to the issuer for up to one minute after the deadline. Clients hide the credentials at the
  deadline and continue checking the result. If retention ends before the app can confirm the outcome
  (for example, because it was asleep or a request stalled), check **Devices** rather than assuming
  the invite expired unused. Wrong-code lookups share a per-session budget with the old browser page;
  paired native devices have a per-device budget with a five-minute cooldown.
- Approved pairings retain the approving device or session. The server rechecks it when the new
  device polls, before issuing a token. Revoking a device or passkey updates its native descendants,
  removes descendant browser passkeys and their sessions, and records the descendant device IDs and
  removed passkey and session counts in one transaction. Browser-issued descendants preserve their
  device and passkey ancestry after the issuing session ends. Upgrading does not infer passkey
  ancestry for credentials that were already issued. Signing out of a browser invalidates its
  outstanding invites and approvals without revoking past joins.
- The server's logs use route templates, not raw URLs or request bodies. The browser strips the
  fragment from its history entry before starting registration. Audit records include invite creation
  and cancellation and identify the issuer (device or session row), not the invite, passkey or token.
- Browser registration uses the same WebAuthn options and verification as adding a passkey. The
  browser's separate one-time ceremony handle is also stored hashed and can't outlive the invite.
  Starting a ceremony does not consume the invite: cancellation or native redemption can still win.
  The passkey and session are saved together. A session created this way is signed in but not fresh:
  step-up actions such as issuing another invite require a successful passkey assertion. Browser start
  and finish require the site's `Origin`; they do not authorize with a cookie. A browser invite does
  not create a bearer token. Upgrading from a schema that did not link sessions to their passkeys
  ends those existing sessions; sign in again to continue.

The public OpenAPI document describes the JSON pairing API:

| Method and path | Purpose |
|---|---|
| `POST /api/v1/pair/invites` | Create an invite (`id`, `code`, `link`, `expires_in`). |
| `GET /api/v1/pair/invites/{id}` | Issuer-only status: `pending`, `used` with `device_name`, or `cancelled`. |
| `DELETE /api/v1/pair/invites/{id}` | Cancel an unused invite, from its issuing device or session. |
| `POST /api/v1/pair/redeem` | Send `secret` (typed code or fragment) and `device_name`; store the returned `device_id` and `token` as for normal pairing. |
| `POST /api/v1/pair/review` | Send `code` to preview the pending device and its `source` address (or `null`); send `code` and its returned `created_at` as `approve_at` to confirm. |

Browser links open `/pair/invite`; the page submits the fragment secret and device name in the JSON
body of `POST /auth/invite/start`, then sends the ceremony handle and attestation to
`POST /auth/invite/finish`. Do not put the invite code or link secret in a URL path or query.
