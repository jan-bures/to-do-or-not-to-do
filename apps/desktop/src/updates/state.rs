//! Installation-wide state shared by the updater, Settings and the tray. No feed credentials or
//! network errors cross this boundary.

use std::time::{Duration, Instant};
use todo_core::time::Timestamp;

const CHECK_INTERVAL: Duration = Duration::from_hours(6);
pub(super) const MAX_JITTER: Duration = Duration::from_mins(30);
pub const FOCUS_IDLE: Duration = Duration::from_mins(30);
const RETRY_INTERVALS: [Duration; 3] = [
    Duration::from_mins(5),
    Duration::from_mins(30),
    CHECK_INTERVAL,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Check,
    Verify,
    Download,
    Apply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Disabled,
    Idle,
    UpToDate { checked_at: Timestamp },
    Checking,
    Downloading { progress: Option<u8> },
    Ready { version: String },
    Failed { kind: Failure, at: Timestamp },
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub installed_version: Option<String>,
    pub state: State,
    pub can_check: bool,
    pub restart_failed: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::new(None, State::NotInstalled)
    }
}

impl Snapshot {
    pub(super) fn new(installed_version: Option<String>, state: State) -> Self {
        Self {
            installed_version,
            state,
            can_check: false,
            restart_failed: false,
        }
    }

    pub fn ready_version(&self) -> Option<&str> {
        match &self.state {
            State::Ready { version } => Some(version),
            _ => None,
        }
    }

    pub fn footer_action(&self) -> bool {
        self.ready_version().is_some()
            || (matches!(self.state, State::Failed { .. }) && self.can_check)
    }

    pub(super) fn begin(&mut self) -> bool {
        if !self.can_check {
            return false;
        }
        self.set(State::Checking, true);
        true
    }

    pub(super) fn set(&mut self, state: State, has_worker: bool) {
        self.can_check = has_worker
            && matches!(
                state,
                State::Idle | State::UpToDate { .. } | State::Failed { .. }
            );
        if state != self.state {
            self.restart_failed = false;
        }
        self.state = state;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Check {
    Automatic,
    Focus,
    Manual,
}

/// Manual checks never postpone an automatic retry or consume a step in its backoff.
#[derive(Debug)]
pub(super) struct Schedule {
    pub due: Instant,
    failures: usize,
    jitter: Duration,
    last_finished: Option<Instant>,
}

impl Schedule {
    pub fn new(now: Instant, jitter: Duration) -> Self {
        Self {
            due: now,
            failures: 0,
            jitter,
            last_finished: None,
        }
    }

    pub fn focus_due(&self, now: Instant) -> bool {
        // Don't cut short automatic backoff, nor repeat an attempt just made in the background.
        now >= self.due
            || (self.failures == 0
                && self
                    .last_finished
                    .is_none_or(|last| now.duration_since(last) >= FOCUS_IDLE))
    }

    pub fn finished(&mut self, check: Check, success: bool, now: Instant) {
        self.last_finished = Some(now);
        if check == Check::Manual {
            return;
        }
        let delay = if success {
            self.failures = 0;
            CHECK_INTERVAL + self.jitter
        } else {
            let delay = RETRY_INTERVALS[self.failures];
            let delay = if delay == CHECK_INTERVAL {
                delay + self.jitter
            } else {
                delay
            };
            self.failures = (self.failures + 1).min(RETRY_INTERVALS.len() - 1);
            delay
        };
        self.due = now + delay;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_checks_debounce_without_consuming_or_postponing_automatic_backoff() {
        let start = Instant::now();
        let jitter = Duration::from_mins(12);
        let mut schedule = Schedule::new(start, jitter);
        assert_eq!(schedule.due, start);
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Disabled);
        assert!(!view.begin());
        view.set(State::Idle, true);
        assert!(view.begin());
        assert_eq!(view.state, State::Checking);
        assert!(!view.begin());
        let at = Timestamp::UNIX_EPOCH;
        schedule.finished(Check::Manual, false, start);
        view.set(
            State::Failed {
                kind: Failure::Check,
                at,
            },
            true,
        );
        assert_eq!(schedule.due, start);

        let first = schedule.due;
        assert!(view.begin());
        schedule.finished(Check::Automatic, false, first);
        view.set(
            State::Failed {
                kind: Failure::Check,
                at,
            },
            true,
        );
        assert_eq!(schedule.due, first + Duration::from_mins(5));
        let retry = schedule.due;
        assert!(view.begin());
        let halfway = first + Duration::from_mins(2);
        schedule.finished(Check::Manual, true, halfway);
        view.set(State::UpToDate { checked_at: at }, true);
        assert_eq!(schedule.due, retry);

        assert!(view.begin());
        schedule.finished(Check::Automatic, false, retry);
        assert_eq!(schedule.due, retry + Duration::from_mins(30));
        let next = schedule.due;
        schedule.finished(Check::Automatic, false, next);
        assert_eq!(schedule.due, next + CHECK_INTERVAL + jitter);
        let next = schedule.due;
        schedule.finished(Check::Automatic, false, next);
        assert_eq!(schedule.due, next + CHECK_INTERVAL + jitter);
        let next = schedule.due;
        schedule.finished(Check::Automatic, true, next);
        assert_eq!(schedule.due, next + CHECK_INTERVAL + jitter);
        let next = schedule.due;
        schedule.finished(Check::Automatic, false, next);
        assert_eq!(schedule.due, next + Duration::from_mins(5));
    }

    #[test]
    fn focus_checks_respect_recent_attempts_and_backoff() {
        let start = Instant::now();
        let mut schedule = Schedule::new(start, Duration::ZERO);
        assert!(schedule.focus_due(start));
        schedule.finished(Check::Automatic, true, start);
        assert!(!schedule.focus_due(start + FOCUS_IDLE - Duration::from_secs(1)));
        let focus = start + FOCUS_IDLE;
        assert!(schedule.focus_due(focus));
        schedule.finished(Check::Focus, false, focus);
        assert!(!schedule.focus_due(focus + Duration::from_mins(1)));
        assert!(schedule.focus_due(focus + Duration::from_mins(5)));
    }

    #[test]
    fn pending_update_remains_actionable_when_detached() {
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Idle);
        view.set(State::Idle, true);
        assert!(view.begin());
        view.set(State::Downloading { progress: None }, true);
        assert!(!view.begin());
        view.set(State::Downloading { progress: Some(50) }, true);
        assert!(!view.begin());
        let ready = |version: &str| State::Ready {
            version: version.into(),
        };
        view.set(ready("0.1.0-build.10"), true);
        assert!(!view.begin());
        view.set(ready("0.1.0-build.10"), false);
        assert_eq!(view.ready_version(), Some("0.1.0-build.10"));
        assert!(view.footer_action());
    }
}
