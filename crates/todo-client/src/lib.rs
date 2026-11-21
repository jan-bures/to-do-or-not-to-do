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
//! | [`outbox`] | outbox and "couldn't sync" entries |
//! | [`pairing`] | device pairing: start, poll |
//! | [`protocol`] | the server API: paths, bodies, response classification |
//! | [`storage`] | the [`Storage`] trait and [`MemoryStorage`] |
//! | [`platform`] | the transport, clock and entropy traits |
//! | [`sse`] | an incremental Server-Sent Events parser |
//! | [`notify`] | runtime-agnostic change signals, timeouts and backoff |
//!
//! # Features
//!
//! - `native`: tokio + reqwest (rustls with *ring*) implementations of the platform traits, for the
//!   desktop app and tests.
//! - `sqlite`: a bundled-SQLite [`Storage`] (implies `native`).

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod client;
pub mod notify;
pub mod outbox;
pub mod pairing;
pub mod platform;
pub mod protocol;
pub mod replica;
pub mod sse;
pub mod storage;

#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use client::{ClientConfig, Connection, ErrorKind, Status, SyncClient, SyncError};
pub use notify::Backoff;
pub use outbox::{CommandId, Epoch, FailedEntry, Operation, OutboxEntry, Rejection};
pub use platform::{Clock, Entropy, EventStream, Transport};
pub use replica::{Replica, SubmitError};
pub use storage::{MemoryStorage, Storage, StorageError};
pub use todo_core::PROTOCOL_VERSION;

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_core_protocol_version() {
        assert_eq!(super::PROTOCOL_VERSION, todo_core::PROTOCOL_VERSION);
    }
}
