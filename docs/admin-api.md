# The local admin API

Routine admin tasks without the owner's passkey: email ingest settings, agent grants and quotas,
revoking agents, devices and share links, and reading the audit log. It's meant for an operator on
the server's own machine, a script or an AI agent, that the owner trusts with those tasks but not
with their passkey or their tasks.

It is **local only**. It runs on a second listener that binds to a loopback address, and it is never
routed through the tunnel or proxy that serves the public site. The public listener doesn't serve
`/api/v1/admin` at all; it answers 404 there for every method and credential.

```text
operator on the server ──► 127.0.0.1:47321  admin listener   /api/v1/admin/...   (tdx_ token)
browsers, apps, agents ──► tunnel ──► 127.0.0.1:47320  public listener   everything else
                                                     (/api/v1/admin → 404)
```

## Turning it on

1. **Make a token.** Run this as the account the server runs as. It works while the server is
   running, because it touches neither the database nor the config:

   ```sh
   todo-server admin-token --out ~/.config/todo/admin-token
   ```

   It writes a new random `tdx_...` token to that file, readable only by your account, and refuses
   to overwrite an existing file. It prints the config lines with the token's SHA-256:

   ```toml
   [admin]
   bind = "127.0.0.1:47321"
   token_sha256 = "<64 hex digits>"
   ```

2. **Add them to `server.toml`** and restart the server. `bind` must be a loopback address
   (`127.0.0.1`, another `127.x.y.z`, or `[::1]`), on a different port from the public `bind`. The
   server refuses to start otherwise, and also when `bind` is set without `token_sha256`. You can use
   the environment variables `TODO_SERVER_ADMIN_BIND` and `TODO_SERVER_ADMIN_TOKEN_SHA256` instead.

3. **Keep the port off the tunnel.** Route only the public port through `cloudflared` or any other
   proxy. The admin listener ignores `CF-Connecting-IP` and `X-Forwarded-For` whatever
   `trust_cloudflare_headers` says; the caller's address in the audit log is always the TCP peer.

**Rotating** means running `admin-token` with a new file, putting the new hash in the config and
restarting. The old token stops working with the restart. **Turning it off** means removing `bind`.

Optional limits, all across every caller:

```toml
[admin]
requests_per_minute = 120   # authenticated requests
request_burst = 60
failures_per_minute = 10    # wrong or missing tokens; past it, every request gets 429 for a while
failure_burst = 10
```

## Calling it

Every request needs `Authorization: Bearer <token>`, and nothing else is accepted: no query
parameter, no cookie, no other header. Device, agent and owner tokens are refused here (401), and
the admin token is refused everywhere on the public listener (401, or 404 under `/api/v1/admin`).

```sh
TOKEN="$(cat ~/.config/todo/admin-token)"
ADMIN=http://127.0.0.1:47321
curl -sS -H "Authorization: Bearer $TOKEN" "$ADMIN/api/v1/admin/ingest"
```

The full description is the OpenAPI document at `GET /api/v1/admin/openapi.json` on the admin
listener. It is separate from the public API's document.

### Reaching it from another machine

When the server runs on a VPS, forward the port over SSH. Nothing else ever needs to listen
publicly:

```sh
ssh -N -L 47321:127.0.0.1:47321 you@vps.example.com
# in another terminal, on your machine:
curl -sS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:47321/api/v1/admin/agents
```

The tunnel ends at the server's loopback interface, so the request arrives with a loopback `Host`
and peer, as if it had been made on the server. Keep the token file on the machine that makes the
calls, not in shell history: read it with `$(cat file)` as above.

### Browsers can't drive it

A web page must never be able to call the admin API, even one served from `localhost` or through DNS
rebinding. So the listener refuses, with 403 `browser_refused`, any request that carries `Origin`,
`Sec-Fetch-Site`, `Sec-Fetch-Mode` or `Cookie`. Browsers add those headers themselves, and scripts
like `curl` don't. It also refuses, with 403 `bad_host`, a `Host` that isn't `localhost`,
`127.0.0.1` or `[::1]`, with or without a port. It never sends CORS headers.

## Routes

All under `/api/v1/admin`. Bodies are JSON, times are Unix milliseconds, and errors have the usual
`{"error": {"code", "message"}}` shape.

