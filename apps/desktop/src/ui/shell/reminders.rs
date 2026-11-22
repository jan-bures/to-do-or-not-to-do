//! Reminder toasts while the app runs (also hidden in the tray): the shell keeps them in line with the
//! synced reminders (`model::reminders` decides, `platform::toast` shows them) every time the data
//! changes, and one wake loop brings it back when the next one is due. The toasts' buttons come back
//! here: Open shows the window with the task, Snooze 10 min puts the reminder off on this computer
//! only, and Complete completes the task (Undo works as for any completion). A button acts only on the
//! reminder as its toast showed it, and Snooze and Complete only once (`Reminders::claim`).

use std::cell::Cell;
use std::pin::pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::{Either, select};
use gpui::{AsyncApp, AsyncWindowContext, Context, Task, WeakEntity, Window};
use todo_client::Readiness;
use todo_core::Command;
use todo_core::ids::TaskId;
use todo_core::time::Timestamp;

use super::ShellView;
use crate::config::DataDir;
use crate::model::reminders::{
    Activation, Claim, Occurrence, Outcome, RETRY_MS, Reminders, still_as_shown,
};
use crate::model::{Now, RoomKey};
use crate::platform::toast::{self, ToastEvent};
use crate::platform::win;

/// How long a reminder that is due may be held back when the first attempt to reach the server
/// failed (offline, refused): then it shows from this computer's copy. Until that attempt ends, or
/// until the replica has caught up with the server's head, it waits (`todo_client::Readiness`).
const CATCH_UP: Duration = Duration::from_secs(60);
/// The longest one wake timer runs: a timer left behind by a new deadline ends within this, and a
/// wake that slept through a suspended computer is at most this late.
const WAKE_CHUNK: Duration = Duration::from_secs(60);
/// The shortest wait between two passes the wake loop starts: whatever the clocks say, it yields.
const MIN_WAKE: Duration = Duration::from_millis(250);

/// Whether reminders that are due may show yet, and when to look again if not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Gate {
    pub open: bool,
    /// While closed: how long until it opens by itself (the failed first attempt's fallback). `None`
    /// when only the replica catching up opens it (which wakes the shell anyway).
    pub recheck_in: Option<Duration>,
}

/// The gate for the replica's readiness, `since_start` (monotonic) after the session started.
pub(super) fn gate(readiness: Readiness, since_start: Duration) -> Gate {
    match readiness {
        Readiness::CaughtUp => Gate {
            open: true,
            recheck_in: None,
        },
        Readiness::Failed if since_start >= CATCH_UP => Gate {
            open: true,
            recheck_in: None,
        },
        Readiness::Failed => Gate {
            open: false,
            recheck_in: Some(CATCH_UP - since_start),
        },
        Readiness::Starting => Gate {
            open: false,
            recheck_in: None,
        },
    }
}

/// When the wake loop should look next: the next reminder (wall clock: alarms are instants), or a
/// monotonic deadline (the startup hold, a save to try again), whichever comes first. Never longer
/// than [`WAKE_CHUNK`], never shorter than [`MIN_WAKE`]: a deadline the clock has already passed (it
/// jumped forward) can't make it spin.
pub(super) fn wake_in(
    wall: Option<Timestamp>,
    mono: Option<Instant>,
    now_wall: Timestamp,
    now_mono: Instant,
) -> Option<Duration> {
    let wall = wall.map(|at| {
        let ms = at.as_millisecond() - now_wall.as_millisecond();
        // A moment after it's due, so the pass finds it due.
        Duration::from_millis(u64::try_from(ms.max(0)).unwrap_or(0) + 50)
    });
    let mono = mono.map(|at| at.saturating_duration_since(now_mono) + Duration::from_millis(50));
    let wait = match (wall, mono) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b)?,
    };
    Some(wait.clamp(MIN_WAKE, WAKE_CHUNK))
}

/// What the wake loop waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Deadlines {
    wall: Option<Timestamp>,
    mono: Option<Instant>,
}

impl Deadlines {
    fn due(self, now_wall: Timestamp, now_mono: Instant) -> bool {
        self.wall.is_some_and(|at| at <= now_wall) || self.mono.is_some_and(|at| at <= now_mono)
    }
}

