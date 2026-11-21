# Agent instructions

A personal to-do app: one owner plus their AI agents. It has a Rust server (REST, SSE, MCP) and clients
sharing a Rust core: a GPUI desktop client first, then Android (Compose) and web (wasm32).

## This repository is public

Nothing deployment-specific or personal may ever be committed here. That includes:

- hostnames or domains;
- email addresses;
- account or tenant IDs, tunnel IDs and tokens;
- local paths that contain user names;
- device names and personal context.

- **Placeholders.** Use `todo.example.com`, `you@example.com` and `<account-id>`.
- **Config.** Runtime config is read from a file outside the repo, or from environment variables. Clients learn
  the server address when they pair. **Never compile a hostname into a build.** Public CI artifacts can be
  downloaded by anyone.
- **Private material.** Plans, the project brief, deployment config and notes live in the private companion
  repo, checked out locally at `private/`, which git ignores. Read `private/docs/` when it exists, especially
  `project-brief.md` and the current milestone plan. Never copy text from it into this repo without
  scrubbing it first.
- **Hooks.** Keep them on: `git config core.hooksPath .githooks`. They run gitleaks and the private denylist.
  Never bypass them (`--no-verify`). If a hook blocks you, remove the leak.
- **Commit identity.** Commits use the owner's GitHub no-reply address, set in this repo's local git config.

## Layout

| Path | Purpose |
|---|---|
| `crates/todo-core` | Domain model, commands and events, validation, the smart-list query language. **No I/O and no platform dependencies.** |
| `crates/todo-client` | Sync engine shared by every client. Put platform specifics behind traits. |
| `crates/todo-ffi` | UniFFI bindings over `todo-client` for the Android app: engine, view model, pairing, fixtures. |
| `crates/todo-server` | axum server: REST, SSE, MCP, auth, ingest. |
| `crates/todo-import` | Microsoft To Do importer CLI. |
| `crates/todo-fs` | Owner-only files and directories (POSIX modes, Windows DACLs), shared by the server and the backup CLI. |
| `crates/todo-backup` | Backup CLI: snapshot, zstd and age, upload to R2 under a monthly operations budget, restore and drill. |
| `crates/todo-design` | Design tokens shared by every client: colours (OKLCH, derived from a hue), type, spacing, radii, motion, landscape geometry, icons. **No I/O.** Generates `generated/tokens.css` and `tokens.json`. |
| `assets/fonts` | Bundled OFL fonts (static instances, Czech coverage), their licenses, sources and build script. |
| `assets/brand` | The app icon (Daybreak): SVG sources, generated PNG/ICO rasters and the flattened Android monochrome layer. Regenerate with `node generate.mjs` (see its README). |
| `assets/icons` | Lucide icon subset (ISC) and the hand-drawn list glyphs. |
| `docs/design-language.md` | The design language. Every UI change follows it and passes the screenshot gate. |
| `apps/desktop` | GPUI desktop app, Windows only for now. Its own Cargo workspace with a vendored, patched `gpui_windows`. See `docs/desktop.md`. |
| `apps/android` | Android app (Kotlin, Compose, Gradle). The Rust core comes through `todo-ffi` and cargo-ndk. See `docs/android.md`. |
| `apps/web` | Web app: Leptos (CSR) on wasm32, served by `todo-server` at `/app`, signed in with the passkey session. Its own Cargo workspace; browser tests and the gallery in `apps/web/e2e`. See `docs/web.md`. |
| `workers/email-ingest` | Cloudflare Email Worker for email to task (plain JS, Node tests). Contract: `docs/ingest-email.md`. |
| `spikes/` | Throwaway experiments. Not in the workspace and not maintained. Don't build on them. |

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# The shared crates must keep building for these targets (CI enforces it):
cargo check -p todo-core -p todo-client -p todo-design --target wasm32-unknown-unknown
cargo check -p todo-core -p todo-client -p todo-design --target aarch64-linux-android
# After changing a design token, regenerate the checked-in exports:
TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated
# The email Worker (Node 22 or later):
cd workers/email-ingest && npm ci && npm test
# The desktop app (its own workspace, Windows only):
cd apps/desktop && cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
pwsh apps/desktop/scripts/gallery.ps1   # screenshot gallery; look at it after UI changes
# The Android app (JDK 21, Android SDK, cargo-ndk; see docs/android.md):
cd apps/android && ./gradlew :app:assembleDebug :app:testDebugUnitTest :app:verifyPaparazziDebug
# The web app (its own workspace; wasm-bindgen-cli at the version in apps/web/Cargo.lock):
cd apps/web && cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
cargo clippy --manifest-path apps/web/Cargo.toml --target wasm32-unknown-unknown -- -D warnings
node apps/web/scripts/build.mjs [--fixtures]   # -> apps/web/dist (or dist-fixtures for the gallery)
cd apps/web/e2e && npm ci && npx playwright test   # browser tests + gallery in apps/web/e2e/gallery/
npx playwright test -c live.config.mjs             # against a real todo-server (cargo build -p todo-server first)
```

For desktop visual checks, agents must use the gallery or screenshot mode. Never launch the interactive app.

The toolchain is pinned in `rust-toolchain.toml` to what the GPUI revision needs. Bump it only together
with GPUI.

## Conventions

- **Errors.** Libraries use `thiserror` with typed errors. Binaries use `anyhow` at the edges. No `unwrap()`
  outside tests (Clippy warns).
- **Logging.** Use `tracing`. Never log task content, email metadata or tokens above `debug`.
- **Workspace dependencies.** Declare versions once in the root `Cargo.toml` under
  `[workspace.dependencies]`.
- **Tests.** Every behaviour change ships with tests. Event and undo logic gets property tests: replaying
  events rebuilds the same state, and undoing every event restores the previous state.
- **Shared crates stay portable.** Before adding a dependency to `todo-core`, `todo-client` or `todo-design`,
  check that it builds for wasm32 and Android.
- **UI values come from `todo-design`.** Clients never hard-code colours, sizes or durations. Follow
  `docs/design-language.md`.
- **Process.** Work on branches, open PRs against `main`, and let CI pass before merging.

## Merging pull requests

**Never merge through the GitHub web UI or with `gh pr merge`.** GitHub writes the merge or squash commit with
the account's primary email address as its author. Passing `--author-email` with the no-reply address doesn't
work either: GitHub rejects it with "Invalid email address".

**Merge by fast-forwarding `main` to the PR's head instead.** Every commit on the branch is then exactly as it
was authored locally, with the no-reply identity, and GitHub marks the PR as merged.

1. **Rebase.** Rebase the PR branch onto the latest `origin/main`. Squash it locally first if you want one
   commit. Push the branch, and wait for CI to pass on that head.
2. **Check the identities.** Run `git log --format='%ae %ce' origin/main..origin/<branch>`. Every line must be
   the no-reply address.
3. **Fast-forward.** Run `git push origin origin/<branch>:main`. It's rejected unless it's a fast-forward, and
   that's intended.
4. **Clean up.** Run `git fetch`, confirm that `gh pr view <n>` shows `MERGED`, and delete the branch.

For stacked PRs, merge the base first. Then rebase the next branch onto `main`, retarget it with
`gh pr edit <n> --base main`, and repeat.
