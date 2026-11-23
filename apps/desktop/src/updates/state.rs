//! Installation-wide state shared by the updater, Settings and the tray. No feed credentials or
//! network errors cross this boundary.

use std::time::{Duration, Instant};
use todo_core::time::Timestamp;

const INITIAL_CHECK_DELAY: Duration = Duration::from_mins(1);
const CHECK_INTERVAL: Duration = Duration::from_hours(6);
const RETRY_INTERVALS: [Duration; 3] = [
    Duration::from_mins(5),
    Duration::from_mins(30),
    CHECK_INTERVAL,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Check,
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
    dismissed_version: Option<String>,
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
            dismissed_version: None,
        }
    }

    pub(super) fn with_dismissal(mut self, version: Option<String>) -> Self {
        self.dismissed_version = version;
        self
    }

    pub fn ready_version(&self) -> Option<&str> {
        match &self.state {
            State::Ready { version } => Some(version),
            _ => None,
        }
    }

    pub fn indicator_version(&self) -> Option<&str> {
        self.ready_version()
            .filter(|version| Some(*version) != self.dismissed_version.as_deref())
    }

    pub(super) fn dismiss(&mut self, version: &str) -> bool {
        if self.indicator_version() != Some(version) {
            return false;
        }
        self.dismissed_version = Some(version.to_owned());
        true
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
    Manual,
}

/// Manual checks never postpone an automatic retry or consume a step in its backoff.
#[derive(Debug)]
pub(super) struct Schedule {
    pub due: Instant,
    failures: usize,
}

impl Schedule {
    pub fn new(now: Instant) -> Self {
        Self {
            due: now + INITIAL_CHECK_DELAY,
            failures: 0,
        }
    }

    pub fn finished(&mut self, check: Check, success: bool, now: Instant) {
        if check == Check::Manual {
            return;
        }
        let delay = if success {
            self.failures = 0;
            CHECK_INTERVAL
        } else {
            let delay = RETRY_INTERVALS[self.failures];
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
        let mut schedule = Schedule::new(start);
        assert_eq!(schedule.due, start + INITIAL_CHECK_DELAY);
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Disabled);
        assert!(!view.begin());
        view.set(State::Idle, true);
        assert!(view.begin());
        assert_eq!(view.state, State::Checking);
        assert!(!view.begin());
        let at = Timestamp::UNIX_EPOCH;
        schedule.finished(Check::Manual, false, start + Duration::from_secs(2));
        view.set(
            State::Failed {
                kind: Failure::Check,
                at,
            },
            true,
        );
        assert_eq!(schedule.due, start + INITIAL_CHECK_DELAY);

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
        assert_eq!(schedule.due, next + CHECK_INTERVAL);
        let next = schedule.due;
        schedule.finished(Check::Automatic, false, next);
        assert_eq!(schedule.due, next + CHECK_INTERVAL);
        let next = schedule.due;
        schedule.finished(Check::Automatic, true, next);
        assert_eq!(schedule.due, next + CHECK_INTERVAL);
        let next = schedule.due;
        schedule.finished(Check::Automatic, false, next);
        assert_eq!(schedule.due, next + Duration::from_mins(5));
    }

    #[test]
    fn pending_update_survives_detaching_and_only_its_indicator_is_dismissed() {
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
        assert!(!view.dismiss("0.1.0-build.9"));
        assert!(view.dismiss("0.1.0-build.10"));
        view.set(ready("0.1.0-build.10"), false);
        assert_eq!(view.ready_version(), Some("0.1.0-build.10"));
        assert_eq!(view.indicator_version(), None);
        view.set(ready("0.1.0-build.11"), true);
        assert_eq!(view.indicator_version(), view.ready_version());
    }
}
