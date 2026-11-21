//! In-memory rate limits: token buckets per key (client IP, bearer principal), the agents' write
//! quota (a sliding window per token), the sign-in failure throttle, and failure-burst reporting.
//!
//! All of these reset when the server restarts. That is deliberate for the write quota too: it bounds
//! how fast an agent can change things, and a restart is an owner action.
//!
//! **Memory is bounded.** Per-key state is a few fixed-size counters, never a list of times, and each
//! map holds at most [`MAX_KEYS`] keys: when it is full, idle keys are dropped, and if it is still full
//! every further key shares one overflow entry (so rotating keys, for example spoofed or many source
//! addresses, cannot grow memory; at worst those sources share one budget).

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Most keys a limiter or tracker holds (see the module docs).
pub const MAX_KEYS: usize = 10_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Debug)]
struct Buckets<K> {
    map: HashMap<K, Bucket>,
    /// Shared by every key that arrives while `map` is full.
    overflow: Option<Bucket>,
}

/// A token bucket per key: `burst` at once, `per_minute` sustained.
#[derive(Debug)]
pub struct KeyedLimiter<K> {
    per_sec: f64,
    burst: f64,
    buckets: Mutex<Buckets<K>>,
}

impl<K: Eq + Hash + Clone> KeyedLimiter<K> {
    /// A limiter (both rates at least 1).
    #[must_use]
    pub fn new(per_minute: u32, burst: u32) -> Self {
        Self {
            per_sec: f64::from(per_minute.max(1)) / 60.0,
            burst: f64::from(burst.max(1)),
            buckets: Mutex::new(Buckets {
                map: HashMap::new(),
                overflow: None,
            }),
        }
    }

    fn refill(&self, b: &mut Bucket, now: Instant) {
        let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.per_sec).min(self.burst);
        b.at = now;
    }

    fn wait(&self, b: &Bucket) -> u32 {
        let wait = ((1.0 - b.tokens) / self.per_sec).ceil();
        // Positive and at most 60 s for `per_minute >= 1`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let wait = (wait as u32).max(1);
        wait
    }

    /// The bucket for `key`, making room or falling back to the overflow bucket when full.
    fn bucket<'a>(&self, buckets: &'a mut Buckets<K>, key: &K, now: Instant) -> &'a mut Bucket {
        let fresh = Bucket {
            tokens: self.burst,
            at: now,
        };
        if !buckets.map.contains_key(key) && buckets.map.len() >= MAX_KEYS {
            // Drop buckets that have refilled completely: they hold no information.
            let full = Duration::from_secs_f64(self.burst / self.per_sec);
            buckets
                .map
                .retain(|_, b| now.saturating_duration_since(b.at) < full);
            if buckets.map.len() >= MAX_KEYS {
                return buckets.overflow.get_or_insert(fresh);
            }
        }
        buckets.map.entry(key.clone()).or_insert(fresh)
    }

    /// Takes a token for `key`, or returns the whole seconds until one is available.
    pub fn check(&self, key: &K) -> Result<(), u32> {
        self.check_at(key, Instant::now())
    }

    /// [`check`](Self::check) at a given time.
    pub fn check_at(&self, key: &K, now: Instant) -> Result<(), u32> {
        let mut buckets = lock(&self.buckets);
        let b = self.bucket(&mut buckets, key, now);
        let mut b2 = *b;
        self.refill(&mut b2, now);
        *b = b2;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(self.wait(b))
        }
    }

    /// Whether `key` has a token, without taking it and without tracking a new key.
    pub fn peek(&self, key: &K) -> Result<(), u32> {
        self.peek_at(key, Instant::now())
    }

    /// [`peek`](Self::peek) at a given time.
    pub fn peek_at(&self, key: &K, now: Instant) -> Result<(), u32> {
        let buckets = lock(&self.buckets);
        let b = match buckets.map.get(key) {
            Some(b) => *b,
            None if buckets.map.len() >= MAX_KEYS => match buckets.overflow {
                Some(b) => b,
                None => return Ok(()),
            },
            None => return Ok(()),
        };
        let mut b = b;
        self.refill(&mut b, now);
        if b.tokens >= 1.0 {
            Ok(())
        } else {
            Err(self.wait(&b))
        }
    }

    /// How many keys are tracked (not counting the overflow bucket).
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.buckets).map.len()
    }

    /// True if no key is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The write quota: at most N writes per token in any rolling hour (a sliding window of write times).
