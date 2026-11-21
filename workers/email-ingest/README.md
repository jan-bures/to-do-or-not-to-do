# email-ingest Worker

A Cloudflare Email Worker that turns forwarded emails into tasks.

- **Address.** Mail to `todo@in.example.com` goes to the Inbox, and mail to `todo+<key>@in.example.com` goes
  to the list the owner gave that key on the server's admin page.
- **Checks.** It accepts mail only from allowlisted senders whose DKIM or SPF passed for exactly the From
  domain. Optionally, the address also carries a secret: `todo-<secret>@in.example.com`.
- **What it sends.** Metadata only (and a capped text body with `#keep`), POSTed to the server with an HMAC
  signature.
- **Failures.** If the server is unreachable, it retries through a Cloudflare Queue for up to 23 h. When it
  gives up, a dead-letter queue and a failure notice tell the owner. If no notice can be sent, the
  metadata goes to an R2 bucket for 30 days, to be re-ingested with `scripts/reingest.js`.

**The contract, the auth research and every limit are in [`docs/ingest-email.md`](../../docs/ingest-email.md).**

## Layout

| Path | Purpose |
|---|---|
| `src/index.js` | `email` and `queue` handlers |
| `src/address.js` | Allowlist, plus address, From parsing |
| `src/auth.js` | `Authentication-Results` parsing and the alignment check |
| `src/subject.js`, `src/body.js` | Subject cleanup and `#keep`, HTML to text, forwarded-sender extraction |
| `src/payload.js` | Capped raw read, MIME parsing (postal-mime), payload building |
| `src/sign.js`, `src/deliver.js` | Signing, POST, failure classification, retry schedule |
| `src/log.js` | Log lines with an allowlist of fields |
| `src/limits.js` | Every bound in one place |
| `scripts/reingest.js` | Re-ingests dead letters archived in R2 (run by hand, not deployed) |
| `wrangler.example.toml` | Config template (placeholders only) |

