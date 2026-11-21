//! The to-do server: an axum app over a SQLite event log.
//!
//! | Module | Contents |
//! |---|---|
//! | [`config`] | TOML config file plus `TODO_SERVER_*` environment overrides |
//! | [`db`] | SQLite schema, migrations, the append-only event log and projection tables |
//! | [`cancel`] | per-request cancellation with an atomic cancel-or-commit hand-off |
//! | [`engine`] | the single writer: `decide`, store and apply atomically; undo; live broadcast |
//! | [`auth`] | the [`Authenticated`](auth::Authenticated) extractor and the owner-token authenticator |
//! | [`api`] | REST handlers, the SSE stream, typed errors, the OpenAPI document |
//! | [`app`] | router, middleware (body limit, timeouts, tracing) and the server lifecycle |
//! | [`conn`] | the accept loop: connection cap, header-read timeout, write-progress watchdog, drain |
//! | [`summary`] | one-line summaries for responses and the activity feed |
//! | [`ratelimit`] | the per-principal event-stream connect limiter |

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod api;
pub mod app;
pub mod auth;
pub mod cancel;
pub mod config;
pub mod conn;
pub mod db;
pub mod engine;
pub mod ratelimit;
pub mod summary;

pub use app::{Options, RunningServer, Server, ServerError};
pub use config::Config;
pub use engine::Engine;
