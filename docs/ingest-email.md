# Email ingest: Worker → server contract

Forwarding an email to `todo@in.example.com` creates a task in the Inbox. Forwarding it to
`todo+<key>@in.example.com` creates it in the list the owner gave that key. No app needs to be running. With the
optional recipient secret, the addresses become `todo-<secret>@in.example.com` and
`todo-<secret>+<list>@in.example.com` (see [Who gets in](#who-gets-in)).

```text
mail client ──► Cloudflare Email Routing ──► Email Worker (workers/email-ingest)
                 SPF/DKIM/DMARC checks          allowlist + exact DKIM/SPF alignment, metadata only
                                                    │ POST /api/v1/ingest/email (HMAC)
                                                    ▼
                                                todo-server ──► task, actor `email-ingest`
                                                    ▲
                    on network error/5xx/429:       │ retried with backoff for up to 23 h
                    Cloudflare Queue ───────────────┘
                         │ out of retries
                         ▼
                    dead-letter queue ──► failure notice to the owner
                         │ notice impossible or failing on the last attempt
                         ▼
                    R2: dead-letters/<date>/<hash>.json (30 days) ──► owner re-ingests
```

This document is the contract between the Worker (this repo, `workers/email-ingest`) and the server
endpoint (`crates/todo-server/src/ingest`). The shared signing test vector is
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

Each `X-Todo-*` header appears exactly once.

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
2. **Headers present, well-formed and not repeated.** If any of the three `X-Todo-*` headers is missing,
   malformed or **present more than once**, respond `401`. Never pick one of several values: a proxy
   may join them, or a parser may take the first while another takes the last.
3. **Timestamp.** It must be within ±300 seconds of the server's clock. Otherwise `401`.
4. **Signature first.** Compute it over the raw body and compare in constant time. Otherwise `401`.
   Nothing before this step touches storage, so an unauthenticated request can't write anything.
5. **Nonce, reserved atomically.** Only now, with the signature verified, `INSERT` the nonce into a table
   with a **unique key** on it (and the time seen). A unique-constraint violation means a replay: respond
   `401`. Never check-then-insert in two steps, because two copies of the same request racing each
   other would both pass the check. Keep nonces for at least 10 minutes, the width of the timestamp
   window; older ones can be pruned, because their timestamps no longer verify.
6. **Schema.** Parse the JSON and check it against the schema below. If it doesn't match, respond `422`.
7. **Sender.** Check `forwarded_by` against the server's own allowlist. If it isn't on it, respond
   `403`. The Worker checks its copy first; the server re-checks so that a leaked HMAC secret alone
   can't create tasks for arbitrary senders. The server checks it once more inside the transaction of
   step 8, after the `message_id` row has taken the write lock, so a sender removed while the request
   waited is refused too.
8. **Dedupe and create, in one transaction.** Retries reuse the same `message_id` with a new timestamp,
   nonce and signature, so `message_id` is what makes them safe.
   - Store `message_id` in a table with a **unique constraint**, and create the task in the **same
     transaction**. Insert the `message_id` row first; if it violates the constraint, roll back and
     respond `200` with `{"status":"duplicate"}`.
   - Otherwise create the task with actor `email-ingest`, commit, and respond `201` with
     `{"status":"created"}`.
   - A lookup followed by a separate insert is not enough: a retry that arrives while the first request
     is still running would create a second task.
   - The response body is optional; the Worker reads only the status code.

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
  "raw_truncated": false,
  "parse_failed": false
}
```

| Field | Type | Meaning |
|---|---|---|
| `version` | `1` | Schema version. The server rejects other values with `422`. |
| `message_id` | string, ≤ 998 chars | The forwarded email's `Message-ID`, angle brackets included. If it was missing or malformed, the Worker uses `<sha256-<hex of the raw bytes>@email-ingest.invalid>`, which stays the same when the same message is redelivered. **The dedupe key.** |
| `list` | string or `null` | The list key from `todo+<list>@` (or `todo-<secret>+<list>@`): lowercase, matching `^[a-z0-9][a-z0-9_-]{0,63}$`. `null` for plain `todo@`, which means the Inbox. |
| `title` | string, 1–500 chars | The subject with `FW:`, `Fwd:`, `WG:`, `Re:` and `AW:` chains (also `Re[2]:`) and `#keep` removed, and whitespace collapsed. `(no subject)` if nothing is left. Fits `todo_core::decide::MAX_TITLE`. |
| `subject` | string, ≤ 998 chars | The original decoded subject, unchanged apart from collapsed whitespace and removed control characters. |
| `keep` | boolean | Whether the subject contained the word `#keep` (see below). |
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
| `parse_failed` | boolean | Whether the MIME parser refused the message (over a structure limit, or malformed). The task is then built from the headers only: `title` and `subject` from the Subject header, `original_sender.source` is `from`, and `body` is `null`. A server that doesn't know this field may ignore it. |

Unknown fields must be ignored, so later versions can add fields without breaking older servers. A
missing `parse_failed` means `false`. Attachments are never sent; see [Follow-ups](#follow-ups).

**`#keep`.** Forward prefixes are stripped first, so `Fwd: #keep Report` counts. `#keep` must then be a
whole word, optionally followed by punctuation (`#keep,`, `#keep.` or `#keep)`); `#keeper` and `x#keep` don't
count.
The check runs on the whole subject before the title is shortened, so cutting a long subject can neither
create nor lose it.

**Forwarded sender.** The Worker only reads `From:` (or `Od:`, `Von:` and so on) from the body when there
is evidence of a forwarded header block: a separator line (`---------- Forwarded message ---------`,
`Begin forwarded message:`, `-----Original Message-----` and localized forms) within the three non-blank
lines before it, or another header line (`Date:`, `Sent:`, `To:`, `Subject:` …) directly before or after it. A
lone `From: …` sentence in someone's text is ignored.

### Task mapping (what the server stores)

- **Title** is `title`.
- **List.** Routing is explicit and never looks at list names, so renaming, deleting or adding a list
  can't silently send mail somewhere else. On the admin page (**Email**) the owner picks the Inbox and
  gives lists keys; each key names one list by ID.
  - `null` (plain `todo@`) and the reserved key `inbox` go to the Inbox.
  - A key the owner set goes to its list.
  - A key nobody set, or one whose list was deleted since, goes to the Inbox, with a line in the notes
    saying the key didn't match. The admin page flags keys whose list is gone.
  - With no Inbox picked, or a deleted one, the server answers `503 inbox_not_found` and stores
    nothing, so the Worker keeps retrying until the owner picks one; the admin page says so.
- **Who can see it.** An email task is an ordinary task: an agent that can read its list sees the
  subject, the sender and any kept body. That is the owner's choice of list; the admin page says so
  next to the Inbox and key pickers. Share pages never show notes or links.
- **Notes.** A short plain-text block, followed by `body.text` only when `keep` is true:

  ```text
  From: Alice Example <alice@example.org>
  Forwarded by you@example.com on 2025-11-21 08:00 (UTC)
  Subject: Fwd: Nákup – mléko #2 #keep
  ```

  The time is `received_at` in the owner's time zone. A cut body (`body.truncated` or `raw_truncated`)
  ends with a line saying so, and `parse_failed` adds a line too.
- **Link.** An email link (the same kind the Microsoft To Do importer makes for Flagged Emails) with the
  original subject, the original sender's address, `received_at` and `message_id`, so the desktop app
  can match it later.
- **Actor and undo.** The task and its link are one event group with actor `email-ingest`: they show in
  the activity feed, and one undo removes both. The `message_id` stays recorded, so forwarding the same
  message again afterwards is still a duplicate.
- **Dedupe key.** The SHA-256 of `message_id` as the link stores it (trimmed, at most 2048 bytes). The
  table of keys is derived from the event log: every email link created by `email-ingest` is one row.
  The server checks it against the log at startup, and rebuilds it then if it differs, and
  `todo-server rebuild-projections` rebuilds it too.
- **Restoring a snapshot.** After the database is restored from a backup, deliveries still queued in the
  Worker (or re-ingested from the dead-letter archive) are retried against the restored log. One whose
  task was stored after the snapshot was taken creates that task again, and that is correct: the
  restored database lost it. One whose task is in the snapshot is still a duplicate.
- **Response.** `201 {"status":"created","task_id":"…"}` or `200 {"status":"duplicate","task_id":"…"}`.
  Errors are the server's usual `{"error":{"code","message"}}` with fixed messages; they never echo the
  request.

### Server configuration

```toml
[ingest]
# The shared secret; without it the route answers 404. A regular file (not a link), owned by the
# account running the server and readable only by it, checked and read through one open handle.
email_hmac_secret_file = "/etc/todo/ingest-email-hmac.secret"
# Seeds the allowlist on the first start; the admin page manages it afterwards.
allowed_senders = ["you@example.com"]
# Per client IP (the TCP peer, or CF-Connecting-IP with trust_cloudflare_headers).
requests_per_minute = 60
request_burst = 20
```

Failed signatures also spend the client IP's budget of failed authentications (`auth.bearer_failures_*`),
like bad API tokens do; an IP that used it up gets `429` before its signature is checked.

## Who gets in

The Worker rejects the email at SMTP time (`message.setReject(...)`) unless all of these hold:

1. **Recipient.** The envelope recipient's local part is `todo`, optionally with `+<list>`, and the list
   key is valid. An empty suffix (`todo+@`) is rejected. With `RECIPIENT_SECRET` set it must be
   `todo-<secret>` instead, again optionally with `+<list>`, and plain `todo@` is rejected. The local
   part can be changed with `INGEST_LOCAL_PART`.
2. **One sender.** The `From` header holds exactly one mailbox.
3. **Allowlist.** That address is in `ALLOWED_SENDERS`. If `ALLOWED_SENDERS` is empty, everything is
   rejected.
4. **Exactly aligned authentication.** Cloudflare's `Authentication-Results` header shows a pass whose
   domain is **exactly** the From domain (case-insensitive, trailing dot ignored):
   - `dkim=pass` with `header.d` equal to the From domain (checked first); or
   - `spf=pass` with the domain of `smtp.mailfrom` equal to the From domain.

   Nothing else counts:
   - not a subdomain or a parent domain. Relaxed alignment would need the Public Suffix List to find
     the organizational domain, and getting that wrong is how `example.co.uk` ends up "aligned" with
     `attacker.co.uk`;
   - not `header.i`;
   - not `dmarc=pass` on its own, because it doesn't say which identifier passed or with which alignment
     mode.

   A missing header, a header from another authentication service, or only failures are all rejected.

   The cost: a provider that signs `From: you@example.com` only with `d=mail.example.com`, and sends
   from a `mail.example.com` envelope, is rejected with `auth_unaligned`. Big mailbox providers sign
   with the From domain itself. Use `DEBUG_AUTH_SHAPE` (below) to see which part didn't match.

Rejections log one line such as `{"evt":"rejected","reason":"auth_unaligned"}`, with no addresses and
no content. Nothing from the message is stored or forwarded. The body is not even read: the checks use
only the headers.

### Allowlist and exact alignment, or a recipient secret too

These are the two ways to run it. **The owner decides.**

- **Default: allowlist plus exact DKIM or SPF alignment.** Anyone who can get a DKIM signature or SPF
  pass for your exact From domain can create tasks: your mail provider, and anyone with access to your
  account there. Everyone else is rejected, even if they know the address. The address can be public.
- **Stronger: add `RECIPIENT_SECRET`.** The ingest address becomes `todo-<secret>@in.example.com`.
  - A sender now also needs the secret, so a forged or misattributed pass alone isn't enough.
  - The cost is a long address, which you save as a contact. Anyone who sees a message you forwarded
    (with its envelope) learns it, so rotate it if it leaks.
  - **Cloudflare can see it.** Email Routing's activity log shows the full recipient address of every
    message. So does the routing rule. Anyone with access to the Cloudflare account can therefore read
    the secret. Today that is only the owner, which is acceptable, but if account access is ever shared
    (another member, an API token with Email Routing read access), rotate the secret.
  - Rules for the secret:
    - exactly 16 to 64 random lowercase letters and digits, such as `openssl rand -hex 12`;
    - validated exactly as configured, with no trimming or case folding;
    - only an absent binding turns the feature off. Any value that is set but malformed, including an
      empty or whitespace-only one, rejects all mail (`config_recipient_secret`) rather than falling
      back to `todo@`;
    - set it as a Worker secret and change the routing rule to the new address;
    - the comparison is constant-time;
    - the secret never appears in logs or in the rejection text.

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
  - The documented example is simplified (`spf=pass; dkim=pass; dmarc=pass`), so the parser reads the
    standard RFC 8601 properties `header.d` and `smtp.mailfrom`. It rejects a pass that comes without
    them.
  - **After deploying, check one real message with `DEBUG_AUTH_SHAPE=true`.** That logs one
    `auth_shape` line of **booleans only**: whether the header was present, the authserv-id matched,
    DKIM passed, `header.d` was present and aligned, SPF passed, `smtp.mailfrom` was present and aligned,
    and whether DMARC passed. It never logs the header, a domain or an address. Turn it off again
    afterwards. If Cloudflare ever omits these properties, every message fails with `auth_unaligned`:
    failing closed, not open.
- **The authserv-id.** The trusted identifier defaults to `mx.cloudflare.net`. It can be overridden
  with the `TRUSTED_AUTHSERV_ID` variable, for tests only.

## Failures and retries

- **Transient failure on first delivery.** On a network error, a timeout, `5xx` or `429`, the Worker puts
  `{v, accepted_at, attempt, mid, payload}` on the Cloudflare Queue.
  - `payload` is the unsigned metadata above.
  - It accepts the email only once the queue has taken it.
  - Every retry gets a **fresh timestamp, nonce and signature**, with the same `message_id`.
- **The schedule.** Retry *n* waits 30 s, 1, 2, 5, 10, 20 and 30 min, then 1 h, then every 2 h, until
  **23 h** after acceptance. The last delay is shortened so the final attempt lands at 23 h. That makes
  19 retries; a hard cap of 40 also applies.
- **The deadline is checked before the POST.** A message that is delivered after its deadline (for
  example, late because of a backlog) is not posted. It only gets its notice.
- **How a retry is queued.** The consumer enqueues a new message with `attempt + 1` and `delaySeconds`,
  then acks the old one. This resets the queue's own retention clock: 24 h on the Free plan, not
  configurable. So the Worker's deadline is the one that counts, and a message can't expire silently.
  - If that enqueue fails, it falls back to `msg.retry({ delaySeconds })`. That does *not* reset the
    retention clock, which is why the window is 23 h and not 24 h: the spare hour covers fallbacks and
    notice retries. The attempt count then also includes the platform's `msg.attempts`, so the backoff
    keeps growing.
  - `max_retries = 30` on the consumer only counts these platform retries, never the normal
    re-enqueues. The 23 h window ends a message first, but 30 is enough for the whole schedule if every
    re-enqueue fails.
- **Dead-letter queue.** A message that runs out of platform retries goes to `todo-email-ingest-dlq`. The
  same Worker consumes it (`batch.queue` tells the two apart) and only sends the failure notice. It
  never posts.
- **Notice before ack.** Whenever the Worker gives up on a queued message (expired, refused, or dead
  letter), it sends the notice **first** and acks only once the notice is sent.
  - If sending fails, it calls `msg.retry({ delaySeconds: 300 })` instead, so the message isn't lost.
    In the main queue that eventually moves it to the dead-letter queue, which tries again.
  - If no notice can reach the owner, the metadata goes to R2 before the ack (next section). That
    covers two cases: no notice is configured, or the dead-letter queue's last attempt failed too.
- **The server is restarting.** Mail accepted while the server restarts is never lost. If neither the
  POST nor the queue succeeds, the email handler throws instead of accepting the message. Cloudflare
  doesn't document what happens to the incoming message then. Either way, it isn't silently accepted.
- **Permanent failure.** A `4xx` other than `408` and `429` is not retried. Only the status code and the
  dedupe hash are logged.
  - **First delivery:** the email is rejected at SMTP time with `Task server refused the message (HTTP
    <status>)`, so the forwarder gets a bounce.
  - **From the queue:** a failure notice is sent (below).
- **Misconfiguration.** A missing or too-short secret, or a missing or non-https `INGEST_URL`, is
  treated as transient. Mail is queued and goes through once the configuration is fixed, within 23 h.
- **Final failure notice.** It goes to `NOTIFY_ADDRESS` through a `send_email` binding that is restricted
  to that one address. It carries the title, the received time, the reason and the dedupe hash.
  - Sending to a **verified destination address** of the account is free on every plan, including
    Workers Free with only Email Routing set up. Sending to arbitrary recipients needs Workers Paid.
    Sources: [Email Service pricing](https://developers.cloudflare.com/email-service/platform/pricing/)
    and [limits](https://developers.cloudflare.com/email-service/platform/limits/#verified-destination-addresses).
  - The sender (`NOTIFY_FROM`) must be on the Email Routing domain.
  - Without the binding the Worker logs `notify_unconfigured` and archives to R2. A failed send logs
    `notify_failed` and is retried as above.

### Dead-letter archive (R2), the last resort

The dead-letter queue has no further destination. If its consumer runs out of retries, Cloudflare deletes
the message. So when no notice can reach the owner, the Worker writes the message to the `DEAD_LETTERS`
R2 bucket before it acks. That happens when:

- no notice is configured, from either queue; or
- the notice fails on the dead-letter queue's last attempt. That is checked as `msg.attempts ≥
  DEAD_LETTER_MAX_RETRIES` (default 10, equal to the consumer's `max_retries`), one attempt early, so
  how the platform counts can't make it skip the archive.

What gets written:

- **Key.** `dead-letters/<YYYY-MM-DD>/<mid>.json`: the archive date and the 12-hex Message-ID hash. A
  malformed envelope uses `unknown-<hash>` instead.
- **Content.** `{"version":1,"archived_at":"…","reason":"…","envelope":{v, accepted_at, attempt, mid,
  payload}}`. That is the same metadata-only payload the server would have received (title, subject,
  senders, list, and the `#keep` body if any). No raw message and no attachments.
- **Bound.** Writes are limited by their own admission counter, with the same per-isolate limit as
  accepted emails (`MAX_ACCEPTED_PER_DAY`). R2's free tier (10 GB-month of storage and 1 million Class A
  operations per month) is far above that.

**Only if that write fails too** (R2 error, over the brake, or no bucket bound) does the Worker give up.
It logs `{"evt":"dead_letter_lost","mid":"…","reason":"…","via":"failed|limited|unconfigured"}`, with
no content, and calls `msg.retry()`, so the platform retries the message or, after its last attempt,
deletes it. With neither a notice nor a bucket configured, retrying can't help, so it acks after that
log line.

A successful write logs `dead_letter_archived`. Watch for both events in the Worker logs.

**Lifecycle.** Add a rule that deletes `dead-letters/` objects after **30 days**, so the archive can't
grow without bound (see the README for the commands). Thirty days is long enough to notice a
`dead_letter_archived` line and re-ingest.

**Re-ingesting.** Download the objects, then run `node scripts/reingest.js <files…>` with `INGEST_URL`
and `INGEST_HMAC_SECRET` set. It POSTs each payload, signed exactly as the Worker signs it, and prints
only the file name, the outcome and the HTTP status. Because the server dedupes on `message_id`, running
it twice, or on a message that did arrive, creates nothing new. The README has the step-by-step
commands.

## Bounds (Workers Free plan)

| What | Bound | Why |
|---|---|---|
| Raw message read and parsed | 256 KiB; the rest of the stream is never read | Free-plan CPU is 10 ms per invocation; see [CPU budget](#cpu-budget). Headers and text parts come first in a forward. Email Routing itself accepts up to 25 MiB. |
| Header section | Must end within those 256 KiB, else rejected | No partial header parsing. |
| MIME structure (postal-mime) | Nesting 16, parts 100, headers 256 KiB, nested messages 3, forwarded addresses 500 | Far below postal-mime's defaults. A message over a limit becomes a headers-only task (`parse_failed`). |
| Original subject | 998 characters | RFC 5322 line limit. |
| Title | 500 characters | `MAX_TITLE` in `todo-core`. |
| Kept body | 64 KiB of UTF-8 | |
| Request body | 100,000 bytes | |
| Queue message | 120,000 bytes, checked before sending | Queues allow 128 KB. |
| Forwarded-sender scan | First 64 KiB and 2,000 lines of the text; lines over 998 characters are skipped | Header labels and separators are matched without backtracking regexes, and the address is found with `indexOf` and one linear scan outward from `@`. |
| HTML tag scan | 4,096 characters per tag; quote-aware | A stray `<` is kept as text. |
| POST timeout | 15 s | |
| Retries | Until 23 h after acceptance, at most 40 | |
| Accepted emails | 50 per isolate in any 24 h window (`MAX_ACCEPTED_PER_DAY`) | See below. |
| Dead-letter archive writes | The same limit, counted separately | Bounds R2 writes in a failure storm. |

### Quota: nothing can be billed, but the quota can run out

A Free-plan account has no payment method, so nothing in this Worker can create a bill. Running out of
a quota only makes things fail: Queues stop accepting writes until the daily reset, and Workers stop
running.

The Queues Free plan allows 10,000 operations per day. A write, a read and a delete each count once per
64 KB of message, and every redelivery is another read. Sources:
[Queues pricing](https://developers.cloudflare.com/queues/platform/pricing/) and
[limits](https://developers.cloudflare.com/queues/platform/limits/).

| Email retried for the full 23 h (19 queue messages) | Operations |
|---|---|
| Small envelope (≤ 64 KB) | 19 × (write + read + delete) = 57 |
| `#keep` envelope over 64 KB (2 chunks) | 19 × 6 = 114 |
| Plus the dead-letter path, redeliveries after `msg.retry` fallbacks, and notice retries | a few more |

So the worst case is about **120 operations per email**. If the server is down for a day, 50 accepted
emails cost about 6,000 of the 10,000, which leaves room for redeliveries. Mail that is delivered on the
first try costs nothing.

That is where the **admission guard** comes from. Each isolate accepts at most `MAX_ACCEPTED_PER_DAY`
(default 50) emails in any sliding 24 h window. It counts only mail that passed the sender checks. Over
that, the email handler throws, so Email Routing reports a temporary failure and the sending server
tries again later. Nothing is lost silently.
- The guard is per isolate, kept in memory: no KV or Durable Objects, which would cost operations of
  their own.
- Cloudflare may run more than one isolate, and an isolate can be recycled at any time. So it is a cheap
  brake on a burst, not an exact account-wide limit. The allowlist and aligned authentication are what
  keep strangers out.

### CPU budget

The Free plan allows 10 ms of CPU per invocation. `test/security.test.js` runs the full email handler on
three hostile messages just under the 256 KiB read cap:

- 40 headers of 900 characters;
- `message/rfc822` nested three levels;
- 990-character adversarial `From:` lines (`a…`, `@…`, `a.a.…`), each after three 990-character dash
  lines that a backtracking separator regex would choke on;
- HTML full of unclosed quotes, stray `<` and unclosed `<style>`.

The test asserts a median under a generous 100 ms. That allows for slow, noisy CI runners and catches
quadratic blowups, which cost seconds rather than milliseconds.

Measured with Node 24 on a desktop machine, warm, as CPU time per email (`process.cpuUsage` over 60
runs, two runs each):

| Case | CPU per email |
|---|---|
| 80 mixed text and HTML parts (within the 100-part limit) | 4.4–6.0 ms |
| 80 HTML parts only, so all of it goes through `htmlToText` | 3.4–3.6 ms |
| 300 parts (over the limit, headers-only fallback) | 1.6 ms |

MIME parsing is most of it (about 4 ms for 256 KiB). The forwarded-sender scan and payload building
together take under 0.5 ms.

- **The margin** is roughly 1.7× to 2× against 10 ms when the isolate is warm.
- **The first message in a fresh isolate** runs before V8 has optimized the code and was measured at
  8–22 ms of wall time, so a cold worst case can go over 10 ms.
- **Typical forwards** are a few tens of KiB. Parsing dominates and grows with size, so they cost
  proportionally less.

If Cloudflare ever ends an invocation for CPU, the email handler fails and the message is not accepted,
just like the throw above. If that happens in practice, lower `MAX_RAW_BYTES`.

## Logging

The Worker logs one JSON line per outcome, with an allowlist of fields enforced in code:

- `evt`;
- `reason` and `via`, as fixed tokens;
- `status`, `attempt`, `delay_s` and `bytes`;
- the booleans `has_list`, `keep`, `body_truncated`, `raw_truncated` and `parse_failed`;
- with `DEBUG_AUTH_SHAPE=true`, the `auth_shape` booleans described above;
- `mid`: the first 12 hex characters of SHA-256 over the `Message-ID`.

Fields that are meant to be booleans are dropped unless they are booleans. The Worker never logs
addresses, subjects, bodies, list names, the recipient secret or the HMAC secret.

## Follow-ups

- **Server endpoint.** Done in `crates/todo-server/src/ingest`, following
  [What the server must check](#what-the-server-must-check-in-this-order). Its tests include one that
  runs this Worker's `postPayload` from Node against the real server.
- **Attachments.** Store attachments of `#keep` mails in R2, at most 10 MB per message. That means
  streaming the parts past the 256 KiB parse window without decoding them in the Worker, and
  probably a separate upload endpoint. Not in this PR.
- **One allowlist.** The server's allowlist is managed on the admin page now (with the Inbox and the
  list keys, every change audited in the same transaction). The Worker still has its
  own copy (`ALLOWED_SENDERS`), which it needs to reject strangers at SMTP time; syncing it from the
  server is left for later.
