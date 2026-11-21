//! A runtime-agnostic change signal and timing helpers.

use crate::platform::{Clock, MaybeSend};
use futures_util::future::{Either, select};
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

#[derive(Debug, Default)]
struct Inner {
    generation: u64,
    wakers: Vec<Waker>,
}

/// A generation counter that futures can wait on. Read the generation *before* checking the
/// condition, then wait for it to change, and no notification is lost.
#[derive(Clone, Debug, Default)]
pub struct Notify {
    inner: Arc<Mutex<Inner>>,
}

impl Notify {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Bumps the generation and wakes every waiter.
    pub fn notify(&self) {
        let wakers = {
            let mut g = self.lock();
            g.generation = g.generation.wrapping_add(1);
            std::mem::take(&mut g.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }

    /// Completes once the generation differs from `seen`.
    #[must_use]
    pub fn changed(&self, seen: u64) -> Changed {
        Changed {
            notify: self.clone(),
            seen,
        }
    }
}

/// Future returned by [`Notify::changed`]; yields the new generation.
#[derive(Debug)]
pub struct Changed {
    notify: Notify,
    seen: u64,
}

impl Future for Changed {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        let mut g = self.notify.lock();
        if g.generation != self.seen {
            return Poll::Ready(g.generation);
        }
        if !g.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            g.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// Runs `fut` with a deadline.
pub async fn timeout<C: Clock, T>(
    clock: &C,
    duration: Duration,
    fut: impl Future<Output = T> + MaybeSend,
) -> Option<T> {
    let fut = pin!(fut);
    let sleep = pin!(clock.sleep(duration));
    match select(fut, sleep).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(((), _)) => None,
    }
}

/// Sleeps for `duration`, or less if `wake` changes from `seen`. Returns true if woken early.
pub async fn sleep_or_wake<C: Clock>(
    clock: &C,
    duration: Duration,
    wake: &Notify,
    seen: u64,
) -> bool {
    let sleep = pin!(clock.sleep(duration));
    let woken = pin!(wake.changed(seen));
    matches!(select(sleep, woken).await, Either::Right(_))
}

/// Exponential backoff with "equal jitter": the delay is half the exponential step plus a random
/// share of the other half, so clients that failed together do not retry together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// First delay.
    pub initial: Duration,
    /// Largest delay.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// The delay before retry number `attempt` (0-based), given a random value. At least
    /// `retry_after` when the server asked for it, but never more than `max` (the server's hint can
    /// be long; `max` keeps the client responsive).
    #[must_use]
    pub fn delay(&self, attempt: u32, random: u64, retry_after: Option<Duration>) -> Duration {
        let step = self
            .initial
            .saturating_mul(1u32 << attempt.min(20))
            .min(self.max);
        let half = step / 2;
        let half_ms = u64::try_from(half.as_millis()).unwrap_or(u64::MAX);
        let jitter = if half_ms == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(random % (half_ms + 1))
        };
        let d = half + jitter;
        match retry_after {
            Some(r) => d.max(r).min(self.max),
            None => d,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let b = Backoff {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(1),
        };
        assert_eq!(b.delay(0, 0, None), Duration::from_millis(50));
        assert_eq!(b.delay(0, 50, None), Duration::from_millis(100));
        assert_eq!(b.delay(3, 0, None), Duration::from_millis(400));
        let capped = b.delay(30, 12345, None);
        assert!(capped >= Duration::from_millis(500) && capped <= Duration::from_secs(1));
        assert_eq!(
            b.delay(0, 0, Some(Duration::from_millis(700))),
            Duration::from_millis(700)
        );
        assert_eq!(
            b.delay(0, 0, Some(Duration::from_secs(30))),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn notify_wakes_after_a_change_only() {
        let n = Notify::default();
        let seen = n.generation();
        let waker = futures_util::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = n.changed(seen);
        assert!(Pin::new(&mut fut).poll(&mut cx).is_pending());
        n.notify();
        assert_eq!(Pin::new(&mut fut).poll(&mut cx), Poll::Ready(seen + 1));
    }
}
