# Server admin gallery

These are captures of the real server-rendered pages, populated through a disposable loopback server. All names, lists, addresses and displayed pairing codes are fixtures. No production account or server is used.

Each view is captured in light and dark at 1440 px and 412 px. The Playwright run also writes a 300%-scale keyboard-focus crop for each capture in its test output directory. The full views below are checked in; the crops are generated review artifacts.

| Capture prefix | Focus crop |
| --- | --- |
| `login-first-run`, `login`, `pair-sign-in` | Sign in with passkey |
| `setup` | Setup code field |
| `overview` | Passkey name field |
| `devices` | Revoke disclosure |
| `devices-details` | Confirm revoke button; device ID and confirmation are open in the full screenshot |
| `agents` | Agent name field |
| `agent-grants` | List access selector |
| `shares` | List or collection selector |
| `email` | Email address field |
| `audit` | Event details disclosure |
| `pair` | Pairing code field |
| `pair-approval` | Approve device button |
| `verify`, `step-up-needed` | Verify with passkey |
| `invite` | Device name field |

From the repository root, build the server and web app, then run the gallery (requires Playwright Chromium):

```sh
cargo build -p todo-server
node apps/web/scripts/build.mjs
cd apps/web/e2e
npm ci
npx playwright install chromium
npx playwright test -c admin.config.mjs
```

The gallery uses its own disposable data directory and port (`ADMIN_GALLERY_PORT`, default 27413).
