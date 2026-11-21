<img src="assets/icon.svg" width="64" height="64" alt="">

# to-do-or-not-to-do

A personal to-do app built as a shared interface for one person and their AI agents.

- **Clients:** a fast native desktop client for Windows (Rust + [GPUI](https://github.com/zed-industries/zed)),
  then Android (Jetpack Compose) and the web (WebAssembly). All of them share one Rust core.
- **Agents:** a single self-hosted server exposes REST and MCP. Agents capture tasks, read lists and help plan.
- **Agent permissions:** each agent gets its own token with permissions per list.
- **Activity and undo:** every change is attributed to whoever made it and can be undone from an activity feed.
- **Microsoft To Do import:** one-time import of open and completed tasks, steps, notes and attachments.

> **Status:** early development. The `spikes/` directory holds throwaway experiments that informed the design.
> Nothing here is ready to use yet.

## Repository layout

| Path | Contents |
|---|---|
| `crates/todo-core` | Domain model shared by every component. No I/O; builds for Windows, Android and wasm32 |
| `crates/todo-client` | Client sync engine shared by the desktop, Android and web clients |
| `crates/todo-server` | Self-hosted server: REST, live event stream, MCP |
| `crates/todo-import` | Microsoft To Do importer |
| `crates/todo-design` | Design tokens shared by every client (colours, type, spacing, motion, icons), with CSS and JSON exports. No I/O |
| `crates/todo-backup` | Encrypted SQLite backups to S3-compatible storage (R2), with restore, a restore drill and a monthly operations budget |
| `assets/fonts`, `assets/icons` | Bundled OFL fonts and the Lucide icon subset, with their licenses |
| `docs/design-language.md` | The design language: concept, colour, type, grid, motion, icons and the screenshot gate |
| `docs/agents` | Connecting agents over MCP (Claude Code, Codex, any streamable-HTTP client) and how they should behave |
| `spikes/gpui-desktop` | GPUI viability spike on Windows: list, text input, tray, global hotkey, software rendering, drag-and-drop formats |
| `spikes/graph-export` | Microsoft To Do export through Microsoft Graph, with a check against the local cache |

Deployment-specific configuration (hostnames, secrets, allowlists) is never part of this repository. The server
reads it from a local config file, and clients learn the server address when they pair.

## Contributing hooks

The repo ships git hooks that block accidental leaks of secrets and private configuration:

```sh
git config core.hooksPath .githooks
```

They use [gitleaks](https://github.com/gitleaks/gitleaks) if it's installed, plus an optional local denylist.

## License

[PolyForm Noncommercial 1.0.0](LICENSE.md): free for personal, research, hobby and other noncommercial use.
Commercial use requires a separate license from the author. Vendored third-party code keeps its own license,
for example `spikes/gpui-desktop/vendor/gpui_windows` (Apache-2.0).
