//! Reminder toasts while the app runs (also hidden in the tray): the shell keeps them in line with the
//! synced reminders (`model::reminders` decides, `platform::toast` shows them) every time the data
//! changes, and one wake loop brings it back when the next one is due. The toasts' buttons come back
//! here: Open shows the window with the task, Snooze 10 min puts the reminder off on this computer
//! only, and Complete completes the task (Undo works as for any completion). A button acts only on the
//! reminder as its toast showed it, and Snooze and Complete only once (`Reminders::claim`).

use std::cell::Cell;
use std::collections::VecDeque;
use std::pin::pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::{Either, select};
use gpui::{AsyncApp, AsyncWindowContext, Context, Task, WeakEntity, Window};
use todo_client::Readiness;
use todo_client::intent::reminder::Alarm;
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

/// The longest one wake timer runs: a timer left behind by a new deadline ends within this, and a
/// wake that slept through a suspended computer is at most this late.
const WAKE_CHUNK: Duration = Duration::from_secs(60);
/// The shortest wait between two passes the wake loop starts: whatever the clocks say, it yields.
const MIN_WAKE: Duration = Duration::from_millis(250);

/// When the wake loop should look next: the next reminder (wall clock: alarms are instants), or a
/// monotonic deadline (a save to try again or a queued click whose time is up), whichever comes first. Never longer
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

/// A mutating toast click waits briefly for catch-up, then expires rather than acting much later.
const ACTION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
struct ToastActions(VecDeque<(Activation, Instant)>);

impl ToastActions {
    fn push(&mut self, action: Activation, now: Instant) {
        self.expire(now);
        // The first mutating click on a token wins. Repeated clicks do not prolong its lifetime.
        if !self
            .0
            .iter()
            .any(|(queued, _)| queued.token() == action.token())
        {
            self.0.push_back((action, now + ACTION_TIMEOUT));
        }
    }

    fn expire(&mut self, now: Instant) {
        while self.deadline().is_some_and(|deadline| deadline <= now) {
            self.0.pop_front();
        }
    }

    fn take_ready(&mut self, readiness: Readiness, now: Instant) -> Option<Activation> {
        self.expire(now);
        if readiness != Readiness::CaughtUp {
            return None;
        }
        self.0.pop_front().map(|(action, _)| action)
    }

    fn deadline(&self) -> Option<Instant> {
        self.0.front().map(|(_, deadline)| *deadline)
    }
}

/// The live session's reminders: what was shown, the toasts, and the wake loop.
#[allow(missing_debug_implementations)] // Holds the sink and GPUI tasks.
pub(super) struct Alerts {
    reminders: Reminders,
    data: DataDir,
    /// The schedule changed and hasn't been saved yet, and when a failed save is tried again (its
    /// own back-off: a refresh, a keystroke in the filter, never writes before then).
    dirty: bool,
    save_retry: Option<Instant>,
    save_failures: usize,
    actions: ToastActions,
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
            dirty: false,
            save_retry: None,
            save_failures: 0,
            actions: ToastActions::default(),
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
        let outcome =
            alerts
                .reminders
                .reconcile(&state, &Now::system(), readiness == Readiness::CaughtUp);
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
        let next = Deadlines {
            wall: outcome.next,
            mono: alerts
                .save_retry
                .filter(|_| alerts.dirty)
                .into_iter()
                .chain(alerts.actions.deadline())
                .min(),
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
        if alerts.reminders.failed(launch, &Now::system()) {
            alerts.dirty = true;
            self.reconcile_reminders(cx);
        }
    }

    /// A toast's button, or the toast itself, was clicked.
    fn on_toast(&mut self, arguments: &str, window: &mut Window, cx: &mut Context<Self>) {
        let activation = Activation::parse(arguments);
        if let Some(action @ (Activation::Snooze(_) | Activation::Complete(_))) = activation {
            if let Some(alerts) = self.alerts.as_mut() {
                alerts.actions.push(action, Instant::now());
                self.refresh(cx);
            }
            return;
        }
        // Open always raises the window, but cannot select a task from a partial replica.
        win::show_and_focus(win::hwnd_of(window));
        let (state, readiness) = self.store.view_and_readiness();
        if let Some(action @ Activation::Open(_)) = activation
            && readiness == Readiness::CaughtUp
            && let Some(alerts) = self.alerts.as_mut()
            && let Some(Claim::Open(task)) = alerts.reminders.claim(action, &state, &Now::system())
        {
            self.reveal_task(task, window, cx);
        }
    }

    /// Apply queued clicks before deriving the shell, validating each against a fresh view. While
    /// catching up the wake loop enforces expiry and the sync change triggers another refresh.
    pub(super) fn drain_reminder_actions(&mut self) {
        let tick = Instant::now();
        while let Some(alerts) = self.alerts.as_mut() {
            if alerts.actions.deadline().is_none() {
                break;
            }
            let (state, readiness) = self.store.view_and_readiness();
            let Some(action) = alerts.actions.take_ready(readiness, tick) else {
                break;
            };
            let now = Now::system();
            match alerts.reminders.claim(action, &state, &now) {
                Some(Claim::Snoozed(outcome)) => alerts.dirty |= outcome.changed,
                Some(Claim::Complete { alarm, occurrence }) => {
                    self.complete_from_toast(alarm, occurrence, &now);
                }
                _ => {} // A stale click has no effect.
            }
        }
    }

