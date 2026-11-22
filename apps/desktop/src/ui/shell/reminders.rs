//! Reminder toasts while the app runs (also hidden in the tray): the shell keeps them in line with the
//! synced reminders (`model::reminders` decides, `platform::toast` shows them) every time the data
//! changes, and one wake timer brings it back when the next one is due. The toasts' buttons come back
//! here: Open shows the window with the task, Snooze 10 min puts the reminder off on this computer
//! only, and Complete completes the task (Undo works as for any completion). A button acts only on the
//! reminder as its toast showed it, and Snooze and Complete only once (`Reminders::claim`).

use std::cell::Cell;
use std::pin::pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::future::{Either, select};
use gpui::{AsyncApp, AsyncWindowContext, Context, Task, WeakEntity, Window};
use todo_client::Connection;
use todo_core::ids::TaskId;
use todo_core::time::Timestamp;
use todo_core::{Command, State};

use super::ShellView;
use crate::config::DataDir;
use crate::model::reminders::{Activation, Claim, Outcome, Reminders};
use crate::model::{Now, RoomKey};
use crate::platform::{toast, win};

/// How long a reminder that is due may be held back after starting, until the first sync has caught
/// up (`Connection::Online`): what changed elsewhere meanwhile (a task completed on the phone) comes
/// in first. Offline, or before the first attempt, it waits this long and then shows from the cache.
/// Well inside the grace period, so the reminder still shows as itself.
const CATCH_UP: Duration = Duration::from_secs(60);
/// The longest one wake timer runs: a timer left behind by a new deadline ends within this, and a
/// wake that slept through a suspended computer is at most this late.
const WAKE_CHUNK: Duration = Duration::from_secs(60);

/// The live session's reminders: what was shown, the toasts, and the wake timer.
#[allow(missing_debug_implementations)] // Holds the sink and GPUI tasks.
pub(super) struct Alerts {
    reminders: Reminders,
    data: DataDir,
    started: Instant,
    started_at: Timestamp,
    /// The schedule changed and hasn't been saved yet (a failed save is tried again on every pass).
    dirty: bool,
    /// When the wake loop should look again, and how to tell it the time changed.
    deadline: Rc<Cell<Option<Timestamp>>>,
    rewake: UnboundedSender<()>,
    _wake: Task<()>,
    _activations: Task<()>,
}

/// What completing a task from its reminder toast submits: always `CompleteTask`, never a toggle, so a
/// task completed elsewhere meanwhile is never reopened (completing a completed task changes nothing).
/// `None` when it is gone or already completed in `state`.
pub(super) fn toast_completion(state: &State, task: TaskId) -> Option<Command> {
    let open = (state.tasks.get(&task)).is_some_and(|t| !t.is_done() && t.merged_into.is_none());
    (open && state.is_live(task.into())).then_some(Command::CompleteTask { id: task })
}