| Method and path | Does |
|---|---|
| `GET /lists` | Live lists and collections: IDs and names only, for picking an Inbox, a key's list or a grant. |
| `GET /ingest` | Whether ingest is on, the Inbox, the allowed senders and the list keys. |
| `PUT /ingest/inbox` | `{"list": "<id>"}` sets the Inbox; `{"list": null}` unsets it, and email is then refused. |
| `GET /ingest/senders` | The allowed senders. |
| `POST /ingest/senders` | `{"address": "..."}` allows a sender. 201 when added, 200 when it was there already. |
| `DELETE /ingest/senders/{address}` | Removes a sender. 204, or 404. |
| `GET /ingest/keys` | The list keys. |
| `POST /ingest/keys` | `{"key": "errands", "list": "<id>"}` makes `todo+errands@` deliver there. 201, 409 if the key is taken, 422 if it's malformed or `inbox`. |
| `DELETE /ingest/keys/{key}` | Removes a key; its mail goes to the Inbox again. |
| `GET /agents`, `GET /agents/{id}` | Agent tokens: name, status, grants, quota and times. Never the token or its hash. |
| `PUT /agents/{id}/access` | Replaces an active agent's grants and write quota together (below). |
| `POST /agents/{id}/revoke` | Revokes an agent token at once. 204, or 404. |
| `GET /devices` | Paired devices: ID, name and times. |
| `POST /devices/{id}/revoke` | Revokes a device. Its token stops working and its event streams close. |
| `GET /shares` | Share links: what each shows (kind, ID, name) and its times. Never the link itself. |
| `POST /shares/{id}/revoke` | Revokes a share link. |
| `GET /audit?limit=100&before=<id>` | The audit log, newest first, at most 500 per page. Pass `next_before` from a page as `before` to get the next one. |

The Email Worker keeps its own copy of the sender list (`ALLOWED_SENDERS`). Update both, as on the
admin page.

### Examples

```sh
H="Authorization: Bearer $TOKEN"

# Pick the Inbox and allow a sender.
curl -sS -H "$H" "$ADMIN/api/v1/admin/lists"
curl -sS -H "$H" -X PUT "$ADMIN/api/v1/admin/ingest/inbox" \
  -H 'Content-Type: application/json' -d '{"list": "<list id>"}'
curl -sS -H "$H" -X POST "$ADMIN/api/v1/admin/ingest/senders" \
  -H 'Content-Type: application/json' -d '{"address": "you@example.com"}'

# Route todo+errands@ to a list, and remove the key again.
curl -sS -H "$H" -X POST "$ADMIN/api/v1/admin/ingest/keys" \
  -H 'Content-Type: application/json' -d '{"key": "errands", "list": "<list id>"}'
curl -sS -H "$H" -X DELETE "$ADMIN/api/v1/admin/ingest/keys/errands"

# Let an agent read one list and nothing else, with at most 30 writes an hour.
curl -sS -H "$H" -X PUT "$ADMIN/api/v1/admin/agents/3/access" \
  -H 'Content-Type: application/json' \
  -d '{"grants": {"default": "none", "lists": {"<list id>": "read"}}, "writes_per_hour": 30}'

# Revoke a device and read the newest audit entries.
curl -sS -H "$H" -X POST "$ADMIN/api/v1/admin/devices/phone-1a2b3c/revoke"
curl -sS -H "$H" "$ADMIN/api/v1/admin/audit?limit=20"
```

Grants are `{"default": <level>, "collections": {<id>: <level>}, "lists": {<id>: <level>}}`, with the
levels `none`, `read`, `capture` and `plan`. A list's own level wins, then its collection's, then the
default. The body replaces the grants entirely, and every ID must name an existing list or
collection. The change takes effect on the agent's next request, and its open event streams close and
reconnect under the new grants.

## Audit

Every change made through this API goes in the audit log, in the same entry kinds as on the admin
pages (`ingest_inbox_set`, `ingest_sender_added`, `list_key_added`, `grants_changed`,
`agent_token_revoked`, `device_revoked`, `share_revoked`, ...), with `"actor": "admin API"` in the
details and the caller's TCP address. Reads aren't logged. The owner sees the entries on
`/admin/audit`.

The token is never logged at any level, and the server stores only its hash.

## Out of scope, and why

The admin token is a long-lived secret that a program holds. If it leaks, the damage has to stay
small and visible, and the owner has to be able to recover without it.

- **No task data.** No tasks, notes, steps, attachments, events, activity, the event stream or MCP,
  and no commands or undo. Lists and collections appear only as IDs and names. An operator that needs
  task data is an agent, and gets an agent token with grants the owner chose.
- **No new credentials.** It can't create agent tokens, approve device pairings, create or rotate
  share links, or make admin tokens. Each of those hands out access, so each stays behind the owner's
  passkey on the admin pages. The API can only take access away, and narrow or widen what an existing
  agent may do within the levels the owner's pages offer.
- **Nothing about passkeys or sessions.** Signing in, step-up and passkeys exist to keep the owner in
  control; a token can't stand in for them.
- **No remote access.** A public admin endpoint would make the token a remote root credential. Local
  only, with SSH for remote use, means an attacker needs both the token and a shell on the server.
- **No minting or revoking admin tokens through the API.** There is one token, its hash is in the
  config, and rotating it needs a restart. Anyone who can do that already controls the server.
