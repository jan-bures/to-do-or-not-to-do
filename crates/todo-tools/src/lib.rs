//! Runtime-free tool contracts shared by MCP and the in-app assistant.
//!
//! The caller supplies clocks, IDs, scope and trusted state. Nothing here executes commands, grants
//! access, talks to a provider or knows about REST/MCP request and response envelopes.

pub mod args;
pub mod assistant;
pub mod command;
pub mod descriptor;
pub mod read;