The code is plain JavaScript ES modules with JSDoc types. It has no build step. The only runtime dependency is
[`postal-mime`](https://www.npmjs.com/package/postal-mime), pinned to an exact version. It has no
dependencies of its own. Wrangler or `cf` bundles it into the Worker at deploy time.

## Tests

The tests need Node 22 or later; CI uses the version pinned in `.github/workflows/ci.yml`. They have no dev
dependencies.

```sh
cd workers/email-ingest
npm ci
npm test
```

## Configuration

| Name | Kind | Purpose |
|---|---|---|
| `INGEST_HMAC_SECRET` | secret | Shared with the server, at least 32 characters, such as `openssl rand -hex 32` |
| `ALLOWED_SENDERS` | secret | Comma-separated allowed sender addresses |
| `INGEST_URL` | var | `https://todo.example.com/api/v1/ingest/email` |
| `RECIPIENT_SECRET` | secret | Optional, exactly 16 to 64 random lowercase letters and digits (`openssl rand -hex 12`), with no spaces. Any other value, even an empty one, rejects all mail. When set, only `todo-<secret>@` and `todo-<secret>+<list>@` are accepted. See "Allowlist and exact alignment, or a recipient secret too" in the contract doc. |
| `INGEST_LOCAL_PART` | var | Optional, default `todo` |
| `MAX_ACCEPTED_PER_DAY` | var | Optional, default 50: accepted emails per isolate in any 24 h window, sized to the Queues free quota |
| `DEBUG_AUTH_SHAPE` | var | Optional. `"true"` logs which parts of `Authentication-Results` passed and aligned, as booleans only |
| `DEAD_LETTER_QUEUE` | var | Optional, default `todo-email-ingest-dlq`; only if you rename that queue |
| `DEAD_LETTER_MAX_RETRIES` | var | Optional, default 10; keep it equal to the dead-letter consumer's `max_retries` |
| `DEAD_LETTERS` | R2 bucket binding | Optional but recommended: the last-resort archive when no notice can be sent |
| `NOTIFY_ADDRESS`, `NOTIFY_FROM` | vars | Optional. Failure notices go to a verified destination address and are sent from the routing domain. |
| `INGEST_QUEUE` | queue producer | Retry queue, consumed by this same Worker |
| (consumers) | queue consumers | The retry queue (`max_retries = 30`, `dead_letter_queue` set) and the dead-letter queue, both handled by this Worker |
| `NOTIFY` | `send_email` binding | Optional, restricted to `NOTIFY_ADDRESS` |

**The real configuration never goes into this repo.** Copy `wrangler.example.toml` to `wrangler.toml` and
fill it in. Git ignores `wrangler.toml`, `cloudflare.config.ts`, `wrangler.config.ts` and `.secrets*.json`
here. Keep the filled-in copy in the private deployment repo.

## Deploy

Everything stays on the Workers Free plan:

- Email Routing, Workers, Queues (10,000 operations per day, 24 h retention);
- `send_email` to a verified destination address.

The Free plan has no payment method, so nothing here can be billed. Running out of a quota only makes
mail fail temporarily until the daily reset; see "Quota" in the contract doc.

The commands use placeholders. Replace `example.com`, `in.example.com`, `you@example.com` and
`<account-id>`.

### 1. One-time account setup (`cf`)

The `cf` resource commands work without any project configuration. Sign in with `cf auth login`, or set
`CLOUDFLARE_API_TOKEN`. Pass the zone with `-z example.com`, or set `CLOUDFLARE_ZONE_ID`.

```sh
# Email Routing on the subdomain: it adds the MX and SPF records for in.example.com.
# (Dashboard alternative: Email Service → Email Routing → Settings → Subdomains.)
cf email-routing enable -z example.com --name in.example.com

# Let todo+<list>@ match the todo@ rule.
cf email-routing settings edit -z example.com --support-subaddress

# The destination address for failure notices. Click the link in the verification email.
cf email-routing addresses create --email you@example.com

# The retry queue and its dead-letter queue.
cf queues create --queue-name todo-email-ingest-retry
cf queues create --queue-name todo-email-ingest-dlq

# The dead-letter archive, with a rule that deletes its objects after 30 days. R2 must be enabled on
# the account; this stays far inside its free tier.
cf r2 buckets create --name todo-email-dead-letters
npx wrangler@4.158.0 r2 bucket lifecycle add todo-email-dead-letters expire-dead-letters dead-letters/ --expire-days 30
```

The lifecycle rule can also be set with `cf`. Note that `update` **replaces** all of the bucket's rules, and
`maxAge` is in seconds:

```sh
cf r2 buckets lifecycle update todo-email-dead-letters --rules '[{"id":"expire-dead-letters","enabled":true,"conditions":{"prefix":"dead-letters/"},"deleteObjectsTransition":{"condition":{"type":"Age","maxAge":2592000}}}]'
cf r2 buckets lifecycle get todo-email-dead-letters
```

### 2. Deploy the Worker

#### With `cf`

`cf deploy` reads `cloudflare.config.ts`, not `wrangler.toml`. `cf migrate` converts one into the other.
It also adds `cf` and Wrangler (as the bundler) to `package.json`.

To keep this repo's `package.json` unchanged, run it in a scratch copy outside the repo. `cf migrate`
refuses to write while `git status` shows changes, so give the copy a fresh repository of its own.

```sh
DEPLOY_DIR="$(mktemp -d)"
cp -r workers/email-ingest/. "$DEPLOY_DIR" && rm -rf "$DEPLOY_DIR/node_modules"
cp /path/to/private/wrangler.toml "$DEPLOY_DIR/wrangler.toml"   # filled-in copy of wrangler.example.toml
cd "$DEPLOY_DIR"
git init -q && git add -A && git commit -qm "deploy snapshot"
npm ci
npm install --save-dev wrangler@latest   # cf supports only the latest Wrangler release
cf migrate --dry-run && cf migrate     # writes cloudflare.config.ts and wrangler.config.ts
cf deploy --dry-run

# Secrets go up with the version. The file never enters the repo; delete it afterwards.
cat > .secrets.json <<'EOF'
{ "INGEST_HMAC_SECRET": "<64 hex characters>", "ALLOWED_SENDERS": "you@example.com, work@example.org",
  "RECIPIENT_SECRET": "<24 hex characters, optional>" }
EOF
cf deploy --secrets-file .secrets.json
rm .secrets.json
```

To change one secret later:

```sh
cf workers secrets update ALLOWED_SENDERS --worker todo-email-ingest --text "<value>"
```

Prefer `--secrets-file`, because `--text` lands in your shell history.

#### With Wrangler (alternative)

Run these from `workers/email-ingest`, with the filled-in `wrangler.toml` in place:

```sh
npm ci
npx wrangler@4.158.0 secret put INGEST_HMAC_SECRET     # prompts for the value
npx wrangler@4.158.0 secret put ALLOWED_SENDERS
npx wrangler@4.158.0 secret put RECIPIENT_SECRET       # optional
npx wrangler@4.158.0 deploy
# (the queues can also be created with: npx wrangler@4.158.0 queues create <name>)
```

### 3. Route mail to the Worker (`cf`)

Create one rule for `todo@in.example.com`. With subaddressing on, it also matches every
`todo+<list>@in.example.com`, and the Worker reads the `+<list>` part from the envelope recipient.

```sh
cf email-routing rules create -z example.com \
  --name "todo to email-ingest" \
  --matchers '[{"type":"literal","field":"to","value":"todo@in.example.com"}]' \
  --actions '[{"type":"worker","value":["todo-email-ingest"]}]'
```

With `RECIPIENT_SECRET` set, use `todo-<secret>@in.example.com` as the rule's value instead, and don't
create a rule for plain `todo@`. The routing rule then holds the secret, which stays inside your
Cloudflare account.

You can also do this in the dashboard: Email Service → Email Routing → Routing rules → Custom address →
"Send to a Worker".

### 4. Connect the server

The server's half is `POST /api/v1/ingest/email` in `todo-server` (`crates/todo-server/src/ingest`). It
is off (404) until it has the secret.

1. Put the same secret in an owner-only file on the server host, owned by and readable only by the
   account the server runs as. The server refuses to start if anyone else can read it, if another
   account owns it, or if it is a symlink rather than a regular file.

   ```sh
   # Linux: create it owner-only, then paste the value the Worker got as INGEST_HMAC_SECRET.
   (umask 077; read -rs s && printf '%s\n' "$s" > /etc/todo/ingest-email-hmac.secret)
   # Windows (PowerShell, as the server's account): write it, then drop inherited access.
   #   Set-Content C:\todo\ingest-email-hmac.secret -NoNewline -Value (Read-Host -MaskInput)
   #   icacls C:\todo\ingest-email-hmac.secret /inheritance:r /grant:r "${env:USERNAME}:F"
   ```

2. Point the server config at it, and seed the allowlist (used once; afterwards the admin page is
   the server's source of truth):

   ```toml
   [ingest]
   email_hmac_secret_file = "/etc/todo/ingest-email-hmac.secret"
   allowed_senders = ["you@example.com", "work@example.org"]
   ```

   Or set `TODO_SERVER_INGEST_EMAIL_HMAC_SECRET_FILE`. Restart the server; it logs
   `email ingest is enabled`.
3. Set the Worker's `INGEST_URL` to `https://todo.example.com/api/v1/ingest/email` (the server's
   public origin plus that path) and deploy it.
4. On the server's admin page, **Email** (`/admin/ingest`): **pick the Inbox** (until you do, the server
   answers 503 and the Worker keeps retrying), check the allowed senders, and give lists keys for
   `todo+<key>@`. Only keys set there work; list names are never matched, and a key that names no list
   (or a deleted one) goes to the Inbox with a note. Agents that can read a list see the subject, sender
   and kept body of email tasks in it. Changes need a fresh passkey check and are audit-logged.

