//! Installation-wide state shared by the updater, Settings and the tray. No feed credentials or
//! network errors cross this boundary.

use std::time::{Duration, Instant};
use todo_core::time::Timestamp;

const CHECK_INTERVAL: Duration = Duration::from_hours(6);
const INITIAL_DELAY: Duration = Duration::from_secs(45);
const CHECK_FAILURES_SHOWN: usize = 3;
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
    Checking { manual: bool },
    Downloading { progress: Option<u8> },
    Ready { version: String },
    Skipped { build: u32 },
    Failed { kind: Failure, at: Timestamp },
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub installed_version: Option<String>,
    pub state: State,
    pub can_check: bool,
    pub restart_failed: bool,
    pub quit_deferred: bool,
    check_failures: usize,
    show_check_failure: bool,
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
            quit_deferred: false,
            check_failures: 0,
            show_check_failure: false,
        }
    }

    pub fn ready_version(&self) -> Option<&str> {
        match &self.state {
            State::Ready { version } => Some(version),
            _ => None,
        }
    }

    pub fn skipped_label(&self) -> Option<String> {
        match self.state {
            State::Skipped { build } => Some(format!(
                "Skipped build {build} after a failed restart · Retry"
            )),
            _ => None,
        }
    }

    pub fn visible_failure(&self) -> bool {
        match self.state {
            State::Failed {
                kind: Failure::Check,
                ..
            } => self.show_check_failure,
            State::Skipped { .. } | State::Failed { .. } => true,
            _ => false,
        }
    }

    pub fn footer_action(&self) -> bool {
        self.ready_version().is_some() || (self.visible_failure() && self.can_check)
    }

    pub(super) fn begin(&mut self, check: Check) -> bool {
        if !self.can_check {
            return false;
        }
        self.set(
            State::Checking {
                manual: check == Check::Manual,
            },
            true,
        );
        true
    }

    pub(super) fn checked(&mut self, check: Check, state: State, has_worker: bool) {
        if matches!(
            state,
            State::Failed {
                kind: Failure::Check,
                ..
            }
        ) {
            self.check_failures = (self.check_failures + 1).min(CHECK_FAILURES_SHOWN);
            self.show_check_failure =
                check == Check::Manual || self.check_failures >= CHECK_FAILURES_SHOWN;
        } else {
            self.check_failures = 0;
            self.show_check_failure = false;
        }
        self.set(state, has_worker);
    }

    pub(super) fn set(&mut self, state: State, has_worker: bool) {
        self.can_check = has_worker
            && matches!(
                state,
                State::Idle | State::UpToDate { .. } | State::Skipped { .. } | State::Failed { .. }
            );
        if state != self.state {
            self.restart_failed = false;
            self.quit_deferred = false;
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
            due: now + INITIAL_DELAY,
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
                    .is_some_and(|last| now.duration_since(last) >= FOCUS_IDLE))
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
        assert_eq!(schedule.due, start + INITIAL_DELAY);
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Disabled);
        assert!(!view.begin(Check::Manual));
        view.set(State::Idle, true);
        assert!(view.begin(Check::Manual));
        assert_eq!(view.state, State::Checking { manual: true });
        assert!(!view.begin(Check::Manual));
        let at = Timestamp::UNIX_EPOCH;
        schedule.finished(Check::Manual, false, start);
        view.set(
            State::Failed {
                kind: Failure::Check,
                at,
            },
            true,
        );
        assert_eq!(schedule.due, start + INITIAL_DELAY);

        let first = schedule.due;
        assert!(view.begin(Check::Automatic));
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
        assert!(view.begin(Check::Manual));
        let halfway = first + Duration::from_mins(2);
        schedule.finished(Check::Manual, true, halfway);
        view.set(State::UpToDate { checked_at: at }, true);
        assert_eq!(schedule.due, retry);

        assert!(view.begin(Check::Automatic));
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
        assert!(!schedule.focus_due(start));
        assert!(schedule.focus_due(start + INITIAL_DELAY));
        schedule.finished(Check::Automatic, true, start + INITIAL_DELAY);
        let focus = start + INITIAL_DELAY + FOCUS_IDLE;
        assert!(!schedule.focus_due(focus - Duration::from_secs(1)));
        assert!(schedule.focus_due(focus));
        schedule.finished(Check::Focus, false, focus);
        assert!(!schedule.focus_due(focus + Duration::from_mins(1)));
        assert!(schedule.focus_due(focus + Duration::from_mins(5)));
    }

    #[test]
    fn only_manual_checks_and_three_consecutive_background_failures_reach_the_footer() {
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Idle);
        view.set(State::Idle, true);
        let at = Timestamp::UNIX_EPOCH;
        let failure = State::Failed {
            kind: Failure::Check,
            at,
        };
        for (check, visible) in [
            (Check::Automatic, false),
            (Check::Focus, false),
            (Check::Automatic, true),
        ] {
            assert!(view.begin(check));
            assert_eq!(view.state, State::Checking { manual: false });
            view.checked(check, failure.clone(), true);
            assert_eq!(view.state, failure); // Settings sees every failure.
            assert_eq!(view.footer_action(), visible);
        }
        view.checked(Check::Automatic, State::UpToDate { checked_at: at }, true);
        view.checked(Check::Focus, failure.clone(), true);
        assert!(!view.footer_action());
        view.checked(Check::Manual, failure, true);
        assert!(view.footer_action());
    }

    #[test]
    fn pending_update_remains_actionable_when_detached() {
        let mut view = Snapshot::new(Some("0.1.0-build.9".into()), State::Idle);
        view.set(State::Idle, true);
        assert!(view.begin(Check::Manual));
        view.set(State::Downloading { progress: None }, true);
        assert!(!view.begin(Check::Manual));
        view.set(State::Downloading { progress: Some(50) }, true);
        assert!(!view.begin(Check::Manual));
        let ready = |version: &str| State::Ready {
            version: version.into(),
        };
        view.set(ready("0.1.0-build.10"), true);
        assert!(!view.begin(Check::Manual));
        view.set(ready("0.1.0-build.10"), false);
        assert_eq!(view.ready_version(), Some("0.1.0-build.10"));
        assert!(view.footer_action());
    }
}
