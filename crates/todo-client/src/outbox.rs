//! What the client still has to send, and what the server refused.

use serde::{Deserialize, Serialize};
use todo_core::command::Command;
use todo_core::ids::EventId;
use todo_core::time::Timestamp;
use todo_tools::assistant::approval::ApprovedBatch;
use uuid::Uuid;

/// The server's log epoch: a random ID that changes when the log is replaced (restored from a
/// backup). Sequence numbers belong to one epoch.
pub type Epoch = Uuid;

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
    /// `POST /api/v1/assistant/approvals`. Never projected optimistically.
    AssistantApproval { batch: ApprovedBatch },
    /// `POST /api/v1/events/{seq}/undo`.
    Undo {
        /// Sequence number of any event of the command to undo.
        seq: u64,
        /// Undo even if later events changed the same fields.
        force: bool,
        /// The ID of the event at `seq` the user meant. If the server has another event there (its
        /// log was restored and grew again), it refuses the undo with `event_mismatch`.
        expected_event_id: EventId,
        /// The log epoch the event was seen in; the server refuses the undo with `epoch_mismatch` if
        /// its log has another one.
        #[serde(default)]
        epoch: Option<Epoch>,
    },
    /// A local undo waiting for an earlier command's receipt. It is resolved to `Undo` before send.
    UndoAfter {
        /// The command as identified by its idempotency key, not by its predicted events.
        original: CommandId,
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
    /// The events the server reported for it, once acknowledged: it is not sent again. The entry leaves
    /// the outbox when an event carrying its command ID reaches the confirmed state.
    #[serde(default)]
    pub acked: Option<Vec<EventId>>,
    /// The log epoch the acknowledgement's events belong to. An acknowledgement only counts while the
    /// replica follows that log: when it resyncs onto another one, the entry is sent again.
    #[serde(default)]
    pub acked_epoch: Option<Epoch>,
}

impl OutboxEntry {
    /// The entry with its acknowledgement forgotten, so it is sent again.
    #[must_use]
    pub fn unacknowledged(&self) -> Self {
        Self {
            acked: None,
            acked_epoch: None,
            ..self.clone()
        }
    }
}

impl OutboxEntry {
    /// The ordinary command, if any.
    #[must_use]
    pub fn command(&self) -> Option<&Command> {
        match &self.op {
            Operation::Command { command } => Some(command),
            Operation::AssistantApproval { .. }
            | Operation::Undo { .. }
            | Operation::UndoAfter { .. } => None,
        }
    }

    /// A local or checked undo, not an ordinary command or assistant approval.
    #[must_use]
    pub fn is_undo(&self) -> bool {
        matches!(
            self.op,
            Operation::Undo { .. } | Operation::UndoAfter { .. }
        )
    }

    /// The event ID for a checked undo, once its target is known.
    #[must_use]
    pub fn undo_target(&self) -> Option<EventId> {
        match self.op {
            Operation::Undo {
                expected_event_id, ..
            } => Some(expected_event_id),
            _ => None,
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