    /// Completes a task from its reminder with an explicit `CompleteTask` (never a toggle), queued only
    /// if the reminder is still as the toast showed it, checked under the sync client's lock as it is
    /// queued: a sync that moved or removed the reminder, or completed the task, wins. The row plays
    /// the completion only when this queued it. Undo (the toast, Ctrl+Z) brings it back.
    fn complete_from_toast(&mut self, alarm: Alarm, occurrence: Occurrence, now: &Now) {
        let task = alarm.task;
        let visible = self.task_is_visible(task);
        let queued = self.submit_if(Command::CompleteTask { id: task }, |state| {
            still_as_shown(state, alarm)
        });
        if let Some(alerts) = self.alerts.as_mut() {
            alerts.dirty |= alerts
                .reminders
                .finish_complete(occurrence, queued.is_err(), now);
        }
        if queued == Ok(true) && visible && self.animate {
            let (now, pace) = (self.now(), self.pace());
            let _ = self.motion.complete(task, now, pace);
        }
    }

    /// Shows a task in the window: in the room the window shows, if it's there, else in its list
    /// (with no filter hiding it), open in the detail pane.
    pub(super) fn reveal_task(
        &mut self,
        task: TaskId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.store.view();
        let Some((list, done)) = (state.tasks.get(&task))
            .filter(|_| state.is_live(task.into()))
            .map(|t| (t.list, t.is_done()))
        else {
            return;
        };
        if !self.task_is_visible(task) {
            if !self.ui.filter.is_empty() {
                self.ui.filter.clear();
                self.filter.update(cx, |i, cx| i.reset(window, cx));
            }
            self.select_room(RoomKey::List(list), cx);
            if self.ui.room == RoomKey::List(list) && done && !self.ui.show_completed {
                self.ui.show_completed = true;
                self.refresh(cx);
            }
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
        let alive = this.update(cx, |this, cx| this.refresh(cx));
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
    fn mutating_toast_clicks_wait_for_catchup_only_until_their_original_deadline() {
        let occurrence = Occurrence {
            id: todo_core::ids::ReminderId::from_uuid(uuid::Uuid::nil()),
            at: 0,
            token: 1,
        };
        let now = Instant::now();
        let snooze = Activation::Snooze(occurrence);
        let complete = Activation::Complete(Occurrence {
            token: 2,
            ..occurrence
        });
        let mut queue = ToastActions::default();
        queue.push(snooze, now);
        assert_eq!(queue.take_ready(Readiness::Starting, now), None);
        assert_eq!(queue.take_ready(Readiness::Failed, now), None);
        assert_eq!(queue.deadline(), Some(now + ACTION_TIMEOUT));
        assert_eq!(
            queue.take_ready(Readiness::CaughtUp, now + Duration::from_secs(1)),
            Some(snooze)
        );
        assert_eq!(
            queue.take_ready(Readiness::CaughtUp, now + Duration::from_secs(1)),
            None
        );

        queue.push(complete, now);
        queue.push(
            Activation::Snooze(Occurrence {
                token: 2,
                ..occurrence
            }),
            now + Duration::from_secs(2),
        );
        assert_eq!(
            queue.take_ready(Readiness::Starting, now + Duration::from_secs(2)),
            None
        );
        assert_eq!(queue.deadline(), Some(now + ACTION_TIMEOUT));
        assert_eq!(
            queue.take_ready(Readiness::CaughtUp, now + ACTION_TIMEOUT),
            None
        );
        assert_eq!(queue.deadline(), None);
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
        // Save retries are monotonic: a wall-clock jump doesn't touch them.
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
        let alarm = Alarm {
            reminder: seen.id,
            task,
            at: seen.at,
        };
        let complete = || Command::CompleteTask { id: task };
        let guard = |s: &todo_core::State| still_as_shown(s, alarm);
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
        // An edit within one millisecond must also invalidate the queued completion.
        let fine = Store::fixture(&f);
        let at = Timestamp::new(seen.at.as_second(), seen.at.subsec_nanosecond() + 1)
            .expect("in the same millisecond");
        assert_eq!(seen.at.as_millisecond(), at.as_millisecond());
        fine.submit(Command::UpdateReminder { id: seen.id, at })
            .expect("moved");
        assert!(
            fine.submit_undoable_if(complete(), guard)
                .expect("valid")
                .is_none()
        );
        // Merge reparented the reminder without changing its timestamp.
        let merged = Store::fixture(&f);
        let target = f
            .state
            .tasks
            .values()
            .find(|t| t.id != task && !t.is_done() && f.state.is_live(t.id.into()))
            .expect("another task")
            .id;
        merged
            .submit(Command::MergeTasks {
                source: task,
                target,
            })
            .expect("merged");
        assert_eq!(merged.view().reminders[&seen.id].task, target);
        assert!(
            merged
                .submit_undoable_if(complete(), guard)
                .expect("valid")
                .is_none()
        );
        assert!(!merged.view().tasks[&target].is_done());
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