Keep the Worker's `ALLOWED_SENDERS` and the server's allowlist in step: the Worker drops strangers at
SMTP time, and the server re-checks, so a leaked HMAC secret alone can't create tasks for other
senders.

### 5. Check it

1. Forward a test email from an allowed address.
2. Watch the logs with `npx wrangler@4.158.0 tail todo-email-ingest`, or in the dashboard. You should see
   `{"evt":"delivered",…}`.

**Check one real message once with `DEBUG_AUTH_SHAPE = "true"`.** It logs one `auth_shape` line of booleans,
such as `"dkim_pass":true,"header_d_aligned":true`, and never the header itself. If a legitimate forward is
rejected with `auth_unaligned`, this shows which part didn't match. Turn it off again afterwards. See
"What Cloudflare provides" in the contract doc.

## Re-ingest dead letters

If the logs show `dead_letter_archived`, the server didn't get that email and no notice reached you. The
metadata is in R2 for 30 days.

1. **List the archive.** Keys are `dead-letters/<YYYY-MM-DD>/<hash>.json`, where the hash is the `mid` in
   the log line:

   ```sh
   cf r2 objects list --bucket-name todo-email-dead-letters --prefix dead-letters/
   ```

2. **Download what you need.** Use the dashboard (R2 → the bucket → Objects), or:

   ```sh
   mkdir -p dead-letters
   npx wrangler@4.158.0 r2 object get todo-email-dead-letters/dead-letters/2025-11-21/abcdef012345.json \
     --file dead-letters/abcdef012345.json --remote
   ```

3. **Re-ingest, once the server is reachable.** Run this from `workers/email-ingest`. It needs Node 22 or
   later and no `npm ci`. `read -s` keeps the secret out of your shell history:

   ```sh
   read -rs INGEST_HMAC_SECRET && export INGEST_HMAC_SECRET
   INGEST_URL=https://todo.example.com/api/v1/ingest/email node scripts/reingest.js dead-letters/*.json
   ```

   It prints one line per file, such as `dead-letters/abcdef012345.json: ok (HTTP 201)`, and exits with
   a non-zero code if any file failed.
   - The server dedupes on `message_id`, so re-running it is safe.
   - A file that is already in, or arrived after all, comes back `ok (HTTP 200)`.
4. **Delete the local copies.** They hold the task title, subject and senders.

The lifecycle rule removes the archived objects after 30 days.
