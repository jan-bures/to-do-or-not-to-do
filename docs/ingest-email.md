# Email ingest: Worker → server contract

Forwarding an email to `todo@in.example.com` creates a task in the Inbox. Forwarding it to
`todo+<list>@in.example.com` creates it in the list named `<list>`. No app needs to be running.

```text
mail client ──► Cloudflare Email Routing ──► Email Worker (workers/email-ingest)
                 SPF/DKIM/DMARC checks          allowlist + aligned auth, metadata only
                                                    │ POST /api/v1/ingest/email (HMAC)
                                                    ▼
                                                todo-server ──► task, actor `email-ingest`
                                                    ▲
                    on network error/5xx/429:       │ retried with backoff for up to 24 h
                    Cloudflare Queue ───────────────┘
```

This document is the contract between the Worker (this repo, `workers/email-ingest`) and the server
endpoint (a later PR). The shared signing test vector is
[`ingest-email-signature.json`](ingest-email-signature.json).

## Request

```http
POST /api/v1/ingest/email HTTP/1.1
Host: todo.example.com
Content-Type: application/json
X-Todo-Timestamp: 1763712000
X-Todo-Nonce: 00112233445566778899aabbccddeeff
X-Todo-Signature: 756c2659…

{"version":1,"message_id":"<…>",…}
```

### Headers

| Header | Format |
|---|---|
| `X-Todo-Timestamp` | Unix time in seconds, as decimal ASCII digits with no sign, fraction or leading zeros. |
| `X-Todo-Nonce` | 128 random bits as 32 lowercase hex characters. A fresh nonce for every attempt. |
| `X-Todo-Signature` | 64 lowercase hex characters: `HMAC-SHA256(key, message)`. |
| `Content-Type` | `application/json`. The body is UTF-8. |

### Signature

```text
key     = UTF-8 bytes of the shared secret INGEST_HMAC_SECRET, used as is (not hex- or base64-decoded)
message = timestamp ‖ 0x0A ‖ nonce ‖ 0x0A ‖ body
```

- `timestamp` and `nonce` are the exact header values, as ASCII.
- `body` is the exact request body bytes as received. Verify the signature over the raw bytes before
  parsing the JSON, and never over a re-serialization.
- The secret is at least 32 characters long. Generate it with something like `openssl rand -hex 32`.

### What the server must check, in this order

1. **Body size.** Reject bodies over 128 KiB with `413` before reading further. The Worker never sends
   more than 100,000 bytes.
2. **Headers present and well-formed.** Otherwise `401`.
3. **Timestamp.** It must be within ±300 seconds of the server's clock. Otherwise `401`.
4. **Signature.** Compute it and compare in constant time. Otherwise `401`.
5. **Nonce.** It must not have been used before. Remember accepted nonces for at least 10 minutes, the
   width of the timestamp window, and reject a reused one with `401`. Only remember nonces whose
   signature verified, so unauthenticated requests can't fill the store.
6. **Schema.** Parse the JSON and check it against the schema below. If it doesn't match, respond `422`.
7. **Sender.** Check `forwarded_by` against the server's own allowlist. If it isn't on it, respond
   `403`. The Worker checks its copy first; the server re-checks so that a leaked HMAC secret alone
   can't create tasks for arbitrary senders.
8. **Dedupe.** Look up `message_id`. If a task was already created from this `message_id`, respond
   `200` with `{"status":"duplicate"}` and create nothing. Retries reuse the same `message_id` with a
   new timestamp, nonce and signature, so this is what makes them safe.
9. **Create the task.** Use actor `email-ingest`, then respond `201` with `{"status":"created"}`. The
   response body is optional; the Worker reads only the status code.

### How the Worker treats responses

