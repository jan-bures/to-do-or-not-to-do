//! Microsoft To Do importer.
//!
//! The import runs in four steps, each a subcommand of the `todo-import` binary:
//!
//! 1. [`bundle`]: read a Microsoft Graph export ([`graph`]) and a *copy* of the local To Do SQLite
//!    cache ([`cache`]), map everything to `todo-core` entities and write a self-contained, versioned
//!    import [`Bundle`](bundle::Bundle).
//! 2. [`verify`]: compare the bundle with the raw Graph export, list by list and field by field.
//! 3. [`apply`]: send the bundle's commands to a server over REST. Every command has a deterministic
//!    `command_id`, so a re-run or a resume after a crash never stores anything twice.
//! 4. [`verify_server`]: read everything back over REST and check that it landed.
//!
//! | Module | Contents |
//! |---|---|
//! | [`graph`] | serde types for the raw Graph export and its loader |
//! | [`cache`] | read-only access to a copy of the To Do cache: groups, order, themes, email metadata |
//! | [`mapping`] | field conversions: times, due dates, notes, priority, recurrence, colours |
//! | [`ids`] | deterministic entity IDs and command IDs |
//! | [`bundle`] | the bundle format and the builder |
//! | [`verify`] | bundle versus export |
//! | [`http`] | the blocking REST client with retries |
//! | [`apply`] | sending the bundle, with a resume journal |
//! | [`verify_server`] | server versus bundle |
//!
//! Task content (titles, notes, email metadata) is personal. Nothing here logs it above `debug`; reports
//! name lists and tasks by a short hash of their Graph ID unless asked to show names.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod apply;
pub mod bundle;
pub mod cache;
pub mod graph;
pub mod http;
pub mod ids;
pub mod mapping;
pub mod verify;
pub mod verify_server;

/// A short, stable, non-reversible label for a source ID, for reports and logs.
#[must_use]
pub fn label(source_id: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(source_id.as_bytes())[..6])
}
