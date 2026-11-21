//! Domain model for the to-do app: entities, commands, events and the smart-list query language.
//!
//! This crate is pure Rust with no I/O and no platform-specific dependencies, because it is shared by
//! the server, the desktop app, the Android app (via UniFFI) and the web client (wasm32). CI checks
//! that it builds for all of those targets.

/// Version of the wire format for commands and events. Bump on breaking changes.
pub const PROTOCOL_VERSION: u32 = 1;

const _: () = assert!(PROTOCOL_VERSION >= 1);
