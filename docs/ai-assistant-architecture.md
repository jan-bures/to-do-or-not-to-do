# In-app assistant: architecture proposal

> **Current transport:** The conversation, scoped tools, and proposal review still run on the client.
> The original direct tailnet transport and local ChatGPT credential store below have been superseded
> by the optional [single-owner server relay](hosting-linux.md#ai-assistant-relay). The relay stores
> credentials and sharing rules, not transcripts or tools. Direct inference is loopback-only. Device-code
> sign-in is opt-in and requires a provider-approved registration; do not reuse Codex’s registration.
> Use the ChatGPT integration only where OpenAI permits the owner-controlled deployment.

**Original decision:** Run the conversation and tool loop in `todo-client`, on the owner's device. Make the
owner's OpenAI-compatible Mac on the tailnet a first-class provider. Send only approved commands to
`todo-server`. Start with no provider enabled, no paid calls, and no transcript sync. Keep the owner's
passkey/device sign-in separate from model-provider authorization. This design covers plumbing; the
companion UX specification owns presentation.

## 1. Runtime and data flow

| Where the loop could run | Trade-off |
|---|---|
| **Shared `todo-client` (recommended)** | The task replica, validation, limits and proposal state machine are shared. The device reaches the Mac directly; cloud requests go straight to the configured provider or an explicitly configured owner-run gateway. The browser has extra transport constraints, not a different agent. |
| `todo-server` | Easier same-origin web transport and centralized secrets, but every prompt/result passes through the public VM; it cannot reach the tailnet. It would hold inference streams and transcripts alongside sync and backups on a [1 GB e2-micro](hosting-linux.md). Do not join the VM to the tailnet, add a model proxy, or create a server-side conversation service. |
| Per client | Can use OS-specific HTTP and secure storage, but duplicates tool execution, privacy policy, approval and retry semantics. Keep only adapters here. |

A turn pins a provider, model, transcript, read scope and a consistent snapshot of **confirmed** data.
A deliberate Send starts inference; the host streams text, assembles complete typed tool calls, and
answers permitted reads from the replica. Proposal calls build a private draft, never a write. The UI
shows a trusted diff after a complete turn. Only the local Approve action can submit the frozen draft
through the command outbox. The server commits the group and broadcasts normal sync events; the owner
can undo it in Activity. No model response ever goes directly into `Client::submit` or a REST call.

- **Desktop:** GPUI subscribes to a Rust chat controller; native async HTTP on the existing runtime.
- **Android:** `todo-ffi` owns the controller on its Rust runtime, like its existing
  [engine/listener](../crates/todo-ffi/src/engine.rs). Export commands such as send, cancel, approve and
  discard, plus typed/coalesced updates; Compose observes them on its UI dispatcher. Do not call across
  UniFFI per token, expose provider credentials in view data, or start background inference on rotation.
- **Web:** Leptos uses the same controller on the browser event loop. Browser `fetch` + `ReadableStream`
  and `AbortController` handle authenticated streaming **POSTs**. `EventSource` is not a fit. Reuse the
  bounded [SSE framing](../crates/todo-client/src/sse.rs) where appropriate, but not sync's cursor,
  reconnect rules, `FetchTransport` cookie/CSRF settings or native bearer transport. Keep provider
  clients separate from the todo transport so neither sends credentials to the other's origin.
- **Offline:** If the Mac is reachable but todo sync is down, a warm client can answer from its last
  confirmed snapshot, labeled stale, and draft proposals. Without a ready snapshot, do not expose task
  tools. Without a reachable inference endpoint, do not queue prompts or invent a response. For the
  first release, enable Approve only when the replica is caught up, ordinary pending edits have drained,
  and the reviewed revision still matches. If connectivity drops *after* approval, reconcile through
  the existing durable outbox with the same ID. No silent rebasing or new approval on reconnect. A warm
  web tab can use its replica; this does not add a cold offline boot or change session revocation.

### Provider boundary

Add a platform-neutral `Provider` trait in `todo-client` with `list_models`, `capabilities(model)`,
`check_connection`, and `stream_turn(request, cancel)`. Pass the selected credential via an injected
adapter and keep it out of request/view types that the controller logs or serializes. Use the existing
[`MaybeSend`/`MaybeSync` pattern](../crates/todo-client/src/platform.rs): native futures can be `Send`,
wasm futures need not be; no Tokio, filesystem, credential store or raw sockets in the default build.
The normalized stream carries text deltas, completed tool calls (opaque call ID, stable tool name,
strictly parsed arguments), optional usage, and a terminal success/failure. Keep protocol-specific
history items necessary for tool continuation inside the adapter, not in the UI or sync log.

Capabilities are per endpoint **and** model: streaming, structured tool calling (including whether
streaming and tools work together), parallel calls (off initially), output limits, usage reporting,
model listing and data/billing class. Listing does not prove tool support. With explicit user consent,
probe using an invented task and a no-effect tool, or use known-good model/template fixtures. If tools
work only without streaming, buffer a bounded round; otherwise provide a clearly labeled text-only
mode with no task access or proposals. Never infer a command from prose or from an incomplete stream.
Bound prompts, tool schema size, context/result bytes, calls, rounds, wall time, idle time and output;
allow one active turn per chat. Cancel on Stop, sign-out, destruction of the controller or provider
change. A provider timeout may already have consumed usage: don't replay model requests automatically.

## 2. Shared tools, proposal and command commit

### One schema, separate authority

[`mcp/tools.rs`](../crates/todo-server/src/mcp/tools.rs) adapts the shared tool contracts to REST reads
and writes. MCP is bearer-only and rejects browser origins; it uses stateless JSON responses, not the
task SSE stream
([transport](../crates/todo-server/src/mcp/mod.rs)). REST can also use the owner's same-origin session,
CSRF header and write-origin check ([auth middleware](../crates/todo-server/src/auth/mod.rs)). Do not
loosen MCP's policy, issue an assistant bearer, or send an owner bearer to a model.

The runtime-free [`todo-tools`](../crates/todo-tools/src/lib.rs) crate owns typed arguments, descriptions,
canonical JSON Schema, and query/command conversion. It does not depend on `rmcp`, axum, SQLite or a
provider SDK. The MCP adapter keeps its transport policy, retry semantics and response/error envelopes;
byte-level snapshots cover `tools/list` and representative errors. The caller supplies the clock, IDs
and explicit list resolution. The assistant's restricted tool inputs are derived from that same schema
and reject transport metadata such as `command_id` (the host generates it). Provider adapters may wrap
the schema in native tool envelopes. No second hand-maintained schema or raw model-authored `Command` JSON.

[`Projection`](../crates/todo-tools/src/assistant/projection.rs) accepts a confirmed state and a live
list allowlist supplied by the host. It filters before query evaluation, ordering, paging and counts.
Assistant free-text queries search visible titles and checklist steps, not notes, so matches do not
become an oracle for omitted note text. Local cursors bind to scope and resolve only visible IDs.

| Tool class | In-app dispatch |
|---|---|
| `list_lists`, `query_tasks`, `get_task` (later collections/tags as needed) | Read through an assistant-scoped projection of the replica. Reuse core query evaluation and relevant caps from [REST reads](../crates/todo-server/src/api/read.rs). Filter **before** search, sort, pagination, aggregates and direct-ID lookup. Return a smaller view than REST where needed for privacy. |
| Initially `create_task`, `update_task` (title/due/priority only), `complete_task`/reopen (non-repeating) | Same typed arguments/normalization, limited to the advertised options. The chat description makes clear that these *propose* changes. The host returns `drafted`, not REST's `applied`/event receipt. Unsupported args and forms fail validation. |
| Later notes, movement, reminders, repeat, merge or archive | Add individually after verifying complete diffs, scope and undo for their side effects. Global tag creation, arbitrary Activity reads, `undo_event`, attachments, structure/import/admin, URL fetching and external tools are not in the initial surface. Owner undo uses the app, not a model call. |

### From draft to confirmed group

1. **Read and simulate.** Capture confirmed `(epoch, cursor, state)` together under the replica lock;
   current `Client::view()` overlays pending optimistic edits and is deliberately not the base. Once
   tools have read this snapshot, never silently reinterpret the turn against another. Resolve relative
   dates, the permitted Inbox and entity IDs as the draft is built; use stable host-generated IDs for
   newly created tasks and all cross-command references. Use only IDs visible to the scoped projection
   or created in this draft. Sequentially run core `decide` and `apply` on a scratch snapshot, using
   restricted assistant grants. Validate every later command on the earlier ones' results. Nothing is
   enqueued; a malformed/incomplete call, failed simulation or aborted turn can't yield an approvable
   partial plan. Bound both command count and induced event count.
2. **Freeze and show effects.** An in-memory pending record contains a host-generated proposal/command
   ID, schema version, epoch, cursor, scope revision, ordered `Vec<Command>`, normalized effects and a
   digest binding the commands/revision to the effects shown. Derive the diff from core events and
   before/after state, not model wording: name the source and destination lists, show exact changes
   and any user-visible side effects, including transient/destructive steps if a final-state diff
   would hide them. Ignore generated event IDs, bookkeeping timestamps and rank-only changes when
   hashing effects, but include semantic order/membership, explicit dates and body fields. The digest
   also binds the frozen decision timestamp and the versioned batch-ID/command-ordinal seed scheme.
   Drop an all-no-op batch. An edit, subset selection or regenerated plan makes a new preview for approval;
   the existing approval never follows it. Unapproved drafts are not synced or written to disk.
3. **Enqueue exactly once.** An owner UI action on the current proposal ID/digest consumes that
   approval under the client lock. Extend `todo-client` with an approved-assistant-batch outbox
   operation, using its existing command ID, storage, receipt and rejection machinery
   ([client](../crates/todo-client/src/client.rs), [replica](../crates/todo-client/src/replica.rs),
   [wire encoding](../crates/todo-client/src/protocol.rs)). Treat this operation explicitly in the
   replica; don’t infer it is undo merely because it isn't a single `Command`. Start non-optimistic:
   show the batch as pending in chat, not as changed tasks, until sync confirms. Persist only this
   explicitly approved operation, not chat/credentials. Duplicate clicks, lost responses and tab
   handover reuse the frozen ID and payload.
4. **Commit atomically.** Add owner-only `POST /api/v1/assistant/approvals` under the existing REST
   admission/deadline/size/CSRF/session rules, using the normal engine and command receipts—not a
   broader MCP grant or N calls to `/commands`. Authorize the paired owner or owner browser session
   first; reject agent tokens. Assign a **reserved, server-selected** `Actor::Agent` displayed as
   **In-app assistant**. Reject provisioning the reserved identity or confusable external IDs. The
   authenticated owner is distinct from the event actor. Store the first approving device ID or
   a domain-separated hash for a browser session in the receipt and expose it only to the owner in
   Activity. For v1 the client owns list choices; bind at most 64 live list IDs into the operation
   and derive explicit list grants for the reserved actor in the scratch state only (`None` by
   default). Cheap structural validation runs before acquiring the writer. Independently allowlist supported command forms and
   enforce target/effect scope, including both sides of any future cross-list move. Never accept an
   actor, elevated grant, or write scope from model output. The route trusts the authenticated first-party
   client to report a human approval; owner auth alone is not cryptographic user presence and does
   not protect against a compromised owner session/device. No provider/tool code can call the route.
5. **Check at the writer and acknowledge.** Under the single writer, check the epoch, then look up a
   committed command ID and verify its canonical payload hash (same bytes/meaning, same result);
   only for a new command, require `expected_head == head` and accept the frozen timestamp only if
   it is no older than 24 hours, no more than five minutes ahead of the server, and no earlier than
   the latest event. Re-decide the whole batch as the reserved actor and compare the computed
   effect digest. Check normal access, hidden-target, commit and
   cancellation rules. Use distinct server event seeds per command (e.g. batch ID + index) or distinct
   writer timestamps: current `decide` seeds IDs from actor, command and time, so identical commands
   within one timestamp must not collide. Give all events one group and write it, projections and
   command result in **one** transaction. This follows [`Engine::execute_checked` and `execute_unique`](../crates/todo-server/src/engine.rs):
   the latter already plans a group for email ingest, but its dedupe key, task result and generated
   command ID are not the approval protocol. Do not broadcast before commit. Use the existing command
   batch indices/counts so `Replica::apply_confirmed` holds a partial SSE batch until it is complete.
   Activity/undo uses the normal actor, grouped events and owner undo REST path (epoch and event ID,
   non-forced conflict checks); add the reserved display name in [shared Activity](../crates/todo-client/src/activity.rs).

The initial global head check is deliberately conservative: even an unrelated event can invalidate
an approval, but a changed task, removed list, intervening write or restored log can never sneak in
between preview and commit. On mismatch, return a typed conflict, sync, then let the owner request a
fresh proposal from current data and approve again; do not change the already approved payload.
Echo no hidden data in errors. Revisit per-entity/field or query dependency fingerprints only if
unrelated conflicts prove disruptive. On disconnect after sending, reconcile the existing command ID
first (including `outcome_unknown`); a committed response can replay even after newer events, while a
restored epoch must never accept a previously approved batch again.

## 3. Provider choices

### A local Mac on the tailnet: first release

Use OpenAI-compatible `GET /v1/models` and `POST /v1/chat/completions` with function calls and SSE.
Support one explicitly configured API base, e.g. `https://mac.tailnet.example/v1`. Add `/v1` if absent;
append relative `models` and `chat/completions` without losing a gateway path. No discovery, hard-coded
host, model downloads, redirects, system HTTP proxies or model-selected URLs. Reject embedded
credentials, queries, fragments and invalid ports. A separate optional endpoint key is sent as
`Authorization: Bearer …`; a 401 or 403 means the key is required or invalid and never affects todo
sign-in. Pick a discovered model ID or enter it manually if listing is unavailable. The explicit
**Test connection** fetches only the catalog with a short deadline. A separate explicit **Test model**
uses invented data and a no-effect function to check the selected model's streaming tool/result round
trip, then buffered tools or text only on protocol incompatibility. It does not retry after a network,
authentication, or usage-limit error. An untested model is text only; listing does not grant task access.
Don't confuse a warm HTTP listener with a valid model or a functioning stream. Distinguish observed
failures (network/timeout, TLS, unauthorized, missing model, incompatible tools; on web, possibly only
a generic network failure). If the Mac sleeps or disconnects, keep the unsent draft, show **Local
provider unavailable**, and offer Test/Retry or an explicit switch. Don't wake it, poll indefinitely,
change destination, resend the transcript or fall back to a cloud model automatically.

- **Mac setup:** Bind inference to loopback, allow only the owner's devices via tailnet grants and
  use [Tailscale Serve](https://tailscale.com/kb/1312/serve), never Funnel. Enable MagicDNS and
  [HTTPS certificates](https://tailscale.com/kb/1153/enabling-https); the `.ts.net` certificate name is
  publicly logged in Certificate Transparency even though the endpoint stays private. Use a generic
  node name. Serve can forward a loopback HTTP port (for example `tailscale serve --bg 11434` for
  Ollama, or forward the port of a small Mac-side reverse proxy). Restrict routes to what the chat
  needs; no model pull/manage/admin endpoints via the web gateway. Use a dedicated endpoint token if
  the host/proxy needs additional admission; validate it there, never send a todo credential. No cloud
  fallback on the Mac: use local weights and disable Ollama cloud features if applicable.
- **Windows GPUI:** Connect directly with normal TLS verification for HTTPS. The native provider also
  accepts explicitly entered HTTP endpoints on loopback or Tailscale: `.ts.net` MagicDNS names,
  `100.64.0.0/10`, and `fd7a:115c:a1e0::/48`. Loopback HTTP supports a model running on the same PC.
  The connector filters actual DNS results for local names: loopback stays on loopback and MagicDNS
  must resolve into those ranges. Before each direct HTTP tailnet request (including the catalog),
  the adapter requires a platform check that an up local interface has a Tailscale address. The
  interface check is not proof of a WireGuard route or of peer identity. Use this option only with
  Tailscale active and routing to the intended peer. HTTPS is recommended, especially if that cannot
  be guaranteed. Never expose an HTTP inference listener to an untrusted network. Other remote URLs
  require HTTPS. The todo-server transport and pairing policy are unchanged.
- **Destination and billing:** Loopback and Tailscale destinations are classified as local and not
  billed per token. Other HTTPS destinations count as cloud with unknown billing and are blocked
  before sending a key, a catalog request or inference. To use a private deployment with a custom
  HTTPS address, the owner must attest that it is theirs, uses local weights, keeps data private and
  does not bill per token. This attestation (not a remote capability or billing check) belongs to
  the exact configured URL, lives in the protected device-local profile and is cleared along with
  its key and scope on URL changes. `Private only` lists are never included for cloud destinations.
  A rate-limit wait hint is capped at five minutes and never schedules an automatic retry.
- **Android Compose/FFI:** The device needs the Tailscale VPN active and the app routed through it,
  including on mobile data or when LAN routing is unavailable. Use the same Serve HTTPS URL. Android
  release policy [denies general cleartext](../apps/android/app/src/main/res/xml/network_security_config.xml).
  The first Android UI should retain HTTPS; do not bypass Android's restrictions with the native
  resolver or add a broad manifest exception. Do not trust all certificates. The app already targets SDK 37; test real Android
  17 with a routed tailnet address and with a LAN address. Where
  [local network protection](https://developer.android.com/privacy-and-security/local-network-permission)
  applies, request `ACCESS_LOCAL_NETWORK` only on explicit setup/connect and handle denial; don't
  assume the same permission behavior for VPN routes and LAN Wi-Fi.
- **Leptos web:** The browser/device, not the VM serving `https://todo.example.com/app`, must be on
  the tailnet. Plain HTTP fetch from HTTPS is [mixed content](https://developer.mozilla.org/en-US/docs/Web/Security/Mixed_content);
  don't rely on browser exemptions. The HTTPS endpoint must grant
  [CORS](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/CORS) to the **exact** app origin
  `https://todo.example.com`: handle `OPTIONS` for `GET`/`POST`, allow `Content-Type` and optional
  `Authorization`, and send the matching `Access-Control-Allow-Origin` on GET/POST, streams and errors.
  Handle preflight without expecting a token; enforce auth on the actual call. Use `Vary: Origin`
  if the response varies and forward SSE without buffering. Use no cookies (`credentials: omit`), no
  `no-cors` or wildcard allowlist. Ollama has `OLLAMA_ORIGINS`; alternatively configure these headers
  on a loopback proxy before Serve. Origin is a browser policy, not authentication. Chrome's
  [Local Network Access](https://developer.chrome.com/blog/local-network-access) may require user
  permission depending on version and resolved address; test on the actual Tailscale path and intended
  browsers. TLS and CORS alone do not guarantee browser compatibility.

**Web CSP is a separate gate:** [`APP_CSP`](../crates/todo-server/src/web/spa.rs) currently sends
`connect-src 'self'`. For web support, add an optional runtime-configured, exact inference **origin**
(HTTPS scheme, host and port), validated when rendering the authenticated app page. The owner enters
it in the deployment config (not checked in) and selects the matching API base in the browser. No
fetch to validate the origin from the VM and no relaxation for other pages or directives. Changing
an endpoint in browser settings cannot override this header. Keep browser support native-only for
that endpoint until HTTPS, CSP, CORS and browser/device policy all pass; give an actionable reason
instead of adding a public relay. Any Mac-side proxy is within the tailnet, not on the cloud VM.

Compatibility is not interchangeable: [Ollama](https://docs.ollama.com/api/openai-compatibility)
([tool calls](https://docs.ollama.com/capabilities/tool-calling), [network/cloud settings](https://docs.ollama.com/faq)),
[LM Studio](https://lmstudio.ai/docs/developer/openai-compat)
([tool parsing](https://lmstudio.ai/docs/developer/openai-compat/tools)),
[llama.cpp](https://github.com/ggml-org/llama.cpp/tree/master/tools/server) and
[MLX LM](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/server.py) expose compatible endpoints
with different coverage. The checkpoint, quantization, tool parser and chat template matter; small
models may emit malformed arguments, repeat tools or put calls in text. Test single-tool argument and
result round trips before enabling actions; don't recover by executing text that looks like JSON.
Apple's [Foundation Models](https://developer.apple.com/documentation/foundationmodels) is a native
Apple-platform API, not this HTTP provider. A Swift bridge on a compatible Apple Intelligence-enabled
Mac could implement the same adapter contract; with no macOS client, it adds little for v1.

### ChatGPT subscription: supported route exists; do not copy the Codex route

**Public docs checked 2026-11-23.** OpenAI now documents
[Sign in with ChatGPT (SIWC)](https://developers.openai.com/siwc/quickstart), including an official
[open-source ChatGPT plan usage flow](https://developers.openai.com/siwc/token-sharing-open-source)
for eligible Plus/Pro users. Website identity sign-in is not permission to run inference; commercial
partner access is a limited trial. Connecting this provider does not sign the owner into the todo app
or grant access to conversations, a platform API key or unrelated model APIs.

| Flow | What it does and what we should do |
|---|---|
| **First-party Codex CLI** | [Official CLI docs](https://developers.openai.com/codex/auth) describe browser sign-in, beta device code for headless use (account/workspace enablement), and automatic refresh. The [browser source](https://github.com/openai/codex/blob/3af06341bd6fe34b3e092dba4a1c5af1cb911ee0/codex-rs/login/src/server.rs) uses auth.openai.com `/oauth/authorize`, PKCE and a loopback callback; its [device source](https://github.com/openai/codex/blob/3af06341bd6fe34b3e092dba4a1c5af1cb911ee0/codex-rs/login/src/device_code_auth.rs) gets a code and polls `/api/accounts/deviceauth/{usercode,token}`, then exchanges the result as a code grant. The [auth manager](https://github.com/openai/codex/blob/3af06341bd6fe34b3e092dba4a1c5af1cb911ee0/codex-rs/login/src/auth/manager.rs) refreshes through `/oauth/token`, replacing rotated credentials. The [subscription default](https://github.com/openai/codex/blob/3af06341bd6fe34b3e092dba4a1c5af1cb911ee0/codex-rs/model-provider-info/src/lib.rs) uses the ChatGPT `backend-api/codex` Responses route, distinct from public API-key usage. This is evidence about the CLI, not our integration contract. |
| **Open source in practice** | Pi's legacy [`openai-codex` OAuth](https://github.com/earendil-works/pi/blob/c2f3a5cea11b4394ff19231d5be7317c351009c6/packages/ai/src/auth/oauth/openai-codex.ts) uses the Codex client registration and browser/device flows; its [adapter](https://github.com/earendil-works/pi/blob/c2f3a5cea11b4394ff19231d5be7317c351009c6/packages/ai/src/api/openai-codex-responses.ts) uses the ChatGPT backend and an account header. Pi also has a distinct [`openai-chatgpt` implementation](https://github.com/earendil-works/pi/blob/c2f3a5cea11b4394ff19231d5be7317c351009c6/packages/ai/src/auth/oauth/openai-chatgpt.ts) for the documented registration flow. The existence of the legacy code or an OSS license grants no OpenAI service permission. |
| **Official open-source option for this app** | [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in): use our own app name, the documented dynamic registration and stable opaque host identifier (one per desktop pairing runtime), system-browser authorization-code + S256 PKCE with a loopback listener, fresh state/nonce and verification of ID-token signature/claims and granted inference scopes. The current endpoints use `/api/accounts/authorize` and `/api/accounts/oauth/token` on auth.openai.com, **not** the CLI registration above. Keep the issued client ID and protected local credential record, [serialize token refresh](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions) against the token endpoint, atomically replace rotating tokens, and support revoke/disconnect. The [public inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) uses `GET https://api.openai.com/v1/models` (SIWC-specific catalog shape) and `POST https://api.openai.com/v1/responses` with `store: false`, `stream: true`; no `backend-api`. Check [preview limits](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations) for function-tool packaging, unsupported output caps/fields, and terminal failures. |

The [SIWC Terms](https://openai.com/policies/sign-in-with-chatgpt-terms/) require using the supported
flow under our own identity, user-controlled local credential storage and runtime, user authorization,
privacy disclosure/revocation, no rate/plan-limit workarounds, no shared inference service or token
resale, and no charge to unlock SIWC in this app. The [consumer terms](https://openai.com/policies/terms-of-use/)
also restrict credential sharing, programmatic extraction and circumventing restrictions; consult the
applicable terms before shipping. Do not borrow the Codex client ID, parse private endpoints as a
contract, import `auth.json`, ask for cookies, or recreate the Codex device flow. Those routes risk
breakage, credential exposure and loss of account or integration access. The public preview can also
change and does not imply every platform or model is supported.

**Windows:** ChatGPT account is a provider in AI settings. It requires explicit sign-in and first-send
consent, uses a separate DPAPI account record, and needs no manual OAuth client ID or secret. The app
sends the stable host UUID as a URN and saves the issued client ID before token exchange, so a failed
exchange can be retried with a new authorization against the same registration. The Cloud adapter
uses the same scoped tools and approval pipeline as the private server. It never receives `Private only`
or `Hidden` lists. Test the protocol with offline fixtures; a real eligible account is a separate owner
validation step. For Android, verify the supported
system-browser/loopback callback, lifecycle and local storage on a real device; don't swap in an
undocumented deep-link or device flow. Defer SIWC in browser builds: the documented open-source flow
expects a local listener and protected credential storage, not a WASM callback, browser token storage
or a managed cloud token broker. Revisit only with a documented supported web/owner-local integration.
If the official flow isn't suitable, use a local model or a separately billed platform API key.

### Plain API keys: supported baseline, opt-in

Use the documented public [OpenAI Responses API](https://developers.openai.com/api/docs/guides/streaming-responses),
[Anthropic Messages](https://platform.claude.com/docs/en/api/overview) with
[streaming](https://platform.claude.com/docs/en/build-with-claude/streaming), or
[OpenRouter-compatible streaming](https://openrouter.ai/docs/api/reference/streaming). Use the
provider's model endpoint (e.g. Anthropic `GET /v1/models`), rather than assuming one JSON dialect.
Native support for owner-entered keys depends on provider guidance. In particular, OpenAI
[advises against deploying keys in browsers or mobile apps](https://help.openai.com/en/articles/5112595-best-practices-for-api-key-safety).
Never ship keys in any binary. Start with an owner-run, tailnet-only gateway on the Mac (or another
owner-controlled host) for providers that require protected upstream keys or cannot serve the browser.
Pin the upstream/provider and models and expose only fixed inference/catalog routes; authenticate the
client and bound usage there. The gateway can stream an OpenAI-compatible wire format, or pass through
a selected provider's protocol with its key supplied server-side. Register the gateway in the client as
**cloud inference**, not as a local model. No arbitrary cloud URL proxy on `todo-server`. Direct native
BYOK can be added for providers that support it, with scoped keys, where the owner accepts the risk.

## 4. Privacy, security and cost

- **Scope before egress.** Opt in per list or collection ID, per provider profile and device. Resolve
  access on the confirmed replica: a list override wins; otherwise the live collection's explicit
  choice applies, including to lists added later. Without a choice the list is hidden. Build a
  projection restricted to allowed live lists; resolve tags, collection names and other metadata only from allowed data.
  Hidden IDs look missing. Omit attachment bytes/URLs, email-link metadata, notes by default and any
  global counts or dependency data from excluded lists; email notes can contain original sender,
  subject and body [by design](ingest-email.md#task-mapping-what-the-server-stores). Prefer small,
  bounded summaries; select fields by tool, never upload the full replica. Invalidate drafts and
  cancel/reset the transcript on exclusion, device sign-out, provider change or restore. Recheck
  referenced tasks against the latest known scope before each round, including tasks moved to an excluded
  list; don't reuse cached tool output or assistant answers from an invalidated scope. A stale offline
  replica cannot know about an unseen remote move. Explain which typed input and fields can leave the
  app; manually typed text cannot be filtered reliably by list.
- **Data is not instructions.** Task/title/note/step/link/model text, including signed email ingest,
  is untrusted. Delimit it in typed tool results, render it safely without executing HTML, remote
  images or automatic links/navigation, and do not let it name new tools, hosts, secrets or actions.
  Permission and length checks are independent of the prompt. Assistant-authored titles normalize
  to NFC and reject control, format, private-use, unassigned and line/paragraph separator characters.
  This rejects emoji sequences containing a zero-width joiner (ZWJ), not standalone emoji.
  Approve protects integrity, not
  confidentiality: reads sent to the chosen provider cannot be undone, and a misleading proposal can
  still induce consent. Show the actual effects and never put a model-supplied approval affordance
  in control of the command path. No file/shell/web tools, hosted MCP, analytics or autonomous jobs.
- **Where data goes.** Local inference sends the prompt/history and scoped tool results to the Mac,
  not solely to the requesting device; make sure the Mac's logs and model runtime do not forward them.
  Cloud inference sends the same context to that provider (and to the gateway and routed upstream, if
  used); telemetry and retention are provider/workspace specific. For example, `store:false` is not
  a zero-retention promise ([OpenAI API data controls](https://developers.openai.com/api/docs/guides/your-data),
  [OpenRouter provider logging](https://openrouter.ai/docs/guides/privacy/provider-logging)). Normal todo
  sync already sends task data to its own server; the assistant adds only the approved command and
  normal events there, not draft prompts, history or keys. No persistent chat history in v1. Don't log
  prompts, arguments, results, secrets or authorization URLs, including on errors or the Mac proxy.
- **Secrets at rest.** Reuse the *pattern*, not the key or record, of desktop's
  [user DPAPI protection](desktop.md#pairing-config-and-sync) and Android's
  [Keystore-backed AES-GCM wrapper](../apps/android/app/src/main/kotlin/app/todoornot/session/SecretCipher.kt).
  DPAPI-sealed owner-only file (or Windows Credential Manager); separate Android alias and ciphertext
  in app-private, non-backed-up storage. Accessible to the local process while in use, not a defense
  against a compromised device. Web has **no** secret vault: don't put long-lived provider keys or any
  SIWC credentials in JavaScript, localStorage, IndexedDB, cookies or build config. Use tailnet grants
  or a dedicated short-lived/session-memory endpoint token if needed; disclose that page script can
  access it. Cloud API keys stay at the owner-controlled gateway for web/mobile. Keep provider creds
  independent of the todo token, outbox, backups and sync. Support Forget/rotate and clear memory on
  disconnect, sign-out and revocation.
- **No cost by default.** Assistant stays off until configured; the local Mac with local weights has
  no per-token fee (hardware/power/network and any tailnet plan still apply). API billing is
  [separate from ChatGPT](https://help.openai.com/en/articles/8156019-how-can-i-move-my-chatgpt-subscription-to-the-api).
  SIWC may consume an existing eligible plan's shared allowance or credits; link to Usage Settings,
  stop on the limit, and require credit use to be off in ChatGPT settings for included-only mode. Do
  not promise no credit use if the integration cannot enforce or verify that setting.
  [Free OpenRouter variants](https://openrouter.ai/docs/guides/routing/model-variants/free) can be
  rate-limited or disappear; use an explicit free variant with no paid routing/fallback and reject
  unknown pricing, not a generic auto model. Show provider and billing class before Send (approval
  does not control inference spend). Paid use requires separate explicit opt-in, a pinned model,
  bounded calls/outputs and provider/gateway spending limits where available. Usage estimates or
  budget alerts are not guaranteed hard caps. Never fall back to a paid provider on outage or quota.

## 5. Implementation sequence

Each row is a separate PR; keep incomplete paths behind a disabled feature until their tests pass.

The controller lives behind `todo-client`'s opt-in `assistant` feature. A host captures the
confirmed replica with `SyncClient::assistant_snapshot`, feeds sync changes to `Controller`, and
runs the future returned by a deliberate `start`. Model adapters supply normalized round events;
`RoundDecoder` can frame SSE or buffered JSON without owning a transport. A framed successful
`finish_reason` can complete a round on clean EOF without `[DONE]`; EOF without positive completion
cannot freeze a draft. Proposal cards use simulated core effects, not streamed prose. The explicit
`approve` action takes an owner sync client and the displayed proposal ID and digest. It returns a
handle for saving/pending/confirmed/rejected status and whole-group Undo; no provider code holds
that sync client. V1 does not advertise destructive tools.

| PR | Scope and release tests |
|---|---|
| 1. Tool contract | Extract typed descriptors/mappers and scope-aware local read projection. Keep MCP behaviour identical. Snapshot discovery/input schema and error cases, compare command conversion and read/query semantics with REST, test hidden IDs, derived counts and excluded references. Check wasm32 and aarch64 Android without native-only dependencies. |
| 2. Controller and proposals | Fake provider, normalized streaming, bounded loop, in-memory diffs and UI-facing state. Fragmented UTF-8/SSE/arguments, missing terminal event, malformed tools, disallowed IDs/fields, scope reset, cancellation and duplicate/delayed messages. Use hostile email/title fixtures and assert zero writes before approval. Property-test batch replay and full preview effects. |
| 3. Approval, sync and Activity | Owner-only REST approval, reserved identity, bounded atomic engine batch, outbox operation, receipt and grouped undo. Integration tests: wrong principal and forged actor/forms, CSRF/origin, invalid last command, mid-transaction failure, two concurrent reviewers, intervening event, epoch restore, lost response/idempotent replay, group replay/undo conflict, crash/IndexedDB durability and SSE convergence across replicas. |
| 4. Windows local Mac | Implement the first compatible adapter, connection/settings flow and GPUI UI. Fixtures for discovery/API variants, broken streams, sleep, usage limits, manual recovery and zero unactioned network calls; real HTTPS tailnet smoke with a selected local model. Desktop gallery/screenshot gate, never interactive app launch for automated visual checks. |
| 5. Android local Mac | Expose controller through UniFFI, keep credentials under Keystore and wire Compose. Test lifecycle, permission denial, no remote HTTP, actual tailnet VPN routing on supported Android versions, stale/local-only mode and restart recovery of an approved batch. Unit tests and Paparazzi gallery. |
| 6. Web local Mac | Exact runtime CSP origin, streaming Fetch adapter, Leptos UI. Playwright with HTTPS cross-origin fixtures: correct preflight and stream, bad origin/CSP/missing TLS, no todo credentials on provider requests, abort, sign-out and tab handover. Test actual browser permissions/routing on the tailnet; mark unsupported combinations without relaxing security. |
| 7. Optional API-key cloud | Owner-run gateway plus one documented cloud backend, then additional adapters in separate PRs. Fake billing error, unknown pricing, opt-in, credential isolation, route constraints, budgets and no auto-fallback. Keep CI offline with fixtures; no paid tests by default. |
| 8. Optional official SIWC | Windows spike first; Android only after platform validation. Recheck terms/docs. Test canceled consent, wrong state/nonce/account/scope, rotating-token races, revocation, expired/limited plan, `response.failed` midstream, supported tool wrapping and required request fields. No `backend-api`, token imports, todo-server token storage or automatic credit use. |