#[derive(Debug, Default)]
pub struct WriteQuotas {
    windows: Mutex<HashMap<i64, VecDeque<Instant>>>,
}

/// The quota window.
pub const QUOTA_WINDOW: Duration = Duration::from_secs(3600);

impl WriteQuotas {
    /// Counts a write by `token` against `per_hour`, or returns the seconds until the oldest write in
    /// the window expires.
    pub fn take(&self, token: i64, per_hour: u32) -> Result<(), u32> {
        self.take_at(token, per_hour, Instant::now())
    }

    /// [`take`](Self::take) at a given time.
    pub fn take_at(&self, token: i64, per_hour: u32, now: Instant) -> Result<(), u32> {
        let mut windows = lock(&self.windows);
        let w = windows.entry(token).or_default();
        while w
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= QUOTA_WINDOW)
        {
            w.pop_front();
        }
        let cap = usize::try_from(per_hour).unwrap_or(usize::MAX);
        if w.len() >= cap {
            let oldest = w.front().copied().unwrap_or(now);
            let left = QUOTA_WINDOW.saturating_sub(now.saturating_duration_since(oldest));
            return Err(u32::try_from(left.as_secs()).unwrap_or(u32::MAX).max(1));
        }
        w.push_back(now);
        Ok(())
    }

    /// Forgets a token (on revocation).
    pub fn forget(&self, token: i64) {
        lock(&self.windows).remove(&token);
    }
}

/// Reports bursts: at least [`BURST_THRESHOLD`] events within a fixed minute, at most once a minute.
#[derive(Debug, Default)]
pub struct BurstCounter {
    /// Start of the current minute, events in it, and whether it was reported.
    window: Mutex<Option<(Instant, usize, bool)>>,
}

impl BurstCounter {
    /// Counts one event; returns the count when this event makes the minute a burst.
    pub fn hit_at(&self, now: Instant) -> Option<usize> {
        let minute = Duration::from_secs(60);
        let mut w = lock(&self.window);
        let (start, count, reported) = w.get_or_insert((now, 0, false));
        if now.saturating_duration_since(*start) >= minute {
            *start = now;
            *count = 0;
            *reported = false;
        }
        *count = count.saturating_add(1);
        if *count >= BURST_THRESHOLD && !*reported {
            *reported = true;
            Some(*count)
        } else {
            None
        }
    }

    /// [`hit_at`](Self::hit_at) now.
    pub fn hit(&self) -> Option<usize> {
        self.hit_at(Instant::now())
    }
}

#[derive(Debug, Clone, Copy)]
struct Strikes {
    /// Start of the counting window.
    start: Instant,
    count: u32,
    /// Set when `count` reached the threshold.
    locked_until: Option<Instant>,
}

impl Strikes {
    fn fresh(now: Instant) -> Self {
        Self {
            start: now,
            count: 0,
            locked_until: None,
        }
    }

    /// Resets an expired window (and an expired lockout).
    fn roll(&mut self, window: Duration, now: Instant) {
        if self.idle(window, now) {
            *self = Self::fresh(now);
        }
    }

    fn idle(&self, window: Duration, now: Instant) -> bool {
        match self.locked_until {
            Some(u) => now >= u,
            None => now.saturating_duration_since(self.start) >= window,
        }
    }
}

#[derive(Debug)]
struct StrikeMap<K> {
    map: HashMap<K, Strikes>,
    overflow: Option<Strikes>,
}

