//! The to-do server: an axum app over a SQLite event log.
//!
//! | Module | Contents |
//! |---|---|
//! | [`admin`] | the local admin API: a loopback-only listener with its own token, for routine admin operations |
//! | [`config`] | TOML config file plus `TODO_SERVER_*` environment overrides |
//! | [`db`] | SQLite schema, migrations, the append-only event log and projection tables |
//! | [`cancel`] | per-request cancellation with an atomic cancel-or-commit hand-off |
//! | [`engine`] | the single writer: `decide`, store and apply atomically; undo; live broadcast |
//! | [`auth`] | passkeys, sessions, device pairing, agent tokens, share links, rate limits, audit log |
//! | [`web`] | server-rendered sign-in, setup, pairing and admin pages, share pages, static assets |
//! | [`api`] | REST handlers, the SSE stream, typed errors, the OpenAPI document |
//! | [`ingest`] | email to task: the HMAC-signed endpoint the Email Worker calls, its allowlist and list keys |
//! | [`blobs`] | attachment content: encryption, the bucket, the R2 budget, garbage collection |
//! | [`mcp`] | the MCP server at `/mcp`: agent tools over the REST handlers |
//! | [`app`] | router, middleware (body limit, timeouts, tracing) and the server lifecycle |
//! | [`conn`] | the accept loop: connection cap, header-read timeout, write-progress watchdog, drain |
//! | [`summary`] | one-line summaries for responses and the activity feed |
//! | [`ratelimit`] | the per-principal event-stream connect limiter |

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod admin;
pub mod api;
pub mod app;
pub mod auth;
pub mod blobs;
pub mod cancel;
pub mod config;
pub mod conn;
pub mod db;
pub mod engine;
pub mod ingest;
pub mod mcp;
pub mod ratelimit;
pub mod scope;
pub mod summary;
pub mod web;

pub use app::{Options, RunningServer, Server, ServerError};
pub use config::Config;
pub use engine::Engine;
