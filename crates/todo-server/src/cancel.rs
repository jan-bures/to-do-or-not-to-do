//! Cancellation of a request's work, with a race-free hand-off between "give up" and "commit".
//!
//! A [`Cancel`] is created per request by the deadline middleware and shared with the blocking work the
//! request starts. It is a small state machine:
//!
//! ```text
//! pending ──cancel()──────▶ cancelled      (the work must not commit)
//!    │
//!    └──begin_commit()────▶ committing     (the work owns the outcome; cancel() now fails)
//! ```
//!
//! Both transitions are a single compare-and-swap, so exactly one side wins. The work calls
//! [`begin_commit`](Cancel::begin_commit) immediately before `COMMIT`; it refuses (and cancels) once the
//! absolute deadline has passed, so a late timer can never let a write slip in after the deadline. The
//! middleware calls [`cancel`](Cancel::cancel) when the deadline passes: if that fails, a commit is in
//! flight and the middleware waits for its outcome instead of reporting a time-out.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

const PENDING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;

#[derive(Debug)]
struct Inner {
    state: AtomicU8,
    deadline: Option<Instant>,
}

/// A request's cancellation state, shared between the handler side and the work it started.
#[derive(Clone, Debug)]
pub struct Cancel(Arc<Inner>);

impl Default for Cancel {
    fn default() -> Self {
        Self::new()
    }
}

impl Cancel {
    /// Pending, without a deadline.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            state: AtomicU8::new(PENDING),
            deadline: None,
        }))
    }

    /// Pending, with an absolute deadline after which the work may no longer start committing.
    #[must_use]
    pub fn with_deadline(deadline: Instant) -> Self {
        Self(Arc::new(Inner {
            state: AtomicU8::new(PENDING),
            deadline: Some(deadline),
        }))
    }

    fn past_deadline(&self) -> bool {
        self.0.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// pending → cancelled. True if the work is now (or already was) cancelled; false if it already
    /// began committing, in which case its outcome stands.
    pub fn cancel(&self) -> bool {
        match self
            .0
            .state
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(s) => s == CANCELLED,
        }
    }

    /// pending → committing, unless the work was cancelled or the deadline has passed (which cancels
    /// it). True if the caller may commit.
    #[must_use]
    pub fn begin_commit(&self) -> bool {
        if self.past_deadline() && self.cancel() {
            return false;
        }
        match self.0.state.compare_exchange(
            PENDING,
            COMMITTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => true,
            Err(s) => s == COMMITTING,
        }
    }

    /// True if the work should stop: cancelled, or past the deadline (and not yet committing).
    /// Long scans call this periodically.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        match self.0.state.load(Ordering::Acquire) {
            CANCELLED => true,
            COMMITTING => false,
            _ => self.past_deadline(),
        }
    }

    /// True once the work began committing.
    #[must_use]
    pub fn is_committing(&self) -> bool {
        self.0.state.load(Ordering::Acquire) == COMMITTING
    }

    /// A guard that calls [`cancel`](Self::cancel) when dropped, unless disarmed. Keep it in a future
    /// that may be dropped (client gone) so the work is cancelled with it.
    #[must_use]
    pub fn on_drop(&self) -> CancelOnDrop {
        CancelOnDrop(Some(self.clone()))
    }
}

/// Cancels a [`Cancel`] when dropped while armed.
#[derive(Debug)]
pub struct CancelOnDrop(Option<Cancel>);

impl CancelOnDrop {
    /// Drops the guard without cancelling.
    pub fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            let _ = c.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancel_and_commit_are_mutually_exclusive() {
        let c = Cancel::new();
        assert!(c.cancel());
        assert!(c.cancel(), "cancelling twice still reports cancelled");
        assert!(!c.begin_commit());
        assert!(c.is_cancelled());

        let c = Cancel::new();
        assert!(c.begin_commit());
        assert!(c.is_committing());
        assert!(!c.cancel(), "a commit in flight cannot be cancelled");
        assert!(!c.is_cancelled());
    }

    #[test]
    fn a_passed_deadline_refuses_the_commit() {
        let c = Cancel::with_deadline(Instant::now() - Duration::from_millis(1));
        assert!(c.is_cancelled());
        assert!(!c.begin_commit());
        assert!(c.cancel());
        let c = Cancel::with_deadline(Instant::now() + Duration::from_secs(60));
        assert!(!c.is_cancelled());
        assert!(c.begin_commit());
    }

    #[test]
    fn the_drop_guard_cancels_unless_disarmed() {
        let c = Cancel::new();
        c.on_drop().disarm();
        assert!(!c.is_cancelled());
        drop(c.on_drop());
        assert!(c.is_cancelled());
    }

    #[test]
    fn concurrent_cancel_and_commit_have_exactly_one_winner() {
        for _ in 0..200 {
            let c = Cancel::new();
            let other = c.clone();
            let t = std::thread::spawn(move || other.begin_commit());
            let cancelled = c.cancel();
            let committed = t.join().unwrap();
            assert_ne!(cancelled, committed);
        }
    }
}
