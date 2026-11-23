//! Client-side sync engine shared by every client: local replica, durable outbox, event cursor and
//! server subscription.
//!
//! Anything platform-specific sits behind a trait so this crate builds for Windows, Android and wasm32
//! with its default features:
//!
//! | Trait | Purpose | Provided |
//! |---|---|---|
//! | [`Transport`] | HTTP request/response plus an SSE [`EventStream`], with the bearer token | `native`: `native::HttpTransport` (reqwest, rustls) |
//! | [`Storage`] | atomic, durable persistence of the replica and the outbox | [`MemoryStorage`]; `sqlite`: `sqlite::SqliteStorage` |
//! | [`Clock`] | current time and sleeping | `native`: `native::TokioClock` |
//! | [`Entropy`] | random bytes for command IDs and jitter | `native`: `native::OsEntropy` |
//!
//! | Module | Contents |
//! |---|---|
//! | [`client`] | [`SyncClient`]: submit, undo, view, status; the send and event loops |
//! | [`replica`] | [`Replica`]: confirmed state, cursor, log epoch, outbox and the optimistic view (no I/O) |
//! | [`activity`] | paged activity reads and shared feed wording |
//! | [`room_header`] | a room's tasks and header: eyebrow, sun progress, summary, status-bar counts, empty line |
//! | [`intent`] | shared quick-add syntax and gesture-to-command builders |
//! | [`intent::suggest`] | local, incremental title-to-list suggestions; no storage or network access |
//! | [`outbox`] | outbox and "couldn't sync" entries |
//! | [`failure`] | plain words for a "couldn't sync" entry: what it was, its name, why it was refused |
//! | [`pairing`] | device pairing: start, poll |
//! | [`protocol`] | the server API: paths, bodies, response classification |
//! | [`storage`] | the [`Storage`] trait and [`MemoryStorage`] |
//! | [`mirror`] | [`mirror::MirrorStorage`]: in memory, with a journal for an asynchronous backend (IndexedDB) |
//! | [`platform`] | the transport, clock and entropy traits |
//! | [`session`] | a browser session's CSRF token, and retrying with a fresh one |
//! | [`sse`] | an incremental Server-Sent Events parser |
//! | [`notify`] | runtime-agnostic change signals, timeouts and backoff |
//!
//! # Features
//!
//! - `native`: tokio + reqwest (rustls with *ring*) implementations of the platform traits, for the
//!   desktop app and tests.
//! - `sqlite`: a bundled-SQLite [`Storage`] (implies `native`).
//! - `web` (wasm32 only): `fetch`, IndexedDB (through [`mirror::MirrorStorage`]), `setTimeout` and
//!   Web Crypto implementations for the web app (`web` module).

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod activity;
pub mod client;
pub mod device_pairing;
pub mod failure;
pub mod intent;
pub mod mirror;
pub mod notify;
pub mod outbox;
pub mod pairing;
pub mod platform;
pub mod protocol;
pub mod replica;
pub mod revocation;
pub mod room_header;
pub mod session;
pub mod sse;
pub mod storage;
pub mod sync_label;
pub mod trash;

#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub mod web;

pub use client::{ClientConfig, Connection, ErrorKind, Readiness, Status, SyncClient, SyncError};
pub use notify::Backoff;
pub use outbox::{CommandId, Epoch, FailedEntry, Operation, OutboxEntry, Rejection};
pub use platform::{Clock, Entropy, EventStream, Transport};
pub use replica::{Replica, SubmitError, TrackedCommand, TrackedUndo};
pub use revocation::Revocation;
pub use storage::{MemoryStorage, Storage, StorageError};
pub use todo_core::PROTOCOL_VERSION;

/// The same-origin entry point to the server's admin pages.
pub const ADMIN_PATH: &str = "/admin";

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_core_protocol_version() {
        assert_eq!(super::PROTOCOL_VERSION, todo_core::PROTOCOL_VERSION);
    }
}
