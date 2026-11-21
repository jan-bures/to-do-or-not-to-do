# email-ingest Worker

A Cloudflare Email Worker that turns forwarded emails into tasks.

- **Address.** Mail to `todo@in.example.com` goes to the Inbox, and mail to `todo+<list>@in.example.com` goes
  to that list.
- **Checks.** It accepts mail only from allowlisted senders whose SPF, DKIM or DMARC passed in alignment with
  the From domain.
- **What it sends.** Metadata only (and a capped text body with `#keep`), POSTed to the server with an HMAC
  signature.
- **Failures.** If the server is unreachable, it retries through a Cloudflare Queue for up to 24 h.

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
| `INGEST_LOCAL_PART` | var | Optional, default `todo` |
| `NOTIFY_ADDRESS`, `NOTIFY_FROM` | vars | Optional. Failure notices go to a verified destination address and are sent from the routing domain. |
| `INGEST_QUEUE` | queue producer | Retry queue, consumed by this same Worker |
| `NOTIFY` | `send_email` binding | Optional, restricted to `NOTIFY_ADDRESS` |

**The real configuration never goes into this repo.** Copy `wrangler.example.toml` to `wrangler.toml` and
fill it in. Git ignores `wrangler.toml`, `cloudflare.config.ts`, `wrangler.config.ts` and `.secrets*.json`
here. Keep the filled-in copy in the private deployment repo.

## Deploy

Everything stays on the Workers Free plan:

- Email Routing, Workers, Queues (10,000 operations per day, 24 h retention);
- `send_email` to a verified destination address.

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

# The retry queue.
cf queues create --queue-name todo-email-ingest-retry
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
{ "INGEST_HMAC_SECRET": "<64 hex characters>", "ALLOWED_SENDERS": "you@example.com, work@example.org" }
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
npx wrangler@4.158.0 deploy
# (the queue can also be created with: npx wrangler@4.158.0 queues create todo-email-ingest-retry)
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

You can also do this in the dashboard: Email Service → Email Routing → Routing rules → Custom address →
"Send to a Worker".

### 4. Check it

1. Set the same `INGEST_HMAC_SECRET` and allowlist on the server.
2. Forward a test email from an allowed address.
3. Watch the logs with `npx wrangler@4.158.0 tail todo-email-ingest`, or in the dashboard. You should see
   `{"evt":"delivered",…}`.

**Check one real `Authentication-Results` header once.** See "What Cloudflare provides" in the contract
doc. If a legitimate forward is rejected with `auth_unaligned`, look at the header (temporarily) before
changing anything.
