//! UniFFI bindings over `todo-client` for the Android app.
//!
//! The app is Kotlin and Compose; everything else is this shared Rust core, as on the desktop:
//!
//! | Module | Contents |
//! |---|---|
//! | [`pairing`] | check a server address, start and poll a pairing, the approval link |
//! | [`engine`] | [`TodoEngine`]: the sync client over the SQLite cache, its foreground sync loop, commands |
//! | [`view`] | the view model: navigation, rooms, rows, the detail pane |
//! | [`labels`] | dates and due labels, in the owner's UTC offset |
//! | [`lists`] | list swatches, glyphs and agent marks |
//! | [`landscape`] | the header landscape geometry, from `todo_design` |
//! | [`fixtures`] | invented sample data for the screenshot gallery |
//!
//! Kotlin bindings are generated from the built library by the `uniffi-bindgen` binary (feature
//! `bindgen`); the Gradle build does that. Nothing here logs task content or tokens.

#![cfg_attr(test, allow(clippy::unwrap_used))]
// UniFFI's generated scaffolding exports `extern "C"` functions.
#![allow(unsafe_code)]

uniffi::setup_scaffolding!();

pub mod device_pairing;
pub mod engine;
mod error;
pub mod fixtures;
pub mod labels;
pub mod landscape;
pub mod lists;
pub mod pairing;
pub mod recovery;
mod runtime;
mod transport;
pub mod view;

#[cfg(target_os = "android")]
mod android;

pub use engine::{ChangeListener, EngineConfig, TodoEngine};
pub use error::FfiError;
pub use pairing::normalize_server_address;
pub use recovery::CacheRecovery;

/// The wire protocol version this core speaks (`todo_core::PROTOCOL_VERSION`).
#[uniffi::export]
pub fn protocol_version() -> u32 {
    todo_core::PROTOCOL_VERSION
}