/// The live session's reminders: what was shown, the toasts, and the wake loop.
#[allow(missing_debug_implementations)] // Holds the sink and GPUI tasks.
pub(super) struct Alerts {
    reminders: Reminders,
    data: DataDir,
    started: Instant,
    /// The schedule changed and hasn't been saved yet, and when a failed save is tried again (its
    /// own back-off: a refresh, a keystroke in the filter, never writes before then).
    dirty: bool,
    save_retry: Option<Instant>,
    save_failures: usize,
    /// When the startup hold ends, while something is held for it.
    hold_ends: Option<Instant>,
    deadlines: Rc<Cell<Deadlines>>,
    rewake: UnboundedSender<()>,
    _wake: Task<()>,
    _events: Task<()>,
}

impl ShellView {
    /// Starts the reminder toasts for a live session (fixtures have none).
    pub fn start_reminders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.store.session() else {
            return;
        };
        let data = session.data().clone();
        let (tx, mut rx) = unbounded::<ToastEvent>();
        let sink = toast::WinToasts::new(tx);
        let events = cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                while let Some(event) = rx.next().await {
                    let handled = this.update_in(cx, |this, window, cx| match event {
                        ToastEvent::Activated(arguments) => this.on_toast(&arguments, window, cx),
                        ToastEvent::Failed(launch) => this.toast_failed(&launch, cx),
                    });
                    if handled.is_err() {
                        break;
                    }
                }
            },
        );
        let deadlines = Rc::new(Cell::new(Deadlines::default()));
        let (rewake, wakes) = unbounded::<()>();
        let wake = cx.spawn({
            let deadlines = deadlines.clone();
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                wake_loop(this, deadlines, wakes, cx).await;
            }
        });
        self.alerts = Some(Alerts {
            reminders: Reminders::new(data.load_reminders(), Box::new(sink)),
            data,
            started: Instant::now(),
            dirty: false,
            save_retry: None,
            save_failures: 0,
            hold_ends: None,
            deadlines,
            rewake,
            _wake: wake,
            _events: events,
        });
        self.reconcile_reminders(cx);
    }

    /// Stops them (quitting, signing out): the toasts go, since nothing would answer their buttons.
    pub fn stop_reminders(&mut self) {
        if let Some(mut alerts) = self.alerts.take() {
            alerts.reminders.clear();
        }
    }

    /// Brings the toasts in line with the data now, and sets the wake for the next reminder. The state
    /// and its readiness are read together, so a replica still catching up never rings.
    pub(super) fn reconcile_reminders(&mut self, cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        let (state, readiness) = self.store.view_and_readiness();
        let gate = gate(readiness, alerts.started.elapsed());
        let outcome = alerts
            .reminders
            .reconcile(&state, &Now::system(), gate.open);
        alerts.hold_ends = (outcome.held && !gate.open)
            .then_some(gate.recheck_in)
            .flatten()
            .map(|d| Instant::now() + d);
        self.after_reminders(outcome, cx);
    }

    /// Saves what changed (a failed save is tried again on its own back-off) and sets the wake loop's
    /// deadlines.
    fn after_reminders(&mut self, outcome: Outcome, _cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        alerts.dirty |= outcome.changed;
        let now = Instant::now();
        if alerts.dirty && alerts.save_retry.is_none_or(|at| now >= at) {
            match alerts.data.save_reminders(alerts.reminders.schedule()) {
                Ok(()) => {
                    alerts.dirty = false;
                    alerts.save_retry = None;
                    alerts.save_failures = 0;
                }
                Err(e) => {
                    tracing::warn!("saving the reminders' schedule failed: {e:#}");
                    let wait = RETRY_MS[alerts.save_failures.min(RETRY_MS.len() - 1)];
                    alerts.save_failures += 1;
                    alerts.save_retry =
                        Some(now + Duration::from_millis(u64::try_from(wait).unwrap_or(0)));
                }
            }
        }
        let save = alerts.save_retry.filter(|_| alerts.dirty);
        let mono = match (alerts.hold_ends, save) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let next = Deadlines {
            wall: outcome.next,
            mono,
        };
        if alerts.deadlines.get() != next {
            alerts.deadlines.set(next);
            let _ = alerts.rewake.unbounded_send(());
        }
    }

    /// Windows reported that a toast it took failed after all: it is tried again.
    fn toast_failed(&mut self, launch: &str, cx: &mut Context<Self>) {
        let (Some(launch), Some(alerts)) = (Activation::parse(launch), self.alerts.as_mut()) else {
            return;
        };
        let state = self.store.view();
        if let Some(outcome) = alerts.reminders.failed(launch, &state, &Now::system()) {
            self.after_reminders(outcome, cx);
        }
    }

    /// A toast's button, or the toast itself, was clicked.
    fn on_toast(&mut self, arguments: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(activation) = Activation::parse(arguments) else {
            return win::show_and_focus(win::hwnd_of(window));
        };
        let claim = match self.alerts.as_mut() {
            Some(alerts) => alerts.reminders.claim(activation, &state, &Now::system()),
            None => None,
        };
        match (activation, claim) {
            (_, Some(Claim::Open(task))) => {
                win::show_and_focus(win::hwnd_of(window));
                self.reveal_task(task, window, cx);
            }
            (_, Some(Claim::Snoozed(outcome))) => self.after_reminders(outcome, cx),
            (_, Some(Claim::Complete(occurrence, task))) => {
                self.complete_from_toast(occurrence, task, cx);
            }
            // Open from a toast no longer up (or the summary): the window, as it is, nothing selected.
            (Activation::Open(_) | Activation::Missed(_), None) => {
                win::show_and_focus(win::hwnd_of(window));
            }
            // A stale Snooze or Complete (its reminder moved, removed or done, or already used) does
            // nothing.
            (Activation::Snooze(_) | Activation::Complete(_), None) => {}
        }
    }

    /// Completes a task from its reminder with an explicit `CompleteTask` (never a toggle), queued only
    /// if the reminder is still as the toast showed it, checked under the sync client's lock as it is
    /// queued: a sync that moved or removed the reminder, or completed the task, wins. The row plays
    /// the completion only when this queued it. Undo (the toast, Ctrl+Z) brings it back.
    fn complete_from_toast(
        &mut self,
        occurrence: Occurrence,
        task: TaskId,
        cx: &mut Context<Self>,
    ) {
        let visible = self.task_is_visible(task);
        let queued = self.submit_if(Command::CompleteTask { id: task }, |state| {
            still_as_shown(state, occurrence, task)
        });
        if let Some(alerts) = self.alerts.as_mut() {
            alerts.reminders.dismiss(occurrence.id);
        }
        if queued && visible && self.animate {
            let (now, pace) = (self.now(), self.pace());
            let _ = self.motion.complete(task, now, pace);
        }
        self.refresh(cx);
    }

    /// Shows a task in the window: in the room the window shows, if it's there, else in its list
    /// (with no filter hiding it), open in the detail pane.
    fn reveal_task(&mut self, task: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.store.view();
        let Some(list) = (state.tasks.get(&task))
            .filter(|_| state.is_live(task.into()))
            .map(|t| t.list)
        else {
            return;
        };
        if !self.task_is_visible(task) {
            if !self.ui.filter.is_empty() {
                self.ui.filter.clear();
                self.filter.update(cx, |i, cx| i.reset(window, cx));
            }
            self.select_room(RoomKey::List(list), cx);
        }
        self.select_task(Some(task), cx);
        window.focus(&self.list_focus, cx);
    }

    /// The clock was changed, or the computer woke up: look at the reminders again now.
    pub fn clock_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }
}

