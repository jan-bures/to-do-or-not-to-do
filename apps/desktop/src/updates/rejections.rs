//! Persistent, expiring cooldowns for size/hash mismatches on authenticated offers.

use super::{atomic_write, release};
use release::Rejection;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use todo_core::time::Timestamp;

const MARKER: &str = ".todo-rejected-updates";
const MAX_BYTES: u64 = 16 * 1024;
const MAX_ENTRIES: usize = 32;
const COOLDOWNS: [i64; 3] = [6 * 3600, 18 * 3600, 24 * 3600];

#[derive(Deserialize, Serialize)]
struct Entry {
    offer: Rejection,
    last_mismatch: Timestamp,
    step: usize,
}

impl Entry {
    fn blocks(&self, now: Timestamp) -> bool {
        let elapsed = now.as_second() - self.last_mismatch.as_second();
        let delay = COOLDOWNS[self.step.min(COOLDOWNS.len() - 1)];
        // A backwards clock change must not extend the cooldown indefinitely.
        (0..delay).contains(&elapsed)
    }
}

#[derive(Default)]
pub(super) struct Rejections(Vec<Entry>);

impl Rejections {
    pub fn load(dir: &Path) -> Self {
        // Legacy entries carry no timestamp. Treat them as expired, not as permanent blocks.
        File::open(dir.join(MARKER))
            .ok()
            .and_then(|file| release::read_bounded(file, MAX_BYTES).ok())
            .filter(|bytes| bytes.len() as u64 <= MAX_BYTES)
            .and_then(|bytes| serde_json::from_slice::<Vec<Entry>>(&bytes).ok())
            .filter(|entries| entries.len() <= MAX_ENTRIES)
            .map(Self)
            .unwrap_or_default()
    }

    pub fn has_newer_blocked(&self, installed: &Version, now: Timestamp) -> bool {
        self.0
            .iter()
            .any(|entry| entry.offer.version > *installed && entry.blocks(now))
    }

    pub fn blocks(&self, offer: &Rejection, now: Timestamp) -> bool {
        self.0
            .iter()
            .any(|entry| entry.offer == *offer && entry.blocks(now))
    }

    pub fn record_mismatch(&mut self, dir: &Path, offer: &Rejection, now: Timestamp) {
        let mut step = 0;
        if let Some(index) = self.0.iter().position(|entry| entry.offer == *offer) {
            if self.0[index].blocks(now) {
                // A manual retry during a cooldown does not extend it.
                return;
            }
            let old = self.0.remove(index);
            if now >= old.last_mismatch {
                step = old.step.saturating_add(1).min(COOLDOWNS.len() - 1);
            }
        }
        self.0.insert(
            0,
            Entry {
                offer: offer.clone(),
                last_mismatch: now,
                step,
            },
        );
        self.0.truncate(MAX_ENTRIES);
        self.save(dir);
    }

    pub fn clear(&mut self, dir: &Path, offer: &Rejection) {
        let count = self.0.len();
        self.0.retain(|entry| entry.offer != *offer);
        if self.0.len() != count {
            self.save(dir);
        }
    }

    fn save(&self, dir: &Path) {
        let saved = serde_json::to_vec(&self.0).is_ok_and(|bytes| {
            bytes.len() as u64 <= MAX_BYTES
                && atomic_write(&dir.join(MARKER), |file| file.write_all(&bytes)).is_ok()
        });
        if !saved {
            tracing::warn!("could not save desktop update cooldowns");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: u8) -> Rejection {
        Rejection {
            version: Version::parse(
                "18446744073709551615.18446744073709551615.18446744073709551615-build.4294967295",
            )
            .expect("valid version"),
            fingerprint: [id; 32],
        }
    }

    #[test]
    fn file_is_bounded_by_entries_and_bytes() {
        let dir = tempfile::tempdir().expect("rejection directory");
        let mut rejections = Rejections::load(dir.path());
        for id in 200..=255 {
            rejections.record_mismatch(dir.path(), &offer(id), Timestamp::UNIX_EPOCH);
        }
        let marker = dir.path().join(MARKER);
        let bytes = std::fs::read(&marker).expect("saved cooldowns");
        assert!(bytes.len() as u64 <= MAX_BYTES);
        let loaded = Rejections::load(dir.path());
        assert_eq!(loaded.0.len(), MAX_ENTRIES);
        assert!(loaded.blocks(&offer(255), Timestamp::UNIX_EPOCH));
        assert!(!loaded.blocks(&offer(200), Timestamp::UNIX_EPOCH));

        let mut oversized = bytes;
        oversized.resize(MAX_BYTES as usize + 1, b' ');
        std::fs::write(&marker, oversized).expect("oversized but valid JSON");
        assert!(Rejections::load(dir.path()).0.is_empty());
    }

    #[test]
    fn legacy_untimed_rejection_can_retry_immediately() {
        let dir = tempfile::tempdir().expect("rejection directory");
        let offer = offer(9);
        super::super::atomic_json(&dir.path().join(MARKER), &[&offer]).expect("legacy marker");
        assert!(!Rejections::load(dir.path()).blocks(&offer, Timestamp::UNIX_EPOCH));
    }

    #[test]
    fn clock_rollback_does_not_prolong_a_cooldown() {
        let dir = tempfile::tempdir().expect("rejection directory");
        let offer = offer(9);
        let mut rejections = Rejections::load(dir.path());
        let future = Timestamp::from_second(3600).expect("valid time");
        rejections.record_mismatch(dir.path(), &offer, future);
        assert!(!rejections.blocks(&offer, Timestamp::UNIX_EPOCH));
    }
}
