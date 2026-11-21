//! Typed identifiers.
//!
//! Every entity has its own ID type wrapping a UUID. Clients generate IDs themselves (UUIDv7, so they
//! sort by creation time), which makes command retries safe. The crate never reads a clock or a random
//! number generator: callers pass the timestamp and the random bytes in, which keeps the core pure and
//! free of platform dependencies (no `getrandom`, so it builds for wasm32 without extra features).

use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Error returned when an identifier or name cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid identifier: {0}")]
pub struct ParseIdError(pub String);

/// Builds a UUIDv7 from a Unix timestamp in milliseconds and 10 bytes of caller-supplied randomness.
#[must_use]
pub fn uuid_v7(unix_ms: u64, rand: [u8; 10]) -> Uuid {
    let ms = unix_ms.to_be_bytes();
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms[2..]);
    b[6] = 0x70 | (rand[0] & 0x0f);
    b[7] = rand[1];
    b[8] = 0x80 | (rand[2] & 0x3f);
    b[9..].copy_from_slice(&rand[3..]);
    Uuid::from_bytes(b)
}

/// Stable 128-bit FNV-1a style hash used to derive deterministic IDs. Not cryptographic.
#[must_use]
pub fn stable_hash(data: &[u8]) -> [u8; 16] {
    fn fnv(data: &[u8], offset: u64) -> u64 {
        let mut h = offset;
        for &b in data {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
    let a = fnv(data, 0xcbf2_9ce4_8422_2325);
    let b = fnv(data, 0x8422_2325_cbf2_9ce4 ^ a.rotate_left(17));
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&a.to_be_bytes());
    out[8..].copy_from_slice(&b.to_be_bytes());
    out
}

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// Creates a UUIDv7 identifier from a Unix timestamp (ms) and 10 random bytes.
            #[must_use]
            pub fn new_v7(unix_ms: u64, rand: [u8; 10]) -> Self {
                Self(uuid_v7(unix_ms, rand))
            }

            /// Derives a deterministic UUIDv7-shaped identifier from a timestamp and a seed.
            ///
            /// The same `(at, seed)` always yields the same ID. `decide` uses this so that retrying a
            /// command produces events with identical IDs, which the server can de-duplicate.
            #[must_use]
            pub fn derive(at: Timestamp, seed: &[u8]) -> Self {
                let h = stable_hash(seed);
                let mut rand = [0u8; 10];
                rand.copy_from_slice(&h[..10]);
                let ms = u64::try_from(at.as_millisecond()).unwrap_or(0);
                Self::new_v7(ms, rand)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self).map_err(|e| ParseIdError(e.to_string()))
            }
        }
    };
}

typed_id!(
    /// Identifier of a [`Collection`](crate::model::Collection).
    CollectionId
);
typed_id!(
    /// Identifier of a [`List`](crate::model::List).
    ListId
);
typed_id!(
    /// Identifier of a [`Task`](crate::model::Task).
    TaskId
);
typed_id!(
    /// Identifier of a [`Step`](crate::model::Step).
    StepId
);
typed_id!(
    /// Identifier of a [`Tag`](crate::model::Tag).
    TagId
);
typed_id!(
    /// Identifier of a [`Link`](crate::model::Link).
    LinkId
);
typed_id!(
    /// Identifier of an [`AttachmentRef`](crate::model::AttachmentRef).
    AttachmentId
);
typed_id!(
    /// Identifier of a [`Reminder`](crate::model::Reminder).
    ReminderId
);
typed_id!(
    /// Identifier of an [`Event`](crate::event::Event).
    EventId
);

/// Identifier of any entity, tagged with its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum EntityId {
    /// A collection.
    Collection(CollectionId),
    /// A list.
    List(ListId),
    /// A task.
    Task(TaskId),
    /// A step.
    Step(StepId),
    /// A tag.
    Tag(TagId),
    /// A link.
    Link(LinkId),
    /// An attachment reference.
    Attachment(AttachmentId),
    /// A reminder.
    Reminder(ReminderId),
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collection(i) => write!(f, "collection {i}"),
            Self::List(i) => write!(f, "list {i}"),
            Self::Task(i) => write!(f, "task {i}"),
            Self::Step(i) => write!(f, "step {i}"),
            Self::Tag(i) => write!(f, "tag {i}"),
            Self::Link(i) => write!(f, "link {i}"),
            Self::Attachment(i) => write!(f, "attachment {i}"),
            Self::Reminder(i) => write!(f, "reminder {i}"),
        }
    }
}

macro_rules! entity_id_from {
    ($($ty:ident => $var:ident),* $(,)?) => {$(
        impl From<$ty> for EntityId {
            fn from(id: $ty) -> Self { Self::$var(id) }
        }
    )*};
}
entity_id_from!(
    CollectionId => Collection, ListId => List, TaskId => Task, StepId => Step,
    TagId => Tag, LinkId => Link, AttachmentId => Attachment, ReminderId => Reminder,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v7_layout_and_ordering() {
        let a = TaskId::new_v7(1_000, [1; 10]);
        let b = TaskId::new_v7(2_000, [0; 10]);
        assert!(a < b);
        assert_eq!(a.as_uuid().get_version_num(), 7);
        let parsed: TaskId = a.to_string().parse().unwrap();
        assert_eq!(parsed, a);
    }

    #[test]
    fn derive_is_deterministic() {
        let t = Timestamp::from_second(1_700_000_000).unwrap();
        assert_eq!(EventId::derive(t, b"x"), EventId::derive(t, b"x"));
        assert_ne!(EventId::derive(t, b"x"), EventId::derive(t, b"y"));
    }

    #[test]
    fn bad_id_is_rejected() {
        assert!("nope".parse::<TaskId>().is_err());
    }
}