/// The one wake loop: sleeps until the next deadline ([`wake_in`]), and plans again once one has
/// passed. A new deadline (`rewake`) starts the wait again.
async fn wake_loop(
    this: WeakEntity<ShellView>,
    deadlines: Rc<Cell<Deadlines>>,
    mut rewake: UnboundedReceiver<()>,
    cx: &mut AsyncApp,
) {
    loop {
        let d = deadlines.get();
        let Some(wait) = wake_in(d.wall, d.mono, Timestamp::now(), Instant::now()) else {
            // Nothing to wait for until a deadline is set.
            if rewake.next().await.is_none() {
                return;
            }
            continue;
        };
        let timer = pin!(cx.background_executor().timer(wait));
        match select(timer, rewake.next()).await {
            Either::Left(_) => {}
            Either::Right((None, _)) => return,
            Either::Right((Some(()), _)) => continue,
        }
        if !deadlines.get().due(Timestamp::now(), Instant::now()) {
            continue;
        }
        deadlines.set(Deadlines::default());
        let alive = this.update(cx, |this, cx| this.reconcile_reminders(cx));
        if alive.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::store::Store;
    use todo_client::intent::reminder::Seen;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn the_gate_waits_for_the_backlog_and_falls_back_only_after_a_failed_attempt() {
        let s = Duration::from_secs;
        // Caught up: open at once.
        assert!(gate(Readiness::CaughtUp, s(0)).open);
        // Still starting (connecting, reading the backlog): closed, however long, until it's in.
        let starting = gate(Readiness::Starting, s(600));
        assert!(!starting.open);
        assert_eq!(starting.recheck_in, None);
        // The first attempt failed: closed for the rest of the minute, then from the cache.
        let failed = gate(Readiness::Failed, s(15));
        assert_eq!(
            failed,
            Gate {
                open: false,
                recheck_in: Some(s(45))
            }
        );
        assert!(gate(Readiness::Failed, s(60)).open);
    }

    #[test]
    fn the_wake_never_spins_on_a_deadline_the_clock_has_passed() {
        let now = Instant::now();
        let wall_now = ts("2026-11-21T10:00:00Z");
        // A reminder in ten minutes: woken within a minute (and again), never later.
        assert_eq!(
            wake_in(Some(ts("2026-11-21T10:10:00Z")), None, wall_now, now),
            Some(WAKE_CHUNK)
        );
        assert_eq!(
            wake_in(Some(ts("2026-11-21T10:00:01Z")), None, wall_now, now),
            Some(Duration::from_millis(1050))
        );
        // The clock jumped forward past the reminder: due now, but the loop still yields.
        assert_eq!(
            wake_in(Some(ts("2026-11-21T09:00:00Z")), None, wall_now, now),
            Some(MIN_WAKE)
        );
        // The startup hold is monotonic: a wall-clock jump doesn't touch it.
        let hold = now + Duration::from_secs(30);
        for wall in [
            wall_now,
            ts("2026-11-22T10:00:00Z"),
            ts("2026-11-20T10:00:00Z"),
        ] {
            assert_eq!(
                wake_in(None, Some(hold), wall, now),
                Some(Duration::from_millis(30_050))
            );
        }
        assert_eq!(wake_in(None, Some(now), wall_now, now), Some(MIN_WAKE));
        assert_eq!(wake_in(None, None, wall_now, now), None);
        // Due once either has passed.
        let d = Deadlines {
            wall: Some(ts("2026-11-21T10:10:00Z")),
            mono: Some(hold),
        };
        assert!(!d.due(wall_now, now));
        assert!(d.due(ts("2026-11-21T10:10:00Z"), now));
        assert!(d.due(wall_now, hold));
    }

    #[test]
    fn a_toasts_complete_is_refused_if_the_reminder_changed_before_it_is_queued() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected");
        let seen = Seen::of(&f.state, task, &f.now.eval()).expect("the task has a reminder");
        let occurrence = Occurrence {
            id: seen.id,
            at: seen.at.as_millisecond(),
            token: 1,
        };
        let complete = || Command::CompleteTask { id: task };
        let guard = |s: &todo_core::State| still_as_shown(s, occurrence, task);
        // Rescheduled between the claim and the queue: nothing is completed.
        let moved = Store::fixture(&f);
        moved
            .submit(Command::UpdateReminder {
                id: seen.id,
                at: ts("2026-11-22T09:00:00Z"),
            })
            .expect("moved");
        assert!(
            moved
                .submit_undoable_if(complete(), guard)
                .expect("valid")
                .is_none()
        );
        assert!(!moved.view().tasks[&task].is_done());
        // Removed meanwhile: nothing either.
        let removed = Store::fixture(&f);
        removed
            .submit(Command::Delete { id: seen.id.into() })
            .expect("removed");
        assert!(
            removed
                .submit_undoable_if(complete(), guard)
                .expect("valid")
                .is_none()
        );
        assert!(!removed.view().tasks[&task].is_done());
        // Completed elsewhere: never reopened, never completed again.
        let done = Store::fixture(&f);
        done.submit(complete()).expect("completed");
        let at = done.view().tasks[&task].completed_at;
        assert!(
            done.submit_undoable_if(complete(), guard)
                .expect("valid")
                .is_none()
        );
        assert_eq!(done.view().tasks[&task].completed_at, at);
        // As shown: completed, and Undo reopens it.
        let store = Store::fixture(&f);
        let target = store
            .submit_undoable_if(complete(), guard)
            .expect("valid")
            .expect("queued");
        assert!(store.view().tasks[&task].is_done());
        store.undo(&target).expect("undone");
        assert!(!store.view().tasks[&task].is_done());
    }
}
