//! Domain model for the to-do app: entities, commands, events and the smart-list query language.
//!
//! This crate is pure Rust with no I/O and no platform-specific dependencies, because it is shared by
//! the server, the desktop app, the Android app (via UniFFI) and the web client (wasm32). CI checks
//! that it builds for all of those targets.
//!
//! # Architecture
//!
//! The model is event sourced:
//!
//! 1. A client builds a [`Command`] (with IDs it generated itself). An entity ID is never reused, even
//!    after the entity's creation was undone.
//! 2. [`decide()`] checks permissions and validity against a [`State`] and returns [`Event`]s. Each event
//!    records the changed fields **with their previous values**.
//! 3. [`apply()`] folds events into the state. It is deterministic, so replaying the log rebuilds the same
//!    state on the server and on every client.
//! 4. [`undo()`] builds compensating events from the values the undone event replaced, after checking
//!    that the supplied event is exactly the applied one and that the actor may make the compensating
//!    change. It refuses (with a typed conflict) when later events, compensations included, changed the
//!    same fields, and (with a typed invariant error) when the result would be an invalid entity. If the
//!    entity's old position key was taken meanwhile, a rekey event in the same group follows.
//!
//! Nothing here reads a clock or a random number generator: callers pass `now` and, for new IDs, a
//! timestamp and random bytes ([`ids`]).
//!
//! | Module | Contents |
//! |---|---|
//! | [`ids`], [`actor`] | typed UUIDv7 identifiers; who did something |
//! | [`model`] | collections, lists, tasks, steps, tags, links, attachments, reminders |
//! | [`ordering`], [`recurrence`], [`time`] | fractional position keys; RRULE subset; date helpers |
//! | [`command`], [`event`], [`state`] | requests, the event log record, the in-memory state |
//! | [`fingerprint`] | SHA-256 content fingerprints of events |
//! | [`mod@decide`], [`mod@apply`], [`mod@undo`] | validation, folding, compensation |
//! | [`access`] | agent access levels and permission checks |
//! | [`query`] | the smart-list query language |

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod access;
pub mod actor;
pub mod apply;
pub mod command;
pub mod decide;
pub mod error;
pub mod event;
pub mod fingerprint;
pub mod ids;
pub mod model;
pub mod ordering;
pub mod query;
pub mod recurrence;
pub mod state;
pub mod time;
pub mod undo;

pub use access::{AccessLevel, Grants};
pub use actor::{Actor, AgentId, DeviceId};
pub use apply::apply;
pub use command::Command;
pub use decide::decide;
pub use error::{ApplyError, DomainError, UndoError};
pub use event::Event;
pub use state::State;
pub use undo::{UndoOptions, undo, undo_with};

/// Hard upload ceiling on the wire. Deployments may configure a smaller limit.
pub const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// Version of the wire format for commands and events. Bump on breaking changes.
pub const PROTOCOL_VERSION: u32 = 1;

const _: () = assert!(PROTOCOL_VERSION >= 1);