| Status | Meaning to the Worker |
|---|---|
| `2xx` | Delivered. Done. |
| `429`, `408`, `5xx`, network error or timeout (15 s) | Transient. Queue and retry with backoff (see below). |
| Any other `4xx`, or `3xx` (redirects aren't followed) | Permanent. Not retried. |

So the server should:

- return `5xx` or `429` for anything that may succeed later, such as the database being busy or the
  server shutting down;
- return `4xx` only for requests that will never succeed;
- never use `409` for duplicates: return `200`.

## Body (schema version 1)

```json
{
  "version": 1,
  "message_id": "<CAexample-1234@mail.example.org>",
  "list": "shopping",
  "title": "Nákup – mléko #2",
  "subject": "Fwd: Nákup – mléko #2 #keep",
  "keep": true,
  "original_sender": { "address": "alice@example.org", "name": "Alice Example", "source": "forwarded_header" },
  "forwarded_by": "you@example.com",
  "received_at": "2025-11-21T08:00:00.000Z",
  "body": { "text": "Line one\nLine \"two\" ✓", "source": "text", "truncated": false },
  "raw_truncated": false
}
```

| Field | Type | Meaning |
|---|---|---|
| `version` | `1` | Schema version. The server rejects other values with `422`. |
| `message_id` | string, ≤ 998 chars | The forwarded email's `Message-ID`, angle brackets included. If it was missing or malformed, the Worker uses `<sha256-<hex of the raw bytes>@email-ingest.invalid>`, which stays the same when the same message is redelivered. **The dedupe key.** |
| `list` | string or `null` | The list key from `todo+<list>@`: lowercase, matching `^[a-z0-9][a-z0-9_-]{0,63}$`. `null` for plain `todo@`, which means the Inbox. |
| `title` | string, 1–500 chars | The subject with `FW:`, `Fwd:`, `WG:`, `Re:` and `AW:` chains (also `Re[2]:`) and `#keep` removed, and whitespace collapsed. `(no subject)` if nothing is left. Fits `todo_core::decide::MAX_TITLE`. |
| `subject` | string, ≤ 998 chars | The original decoded subject, unchanged apart from collapsed whitespace and removed control characters. |
| `keep` | boolean | Whether the subject contained `#keep`. |
| `original_sender` | object | Who wrote the forwarded email (see below). |
| `original_sender.address` | string | Lowercased address. |
| `original_sender.name` | string or `null` | Display name, ≤ 200 chars. |
| `original_sender.source` | `"forwarded_header"` or `"from"` | `forwarded_header` if it came from the forwarded header block in the body (`From: … <…>`, `Od:`, `Von:` and so on). `from` if no block was found and the forward's own From was used. |
| `forwarded_by` | string | The forward's own From address, lowercased: an allowlisted sender. |
| `received_at` | string | When the Worker received the email: RFC 3339 in UTC, with milliseconds. |
| `body` | object or `null` | `null` unless `keep` is true and the message had text. |
| `body.text` | string, ≤ 64 KiB of UTF-8 | text/plain if present, otherwise the HTML stripped to text. LF line endings, no control characters except tab and LF. |
| `body.source` | `"text"` or `"html"` | Which part the text came from. |
| `body.truncated` | boolean | Whether the text was cut to fit. |
| `raw_truncated` | boolean | Whether the message was larger than the 256 KiB the Worker parses. The title and sender are still reliable; a kept body may be cut short. |

Unknown fields must be ignored, so later versions can add fields without breaking older servers.
Attachments are never sent; see [Follow-ups](#follow-ups).

### Suggested task mapping (server PR)

- **Title** is `title`.
- **List.** `list` is resolved by list key. If it is `null`, or no list has that key, the task goes to the
  Inbox, and the notes say the requested list wasn't found.
- **Notes.** A short plain-text block, followed by `body.text` when `keep` is true:

  ```text
  From: Alice Example <alice@example.org>
  Forwarded by you@example.com on 2025-11-21 09:00 (owner's time zone)
  Subject: Fwd: Nákup – mléko #2 #keep
  ```

- **Link.** An email link carrying `message_id`, so the desktop app can match it later.

## Who gets in

The Worker rejects the email at SMTP time (`message.setReject(...)`) unless all of these hold:

1. **Recipient.** The envelope recipient's local part is `todo`, optionally with `+<list>`, and the list
   key is valid. The local part can be changed with `INGEST_LOCAL_PART`.
2. **One sender.** The `From` header holds exactly one mailbox.
3. **Allowlist.** That address is in `ALLOWED_SENDERS`. If `ALLOWED_SENDERS` is empty, everything is
   rejected.
4. **Aligned authentication.** Cloudflare's `Authentication-Results` header shows a pass whose
   domain is aligned with the From domain:
   - `dkim=pass` with `header.d` (or the domain of `header.i`);
   - `spf=pass` with the domain of `smtp.mailfrom`;
   - or `dmarc=pass` with `header.from`.

   Aligned means equal, or one is a subdomain of the other (relaxed alignment). A pass for some other
   domain doesn't count. That stops a spoofed `From: you@example.com` sent from a domain with valid SPF of
   its own. A missing header, a header from another authentication service, or only failures are all
   rejected.

Rejections log one line such as `{"evt":"rejected","reason":"auth_unaligned"}`, with no addresses and
no content. Nothing from the message is stored or forwarded. The body is not even read: the checks use
only the headers.

### What Cloudflare provides, and what we rely on

- **The header.** Email Routing checks SPF, DKIM and DMARC before it invokes the Worker. It **prepends**
  a standard `Authentication-Results` header, whose authentication service identifier is
  `mx.cloudflare.net`.
  - The Worker is told to read the **first** such header through `message.headers` and check that
    identifier.
  - Other `Authentication-Results` headers may come from the sender and must not be trusted.
  - ARC isn't needed, because Cloudflare invokes the Worker directly.
  - Source: [Workers API: Authentication results](https://developers.cloudflare.com/email-service/api/route-emails/email-handler/#authentication-results).
- **DMARC policy.** Email Routing rejects mail that fails authentication under the sender domain's
  DMARC policy before any rule or Worker runs. Sources:
  [Postmaster: DMARC enforcing](https://developers.cloudflare.com/email-service/reference/postmaster/#dmarc-enforcing)
  and [Email lifecycle: stage details](https://developers.cloudflare.com/email-service/concepts/email-lifecycle/#stage-details-1).
  - That alone isn't enough. A domain with `p=none`, or with no DMARC record at all, still gets through.
    So the Worker does its own alignment check.
- **How the Worker reads it.**
  - `Headers.get()` joins repeated headers with `", "`. The Worker parses only up to the first top-level
    comma (comments and quoted strings are skipped). The Authentication-Results grammar has no
    top-level commas, so that is exactly the first header, the one Cloudflare prepended.
  - The documented example is simplified (`spf=pass; dkim=pass; dmarc=pass`), so the parser also reads
    the standard RFC 8601 properties `header.d`, `header.i`, `smtp.mailfrom` and `header.from`. It
    rejects any pass that comes without one of them.
  - **After deploying, check one real header in the Worker logs** (with `wrangler tail` or the dashboard,
    temporarily, then remove the log line). If Cloudflare ever omits these properties, every message
    will fail with `auth_unaligned`: failing closed, not open.
- **The authserv-id.** The trusted identifier defaults to `mx.cloudflare.net`. It can be overridden
  with the `TRUSTED_AUTHSERV_ID` variable, for tests only.

## Failures and retries

- **Transient failure on first delivery.** On a network error, a timeout, `5xx` or `429`, the Worker puts
  `{v, accepted_at, attempt, mid, payload}` on the Cloudflare Queue.
  - `payload` is the unsigned metadata above.
  - It accepts the email only once the queue has taken it.
  - Every retry gets a **fresh timestamp, nonce and signature**, with the same `message_id`.
- **The schedule.** Retry *n* waits 30 s, 1, 2, 5, 10, 20 and 30 min, then 1 h, then every 2 h, until
  24 h after acceptance. The last delay is shortened so the final attempt lands at 24 h. That makes 19
  retries; a hard cap of 40 also applies.
- **How a retry is queued.** The consumer acks the message and enqueues a new one with
  `attempt + 1` and `delaySeconds`. This resets the queue's own retention clock: 24 h on the Free plan,
  not configurable. So the Worker's deadline is the one that counts, and a message can't expire
  silently.
  - If that enqueue fails, it falls back to `msg.retry({ delaySeconds })`.
  - If the consumer itself throws, Queues retries the batch up to `max_retries` times.
- **The server is restarting.** Mail accepted while the server restarts is never lost. If neither the
  POST nor the queue succeeds, the email handler throws instead of accepting the message. Cloudflare
  doesn't document what happens to the incoming message then. Either way, it isn't silently accepted.
- **Permanent failure.** A `4xx` other than `408` and `429` is not retried. Only the status code and the
  dedupe hash are logged.
  - **First delivery:** the email is rejected at SMTP time with `Task server refused the message (HTTP
    <status>)`, so the forwarder gets a bounce.
  - **From the queue:** a failure notice is sent (below).
- **Misconfiguration.** A missing or too-short secret, or a missing or non-https `INGEST_URL`, is
  treated as transient. Mail is queued and goes through once the configuration is fixed, within 24 h.
- **Final failure notice.** It goes to `NOTIFY_ADDRESS` through a `send_email` binding that is restricted
  to that one address. It carries the title, the received time, the reason and the dedupe hash.
  - Sending to a **verified destination address** of the account is free on every plan, including
    Workers Free with only Email Routing set up. Sending to arbitrary recipients needs Workers Paid.
    Sources: [Email Service pricing](https://developers.cloudflare.com/email-service/platform/pricing/)
    and [limits](https://developers.cloudflare.com/email-service/platform/limits/#verified-destination-addresses).
  - The sender (`NOTIFY_FROM`) must be on the Email Routing domain.
  - If the binding isn't configured, or the send fails, the Worker logs `notify_unconfigured` or
    `notify_failed` and nothing else happens. That is the documented limitation.

## Bounds (Workers Free plan)

| What | Bound | Why |
|---|---|---|
| Raw message read and parsed | 256 KiB; the rest of the stream is never read | Free-plan CPU is 10 ms per invocation. Parsing 256 KiB took about 5 ms when measured, and 1 MiB about 17 ms. Headers and text parts come first in a forward. Email Routing itself accepts up to 25 MiB. |
| Header section | Must end within those 256 KiB, else rejected | No partial header parsing. |
| MIME structure (postal-mime) | Nesting 32, parts 500, headers 256 KiB, nested messages 3 | Far below postal-mime's defaults. |
| Original subject | 998 characters | RFC 5322 line limit. |
| Title | 500 characters | `MAX_TITLE` in `todo-core`. |
| Kept body | 64 KiB of UTF-8 | |
| Request body | 100,000 bytes | |
| Queue message | 120,000 bytes, checked before sending | Queues allow 128 KB. |
| Text scanned for the forwarded header block | 64 KiB | |
| POST timeout | 15 s | |
| Retries | Until 24 h after acceptance, at most 40 | |

Every queued email costs at most 3 operations per attempt (write, read and delete) out of the Free
plan's 10,000 per day, which is about 60 for an email that is retried for the full 24 h (19 retries). See
[Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/) and
[limits](https://developers.cloudflare.com/queues/platform/limits/).

## Logging

The Worker logs one JSON line per outcome, with an allowlist of fields enforced in code:

- `evt`;
- `reason` and `via`, as fixed tokens;
- `status`, `attempt` and `delay_s`;
- the booleans `has_list`, `keep`, `body_truncated` and `raw_truncated`;
- `mid`: the first 12 hex characters of SHA-256 over the `Message-ID`.

It never logs addresses, subjects, bodies, list names or the secret.

## Follow-ups

- **Attachments.** Store attachments of `#keep` mails in R2, at most 10 MB per message. That means
  streaming the parts past the 256 KiB parse window without decoding them in the Worker, and
  probably a separate upload endpoint. Not in this PR.
- **One allowlist.** Manage the allowlist on the admin page and have the server be the single source of
  truth. The Worker's copy would then be only an early filter.
