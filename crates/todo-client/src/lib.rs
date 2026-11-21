//! Client-side sync engine: local cache, command queue, event cursor and server subscription.
//!
//! Shared by every client. Anything platform-specific (storage, HTTP transport, notifications) must sit
//! behind a trait so this crate keeps building for Windows, Android and wasm32.

pub use todo_core::PROTOCOL_VERSION;

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_core_protocol_version() {
        assert_eq!(super::PROTOCOL_VERSION, todo_core::PROTOCOL_VERSION);
    }
}
