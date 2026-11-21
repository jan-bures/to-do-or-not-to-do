//! Deterministic identifiers.
//!
//! Every entity and every command of a bundle gets an ID derived from a SHA-256 of a fixed namespace,
//! the kind of source object, its source ID (a Graph ID, a cache ID, or a tag name) and a field name.
//! Building the bundle twice from the same export therefore yields the same IDs, and so does a rebuild
//! from a newer export for every object that was already there. On the server a retried command with
//! the same `command_id` replays its stored result, and creating an entity whose ID exists fails, so an
//! import can never store anything twice.
//!
//! Entity IDs are UUIDv7-shaped with the entity's own creation time, so they sort roughly by age like
//! the IDs clients generate. Command IDs are UUIDv8 (custom) built from the hash alone.

use jiff::Timestamp;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Namespace of every hash. Bump to start a fresh, independent import.
pub const NAMESPACE: &str = "todo-import/v1";

fn digest(parts: &[&str]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(NAMESPACE.as_bytes());
    for p in parts {
        h.update([0u8]);
        h.update(p.as_bytes());
    }
    h.finalize().into()
}

/// The `command_id` for the command that writes `field` of the source object `(kind, source_id)`.
#[must_use]
pub fn command_id(kind: &str, source_id: &str, field: &str) -> Uuid {
    let h = digest(&["command", kind, source_id, field]);
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 0x0f) | 0x80; // version 8
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    Uuid::from_bytes(b)
}

/// The UUID of the entity made from the source object `(kind, source_id)`, created at `at`.
#[must_use]
pub fn entity_uuid(kind: &str, source_id: &str, at: Timestamp) -> Uuid {
    let h = digest(&["entity", kind, source_id]);
    let mut rand = [0u8; 10];
    rand.copy_from_slice(&h[..10]);
    let ms = u64::try_from(at.as_millisecond()).unwrap_or(0);
    todo_core::ids::uuid_v7(ms, rand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_distinct() {
        let t = Timestamp::from_second(1_700_000_000).unwrap();
        assert_eq!(
            command_id("task", "A", "entity"),
            command_id("task", "A", "entity")
        );
        assert_ne!(
            command_id("task", "A", "entity"),
            command_id("task", "B", "entity")
        );
        assert_ne!(
            command_id("task", "A", "entity"),
            command_id("step", "A", "entity")
        );
        assert_ne!(
            command_id("task", "A", "entity"),
            command_id("task", "A", "other")
        );
        assert_eq!(command_id("task", "A", "entity").get_version_num(), 8);
        let e = entity_uuid("task", "A", t);
        assert_eq!(e, entity_uuid("task", "A", t));
        assert_eq!(e.get_version_num(), 7);
        assert_ne!(e, entity_uuid("task", "B", t));
    }
}