impl ShellView {
    /// Starts the reminder toasts for a live session (fixtures have none).
    pub fn start_reminders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.store.session() else {
            return;
        };
        let data = session.data().clone();
        let (tx, mut rx) = unbounded::<String>();
        let sink = toast::WinToasts::new(tx);
        let activations = cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                while let Some(arguments) = rx.next().await {
                    let handled = this.update_in(cx, |this, window, cx| {
                        this.on_toast(&arguments, window, cx);
                    });
                    if handled.is_err() {
                        break;
                    }
                }
            },
        );
        let deadline = Rc::new(Cell::new(None));
        let (rewake, wakes) = unbounded::<()>();
        let wake = cx.spawn({
            let deadline = deadline.clone();
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                wake_loop(this, deadline, wakes, cx).await;
            }
        });
        self.alerts = Some(Alerts {
            reminders: Reminders::new(data.load_reminders(), Box::new(sink)),
            data,
            started: Instant::now(),
            started_at: Timestamp::now(),
            dirty: false,
            deadline,
            rewake,
            _wake: wake,
            _activations: activations,
        });
        self.reconcile_reminders(cx);
    }

    /// Stops them (quitting, signing out): the toasts go, since nothing would answer their buttons.
    pub fn stop_reminders(&mut self) {
        if let Some(mut alerts) = self.alerts.take() {
            alerts.reminders.clear();
        }
    }

    /// Brings the toasts in line with the data now, and sets the wake for the next reminder.
    pub(super) fn reconcile_reminders(&mut self, cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        let state = self.store.view();
        let caught_up = self.store.sync().connection == Connection::Online;
        let can_show = caught_up || alerts.started.elapsed() >= CATCH_UP;
        let outcome = alerts.reminders.reconcile(&state, &Now::system(), can_show);
        self.after_reminders(outcome, !can_show, cx);
    }

    /// Saves what changed (until it saves) and sets the wake: when the next reminder is due, or, while
    /// one is held for the first catch-up, when that hold ends.
    fn after_reminders(&mut self, outcome: Outcome, gated: bool, _cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        alerts.dirty |= outcome.changed;
        if alerts.dirty {
            match alerts.data.save_reminders(alerts.reminders.schedule()) {
                Ok(()) => alerts.dirty = false,
                Err(e) => tracing::warn!("saving the reminders' schedule failed: {e:#}"),
            }
        }
        let hold_ends = (outcome.held && gated).then(|| {
            let ms = alerts.started_at.as_millisecond()
                + i64::try_from(CATCH_UP.as_millis()).unwrap_or(i64::MAX);
            Timestamp::from_millisecond(ms).unwrap_or(alerts.started_at)
        });
        let next = match (outcome.next, hold_ends) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if alerts.deadline.get() != next {
            alerts.deadline.set(next);
            let _ = alerts.rewake.unbounded_send(());
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
            (_, Some(Claim::Snoozed(outcome))) => self.after_reminders(outcome, false, cx),
            (_, Some(Claim::Complete(task))) => self.complete_from_toast(task, cx),
            // Open on a reminder that changed, or the summary: the window, as it is.
            (Activation::Open(_) | Activation::Missed, None) => {
                win::show_and_focus(win::hwnd_of(window));
            }
            // A stale Snooze or Complete (its reminder moved, removed or done, or already used) does
            // nothing.
            (Activation::Snooze(_) | Activation::Complete(_), None) => {}
        }
    }

    /// Completes a task from its reminder, with an explicit `CompleteTask` (never a toggle, which a
    /// completion synced in meanwhile would turn into a reopening). The row plays the completion only
    /// when this click is what completed it. Undo (the toast, Ctrl+Z) brings it back.
    fn complete_from_toast(&mut self, task: TaskId, cx: &mut Context<Self>) {
        let Some(cmd) = toast_completion(&self.store.view(), task) else {
            return self.refresh(cx);
        };
        let visible = self.task_is_visible(task);
        if self.submit(vec![cmd]) {
            let completed = (self.store.view().tasks.get(&task)).is_some_and(|t| t.is_done());
            if completed && visible && self.animate {
                let (now, pace) = (self.now(), self.pace());
                let _ = self.motion.complete(task, now, pace);
            }
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

/// The one wake timer: sleeps until the deadline (in chunks of at most [`WAKE_CHUNK`], so a timer
/// dropped for a new deadline never lingers long), and plans again once it has passed. A new deadline
/// (`rewake`) starts the wait again.
async fn wake_loop(
    this: WeakEntity<ShellView>,
    deadline: Rc<Cell<Option<Timestamp>>>,
    mut rewake: futures::channel::mpsc::UnboundedReceiver<()>,
    cx: &mut AsyncApp,
) {
    loop {
        let Some(at) = deadline.get() else {
            // Nothing to wait for until a deadline is set.
            if rewake.next().await.is_none() {
                return;
            }
            continue;
        };
        let left = at.as_millisecond() - Timestamp::now().as_millisecond();
        if left > 0 {
            // A moment after it's due, so the pass finds it due.
            let wait = Duration::from_millis(u64::try_from(left).unwrap_or(0) + 50).min(WAKE_CHUNK);
            let timer = pin!(cx.background_executor().timer(wait));
            match select(timer, rewake.next()).await {
                Either::Left(_) => continue,
                Either::Right((None, _)) => return,
                Either::Right((Some(()), _)) => continue,
            }
        }
        deadline.set(None);
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

    #[test]
    fn completing_from_a_toast_never_reopens_a_task_completed_meanwhile() {
        let f = fixtures::my_day();
        let task = f.ui.task.expect("selected");
        let store = Store::fixture(&f);
        // Checked: open, so the toast's Complete goes ahead.
        let cmd = toast_completion(&store.view(), task).expect("open");
        assert_eq!(cmd, Command::CompleteTask { id: task });
        // A sync completes it before the command is submitted.
        store
            .submit(Command::CompleteTask { id: task })
            .expect("completed elsewhere");
        let done_at = store.view().tasks[&task].completed_at;
        // Submitting what was checked leaves it completed, as it was.
        store.submit_undoable(cmd).expect("valid");
        let after = store.view();
        assert!(after.tasks[&task].is_done());
        assert_eq!(after.tasks[&task].completed_at, done_at);
        // And once completed, there is nothing to submit at all.
        assert_eq!(toast_completion(&after, task), None);
        // A deleted task neither.
        let mut gone = (*after).clone();
        gone.tasks.get_mut(&task).expect("task").completed_at = None;
        let deleted = Store::fixture(&f);
        deleted
            .submit(Command::Delete { id: task.into() })
            .expect("deleted");
        assert_eq!(toast_completion(&deleted.view(), task), None);
        assert!(toast_completion(&gone, task).is_some());
    }
}
