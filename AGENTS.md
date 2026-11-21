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
| `crates/todo-server` | axum server: REST, SSE, MCP, auth, ingest. |
| `crates/todo-import` | Microsoft To Do importer CLI. |
| `apps/desktop` | GPUI desktop app (not yet created). |
| `spikes/` | Throwaway experiments. Not in the workspace and not maintained. Don't build on them. |

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# The shared crates must keep building for these targets (CI enforces it):
cargo check -p todo-core -p todo-client --target wasm32-unknown-unknown
cargo check -p todo-core -p todo-client --target aarch64-linux-android
```

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
- **Shared crates stay portable.** Before adding a dependency to `todo-core` or `todo-client`, check that it
  builds for wasm32 and Android.
- **Process.** Work on branches, open PRs against `main`, and let CI pass before merging.

## Merging pull requests

Never merge through the GitHub web UI, and never merge with `gh pr merge` without setting the author. GitHub
would otherwise use the account's primary email address as the merge-commit author. Always use:

```sh
gh pr merge <n> --squash --author-email 2668168+jan-bures@users.noreply.github.com
```

Then run `git fetch` and check `git log --format='%ae %ce' -1 origin/main` before pushing anything else.
