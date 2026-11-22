//! Reminder toasts while the app runs (also hidden in the tray): the shell keeps them in line with the
//! synced reminders (`model::reminders` decides, `platform::toast` shows them) every time the data
//! changes, and wakes itself when the next one is due. The toasts' buttons come back here: Open shows
//! the window with the task, Snooze 10 min puts the reminder off on this computer only, and Complete
//! completes the task (Undo works as for any completion).

use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::unbounded;
use gpui::{AsyncWindowContext, Context, Task, WeakEntity, Window};
use todo_client::Connection;
use todo_core::Command;
use todo_core::ids::TaskId;
use todo_core::time::Timestamp;

use super::ShellView;
use crate::config::DataDir;
use crate::model::reminders::{Activation, Outcome, Reminders};
use crate::model::{Now, RoomKey};
use crate::platform::{toast, win};

/// How long the first catch-up after starting may hold back a reminder that is due: what changed
/// elsewhere meanwhile (a task completed on the phone) comes in first. Well inside the grace period, so
/// the reminder still shows as itself.
const CATCH_UP: Duration = Duration::from_secs(60);

/// The live session's reminders: what was shown, the toasts, and the timer for the next one.
#[allow(missing_debug_implementations)] // Holds the sink and GPUI tasks.
pub(super) struct Alerts {
    reminders: Reminders,
    data: DataDir,
    started: Instant,
    /// When the timer wakes the shell next, and the timer (dropping it cancels it).
    wake: Option<(Timestamp, Task<()>)>,
    _activations: Task<()>,
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
        self.alerts = Some(Alerts {
            reminders: Reminders::new(data.load_reminders(), Box::new(sink)),
            data,
            started: Instant::now(),
            wake: None,
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

    /// Brings the toasts in line with the data now, and sets the timer for the next reminder.
    pub(super) fn reconcile_reminders(&mut self, cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        let state = self.store.view();
        let can_show = alerts.started.elapsed() >= CATCH_UP
            || self.store.sync().connection != Connection::Syncing;
        let outcome = alerts.reminders.reconcile(&state, &Now::system(), can_show);
        self.after_reminders(outcome, cx);
    }

    /// Saves what changed and sets the timer for when the next reminder is due.
    fn after_reminders(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        let Some(alerts) = self.alerts.as_mut() else {
            return;
        };
        if outcome.changed
            && let Err(e) = alerts.data.save_reminders(alerts.reminders.schedule())
        {
            tracing::warn!("saving the reminders' schedule failed: {e:#}");
        }
        let Some(next) = outcome.next else {
            alerts.wake = None;
            return;
        };
        if alerts.wake.as_ref().is_some_and(|(at, _)| *at == next) {
            return;
        }
        let ms = next.as_millisecond() - Timestamp::now().as_millisecond();
        // A moment after it's due, so the pass finds it due. (The shell also refreshes every half
        // minute, which covers a timer that slept through a suspended computer.)
        let wait = Duration::from_millis(u64::try_from(ms.max(0)).unwrap_or(0) + 50);
        let timer = cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(alerts) = this.alerts.as_mut() {
                        alerts.wake = None;
                    }
                    this.reconcile_reminders(cx);
                });
            },
        );
        alerts.wake = Some((next, timer));
    }

    /// A toast's button, or the toast itself, was clicked.
    fn on_toast(&mut self, arguments: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.store.view();
        let task_of = |id| state.reminders.get(&id).map(|r| r.task);
        match Activation::parse(arguments) {
            Some(Activation::Open(id)) => {
                win::show_and_focus(win::hwnd_of(window));
                if let Some(task) = task_of(id) {
                    self.reveal_task(task, window, cx);
                }
            }
            Some(Activation::Snooze(id)) => {
                let Some(alerts) = self.alerts.as_mut() else {
                    return;
                };
                let outcome = alerts.reminders.snooze(id, &state, &Now::system());
                self.after_reminders(outcome, cx);
            }
            Some(Activation::Complete(id)) => {
                if let Some(alerts) = self.alerts.as_mut() {
                    alerts.reminders.dismiss(id);
                }
                if let Some(task) = task_of(id) {
                    self.complete_from_toast(task, cx);
                }
            }
            Some(Activation::Missed) | None => win::show_and_focus(win::hwnd_of(window)),
        }
    }

    /// Completes a task from its reminder: with the row's choreography where the room shows it, else
    /// straight away. Either way Undo (the toast, Ctrl+Z) brings it back.
    fn complete_from_toast(&mut self, task: TaskId, cx: &mut Context<Self>) {
        let state = self.store.view();
        let open =
            (state.tasks.get(&task)).is_some_and(|t| !t.is_done() && state.is_live(task.into()));
        if !open {
            return;
        }
        if self.task_is_visible(task) {
            self.toggle_complete(task, cx);
        } else {
            self.submit(vec![Command::CompleteTask { id: task }]);
            self.refresh(cx);
        }
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
}
