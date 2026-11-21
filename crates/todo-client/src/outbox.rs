//! What the client still has to send, and what the server refused.

use serde::{Deserialize, Serialize};
use todo_core::command::Command;
use todo_core::ids::EventId;
use todo_core::time::Timestamp;
use uuid::Uuid;

/// The client-generated idempotency key of a command or undo (a UUIDv7).
pub type CommandId = Uuid;

/// What an outbox entry asks the server to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// `POST /api/v1/commands`.
    Command {
        /// The command.
        command: Command,
    },
    /// `POST /api/v1/events/{seq}/undo`.
    Undo {
        /// Sequence number of any event of the command to undo.
        seq: u64,
        /// Undo even if later events changed the same fields.
        force: bool,
    },
}

/// A command or undo waiting to be confirmed by the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Idempotency key, reused on every retry.
    pub command_id: CommandId,
    /// What to do.
    pub op: Operation,
    /// When the app submitted it. Also the `now` the optimistic view decides the command with, so the
    /// view is stable across recomputations.
    pub submitted_at: Timestamp,
    /// How often it was sent. Non-zero means the server may already have applied it.
    #[serde(default)]
    pub attempts: u32,
    /// The events the server reported for it, once acknowledged. The entry leaves the outbox when the
    /// first of them reaches the confirmed state.
    #[serde(default)]
    pub acked: Option<Vec<EventId>>,
}

impl OutboxEntry {
    /// The command, unless this is an undo.
    #[must_use]
    pub fn command(&self) -> Option<&Command> {
        match &self.op {
            Operation::Command { command } => Some(command),
            Operation::Undo { .. } => None,
        }
    }
}

/// Why the server definitively refused an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// HTTP status.
    pub status: u16,
    /// The server's error code, for example `deleted` or `undo_conflict`.
    pub code: String,
    /// The server's message. May quote task content: show it, never log it above `debug`.
    pub message: String,
}

/// An entry the server refused ("couldn't sync"). Kept until the app dismisses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedEntry {
    /// The entry as it was in the outbox.
    pub entry: OutboxEntry,
    /// When it was refused.
    pub failed_at: Timestamp,
    /// Why.
    pub rejection: Rejection,
}