/// Counts failed sign-ins per key (client IP) in a fixed window and locks a key out for the window
/// once it reaches the threshold. Also reports bursts of failures across all keys, for the audit log.
#[derive(Debug)]
pub struct FailureTracker<K> {
    threshold: u32,
    window: Duration,
    per_key: Mutex<StrikeMap<K>>,
    burst: BurstCounter,
}

/// Failures across all clients within a minute that count as a burst.
pub const BURST_THRESHOLD: usize = 20;

impl<K: Eq + Hash + Clone> FailureTracker<K> {
    /// A tracker.
    #[must_use]
    pub fn new(threshold: u32, window: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            window,
            per_key: Mutex::new(StrikeMap {
                map: HashMap::new(),
                overflow: None,
            }),
            burst: BurstCounter::default(),
        }
    }

    fn secs_left(until: Instant, now: Instant) -> u32 {
        u32::try_from(until.saturating_duration_since(now).as_secs())
            .unwrap_or(u32::MAX)
            .max(1)
    }

    /// Seconds until `key` may try again, if it is locked out.
    pub fn locked(&self, key: &K) -> Option<u32> {
        self.locked_at(key, Instant::now())
    }

    /// [`locked`](Self::locked) at a given time.
    pub fn locked_at(&self, key: &K, now: Instant) -> Option<u32> {
        let m = lock(&self.per_key);
        let s = match m.map.get(key) {
            Some(s) => *s,
            None if m.map.len() >= MAX_KEYS => m.overflow?,
            None => return None,
        };
        s.locked_until
            .filter(|u| now < *u)
            .map(|u| Self::secs_left(u, now))
    }

    /// Records a failure. Returns what it caused: whether `key` is now locked out, and whether this
    /// started a burst across all keys (reported at most once a minute).
    pub fn fail(&self, key: &K) -> Failure {
        self.fail_at(key, Instant::now())
    }

    /// [`fail`](Self::fail) at a given time.
    pub fn fail_at(&self, key: &K, now: Instant) -> Failure {
        let locked_out = {
            let mut m = lock(&self.per_key);
            let window = self.window;
            if !m.map.contains_key(key) && m.map.len() >= MAX_KEYS {
                m.map.retain(|_, s| !s.idle(window, now));
            }
            let s = if m.map.contains_key(key) || m.map.len() < MAX_KEYS {
                m.map
                    .entry(key.clone())
                    .or_insert_with(|| Strikes::fresh(now))
            } else {
                m.overflow.get_or_insert_with(|| Strikes::fresh(now))
            };
            s.roll(window, now);
            s.count = s.count.saturating_add(1);
            if s.count == self.threshold {
                s.locked_until = Some(now + window);
                true
            } else {
                false
            }
        };
        Failure {
            locked_out,
            burst: self.burst.hit_at(now),
        }
    }

    /// Clears `key`'s failures (after a successful sign-in).
    pub fn succeed(&self, key: &K) {
        lock(&self.per_key).map.remove(key);
    }

    /// How many keys are tracked (not counting the overflow entry).
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.per_key).map.len()
    }

    /// True if no key is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What a recorded failure caused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// The key just reached the lockout threshold.
    pub locked_out: bool,
    /// A burst of this many failures across all keys just started.
    pub burst: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_refill_per_key() {
        let l = KeyedLimiter::new(60, 2);
        let t = Instant::now();
        l.check_at(&1, t).unwrap();
        l.check_at(&1, t).unwrap();
        assert_eq!(l.check_at(&1, t), Err(1));
        l.check_at(&2, t).unwrap();
        l.check_at(&1, t + Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn the_write_quota_is_a_sliding_hour() {
        let q = WriteQuotas::default();
        let t = Instant::now();
        q.take_at(7, 2, t).unwrap();
        q.take_at(7, 2, t + Duration::from_secs(10)).unwrap();
        let wait = q.take_at(7, 2, t + Duration::from_secs(20)).unwrap_err();
        assert_eq!(wait, 3580);
        q.take_at(8, 2, t).unwrap();
        // The first write leaves the window after an hour.
        q.take_at(7, 2, t + QUOTA_WINDOW).unwrap();
        assert!(q.take_at(7, 2, t + QUOTA_WINDOW).is_err());
        q.forget(7);
        q.take_at(7, 2, t + QUOTA_WINDOW).unwrap();
    }

    #[test]
    fn limiter_memory_is_capped_and_overflow_keys_share_a_bucket() {
        let l = KeyedLimiter::new(1, 1);
        let t = Instant::now();
        for k in 0..MAX_KEYS {
            l.check_at(&k, t).unwrap();
        }
        assert_eq!(l.len(), MAX_KEYS);
        // Full, and nothing is idle: new keys share one bucket instead of growing the map.
        l.check_at(&(MAX_KEYS + 1), t).unwrap();
        assert!(l.check_at(&(MAX_KEYS + 2), t).is_err());
        assert!(l.peek_at(&(MAX_KEYS + 3), t).is_err());
        assert_eq!(l.len(), MAX_KEYS);
        // Once buckets have refilled, they are dropped and new keys get their own again.
        let later = t + Duration::from_secs(120);
        l.check_at(&(MAX_KEYS + 4), later).unwrap();
        assert!(l.len() <= 1);
    }

    #[test]
    fn peeking_neither_takes_nor_tracks() {
        let l = KeyedLimiter::new(60, 1);
        let t = Instant::now();
        assert!(l.peek_at(&1, t).is_ok());
        assert!(l.is_empty());
        l.check_at(&1, t).unwrap();
        assert!(l.peek_at(&1, t).is_err());
        assert!(l.peek_at(&1, t + Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn failure_memory_is_capped() {
        let f = FailureTracker::new(3, Duration::from_secs(60));
        let t = Instant::now();
        for k in 0..(MAX_KEYS * 2) {
            f.fail_at(&k, t);
        }
        assert_eq!(f.len(), MAX_KEYS);
        // The overflow keys share one counter, which is locked out now.
        assert!(f.locked_at(&(MAX_KEYS * 3), t).is_some());
        // After the window, idle keys make room again.
        f.fail_at(&(MAX_KEYS * 3), t + Duration::from_secs(61));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn bursts_are_counted_in_fixed_minutes() {
        let b = BurstCounter::default();
        let t = Instant::now();
        let reported: Vec<_> = (0..BURST_THRESHOLD * 3)
            .filter_map(|_| b.hit_at(t))
            .collect();
        assert_eq!(reported, [BURST_THRESHOLD]);
        let next = t + Duration::from_secs(60);
        let reported = (0..BURST_THRESHOLD).filter_map(|_| b.hit_at(next)).count();
        assert_eq!(reported, 1, "a new minute can report again");
    }

    #[test]
    fn failures_lock_out_and_report_bursts() {
        let f = FailureTracker::new(3, Duration::from_secs(60));
        let t = Instant::now();
        assert!(!f.fail_at(&"a", t).locked_out);
        assert!(!f.fail_at(&"a", t).locked_out);
        assert!(f.locked_at(&"a", t).is_none());
        assert!(f.fail_at(&"a", t).locked_out);
        assert_eq!(f.locked_at(&"a", t), Some(60));
        assert!(f.locked_at(&"b", t).is_none());
        assert!(f.locked_at(&"a", t + Duration::from_secs(60)).is_none());
        f.succeed(&"a");
        let mut bursts = 0;
        for i in 0..(BURST_THRESHOLD * 2) {
            let key = if i % 2 == 0 { "b" } else { "c" };
            if f.fail_at(&key, t).burst.is_some() {
                bursts += 1;
            }
        }
        assert_eq!(bursts, 1, "reported once per minute");
    }
}
