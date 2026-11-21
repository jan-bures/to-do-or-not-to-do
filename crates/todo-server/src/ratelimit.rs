//! Per-principal limits on event-stream (re)connects: a token bucket, and the catch-up marks that make
//! an auto-reconnect after `catch_up` cheap.

use crate::auth::Principal;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Allows `burst` connects at once and `per_minute` sustained, per principal.
///
/// Only authenticated principals reach the limiter, and there are as many as configured tokens, so
/// the map stays small.
#[derive(Debug)]
pub struct ConnectLimiter {
    per_sec: f64,
    burst: f64,
    buckets: Mutex<HashMap<Principal, Bucket>>,
}

impl ConnectLimiter {
    /// A limiter with the given sustained rate and burst (both at least 1).
    #[must_use]
    pub fn new(per_minute: u32, burst: u32) -> Self {
        Self {
            per_sec: f64::from(per_minute.max(1)) / 60.0,
            burst: f64::from(burst.max(1)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a token for `who`, or returns the whole seconds until one is available.
    pub fn check(&self, who: &Principal) -> Result<(), u32> {
        self.check_at(who, Instant::now())
    }

    fn check_at(&self, who: &Principal, now: Instant) -> Result<(), u32> {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let b = buckets.entry(who.clone()).or_insert(Bucket {
            tokens: self.burst,
            at: now,
        });
        let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.per_sec).min(self.burst);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            let wait = ((1.0 - b.tokens) / self.per_sec).ceil();
            // `wait` is positive and at most 60 s for `per_minute >= 1`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Err((wait as u32).max(1))
        }
    }
}

/// The highest point at which each principal's event streams ended with an in-stream `catch_up`.
///
/// An `EventSource` reconnects on its own when a stream ends, to the same position. A reconnect at or
/// before the mark is answered with 409 `catch_up_required` straight away, without a stream slot or a
/// database read, and a 409 stops `EventSource` from retrying.
///
/// A principal can have several streams at once (one device, two windows). So the mark only ever
/// rises: a stream connecting from a later point is allowed but does not clear it, and only a JSON page
/// that reaches past the mark ([`paged`](Self::paged)) does.
#[derive(Debug, Default)]
pub struct CatchUpMarks {
    marks: Mutex<HashMap<Principal, u64>>,
}

impl CatchUpMarks {
    /// Records that a stream of `who` gave up at `since`; keeps the higher of this and any earlier mark.
    pub fn set(&self, who: &Principal, since: u64) {
        let mut marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        let mark = marks.entry(who.clone()).or_insert(since);
        *mark = (*mark).max(since);
    }

    /// True if `who` must catch up before streaming from `since`. Never clears the mark.
    #[must_use]
    pub fn must_catch_up(&self, who: &Principal, since: u64) -> bool {
        self.marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(who)
            .is_some_and(|&mark| since <= mark)
    }

    /// `who` read the log (JSON) up to `reached`; clears the mark once that is past it.
    pub fn paged(&self, who: &Principal, reached: u64) {
        let mut marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        if marks.get(who).is_some_and(|&mark| reached > mark) {
            marks.remove(who);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use todo_core::DeviceId;

    fn owner(name: &str) -> Principal {
        Principal::Owner {
            device: DeviceId::new(name).unwrap(),
        }
    }

    #[test]
    fn burst_then_sustained_rate_per_principal() {
        let l = ConnectLimiter::new(60, 3);
        let (a, b) = (owner("a"), owner("b"));
        let t0 = Instant::now();
        for _ in 0..3 {
            l.check_at(&a, t0).unwrap();
        }
        assert_eq!(l.check_at(&a, t0), Err(1));
        // Another principal has its own bucket.
        l.check_at(&b, t0).unwrap();
        // One token per second refills.
        l.check_at(&a, t0 + Duration::from_millis(1000)).unwrap();
        assert!(l.check_at(&a, t0 + Duration::from_millis(1100)).is_err());
        // Never more than the burst.
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..3 {
            l.check_at(&a, later).unwrap();
        }
        assert!(l.check_at(&a, later).is_err());
    }

    #[test]
    fn catch_up_marks_rise_and_only_paging_past_them_clears_them() {
        let m = CatchUpMarks::default();
        let (a, b) = (owner("a"), owner("b"));
        assert!(!m.must_catch_up(&a, 0));
        m.set(&a, 7);
        assert!(m.must_catch_up(&a, 7));
        assert!(m.must_catch_up(&a, 3));
        assert!(!m.must_catch_up(&b, 7), "marks are per principal");
        // Another stream of the same principal, further ahead, may connect without clearing the mark.
        assert!(!m.must_catch_up(&a, 20));
        assert!(m.must_catch_up(&a, 7));
        // A lower mark from another stream does not lower it; a higher one raises it.
        m.set(&a, 5);
        assert!(m.must_catch_up(&a, 7));
        m.set(&a, 12);
        assert!(m.must_catch_up(&a, 10));
        // Paging up to the mark is not enough; past it clears it.
        m.paged(&a, 12);
        assert!(m.must_catch_up(&a, 12));
        m.paged(&a, 13);
        assert!(!m.must_catch_up(&a, 12));
        assert!(!m.must_catch_up(&a, 0));
    }
}
