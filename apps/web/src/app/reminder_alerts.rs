//! Deliver in this page only: a system notification when possible, otherwise the shared toast view.

use super::{Ctx, ctx, icon};
use crate::model::RoomId;
use crate::platform;
use crate::reminder_alerts::{Permission, timeout_ms};
use js_sys::{Function, Promise};
use leptos::leptos_dom::helpers::{TimeoutHandle, set_timeout_with_handle};
use leptos::prelude::*;
use std::collections::BTreeSet;
use std::time::Duration;
use todo_client::intent::{self, reminder};
use todo_core::ids::TaskId;
use todo_design::Icon;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(raw_module = "./reminder-browser.js")]
extern "C" {
    #[wasm_bindgen(js_name = reminderPermission)]
    fn permission() -> String;
    #[wasm_bindgen(js_name = requestReminderPermission)]
    fn request_permission() -> Promise;
    #[wasm_bindgen(js_name = showReminderNotification)]
    fn notify(title: &str, task: &str, open: &Function) -> bool;
    #[wasm_bindgen(js_name = claimReminder)]
    fn claim(key: &str, at: f64, cutoff: f64, eligible: &Function) -> Promise;
}

#[derive(Clone, Copy)]
pub(super) struct Alerts {
    permission: RwSignal<Permission>,
    queue: RwSignal<Vec<reminder::Alarm>>,
}

impl Alerts {
    pub(super) fn new() -> Self {
        Self {
            permission: RwSignal::new(Self::read_permission()),
            queue: RwSignal::new(Vec::new()),
        }
    }

    fn read_permission() -> Permission {
        Permission::from_browser(&permission())
    }

    pub(super) fn refresh_permission(self) {
        self.permission.set(Self::read_permission());
    }

    pub(super) fn first(self) -> Option<reminder::Alarm> {
        self.queue.with(|q| q.first().copied())
    }

    pub(super) fn dismiss(self, alarm: reminder::Alarm) {
        self.queue.update(|q| q.retain(|a| *a != alarm));
    }
}

pub(super) fn running(c: Ctx) -> bool {
    c.sync
        .with(|s| s.reminders_ready && !s.signed_out && !s.read_only)
}

fn can_deliver() -> bool {
    Alerts::read_permission() == Permission::Granted
        || document().visibility_state() == web_sys::VisibilityState::Visible
}

fn eligible(c: Ctx, alarm: reminder::Alarm) -> bool {
    let now = platform::now().at;
    untrack(|| running(c))
        && alarm.at <= now
        && alarm.at >= reminder::alert_cutoff(now)
        && c.state.with_untracked(|s| alarm.is_current(s))
}

pub(super) fn start(c: Ctx) {
    let timer = StoredValue::new_local(None::<TimeoutHandle>);
    let tick = RwSignal::new(0_u64);
    let wake = move || {
        c.reminder_alerts.refresh_permission();
        tick.update(|t| *t += 1);
    };
    platform::on_wake(wake);
    let _focus = window_event_listener(leptos::ev::focus, move |_| wake());
    let busy = StoredValue::new(BTreeSet::new());
    let delivered = StoredValue::new(BTreeSet::new());
    // A single handler per page. The title is the only value passed to the Notification API.
    let open = Closure::<dyn Fn(String)>::new(move |id: String| {
        if let Ok(id) = id.parse::<TaskId>() {
            open_task(c, id);
        }
    });
    let open = StoredValue::new_local(open.into_js_value().unchecked_into::<Function>());

    Effect::new(move |_| {
        c.now.track(); // catches wall-clock changes, including when a timer was throttled
        tick.track();
        if let Some(old) = timer.try_update_value(Option::take).flatten() {
            old.clear();
        }
        if !running(c) {
            return;
        }
        let now = platform::now().at;
        let cutoff = reminder::alert_cutoff(now);
        let (due, next) = c.state.with(|state| {
            c.reminder_alerts
                .queue
                .update(|q| q.retain(|alarm| alarm.is_current(state)));
            delivered.update_value(|keys| keys.retain(|key: &reminder::AlertKey| key.at >= cutoff));
            delivered.with_value(|keys| reminder::alert_plan(state, now, |key| keys.contains(&key)))
        });
        if let Some(next) = next {
            timer.set_value(
                set_timeout_with_handle(wake, Duration::from_millis(timeout_ms(next, now))).ok(),
            );
        }
        // Leave fallback claims to a visible tab. A background tab can still show a native alert.
        if !can_deliver() {
            return;
        }
        for alarm in due {
            let key = alarm.key();
            if busy.try_update_value(|keys| keys.insert(key)) == Some(true) {
                leptos::task::spawn_local(async move {
                    let check = Closure::<dyn Fn() -> bool>::new(move || {
                        eligible(c, alarm) && can_deliver()
                    });
                    let claim = JsFuture::from(claim(
                        &format!("{}@{}", key.reminder, key.at),
                        alarm.at.as_millisecond() as f64,
                        cutoff.as_millisecond() as f64,
                        check.as_ref().unchecked_ref(),
                    ))
                    .await;
                    busy.update_value(|keys| {
                        keys.remove(&key);
                    });
                    let Ok(claim) = claim else {
                        // The normal clock tick retries. Do not bypass cross-tab coordination.
                        tracing::debug!("could not record reminder delivery");
                        return;
                    };
                    match claim.as_f64() {
                        Some(0.0) => delivered.update_value(|keys| {
                            keys.insert(key);
                        }),
                        Some(1.0) => {
                            delivered.update_value(|keys| {
                                keys.insert(key);
                            });
                            if eligible(c, alarm)
                                && let Some(title) = c.state.with_untracked(|s| {
                                    s.tasks.get(&alarm.task).map(|t| t.title.clone())
                                })
                            {
                                if !open.with_value(|open| {
                                    notify(&title, &alarm.task.to_string(), open)
                                }) {
                                    c.reminder_alerts.queue.update(|q| q.push(alarm));
                                }
                                c.reminder_alerts.refresh_permission();
                            }
                        }
                        _ => {}
                    }
                });
            }
        }
    });
}

fn open_task(c: Ctx, task: TaskId) {
    let list = c.state.with_untracked(|s| {
        s.is_live(task.into())
            .then(|| s.tasks.get(&task).map(|t| t.list))
            .flatten()
    });
    if let Some(list) = list {
        c.go(RoomId::List(list));
        c.select(Some(task));
        request_animation_frame(|| {
            platform::focus("task-detail");
        });
    }
}

fn finish(c: Ctx, alarm: reminder::Alarm, snooze: bool) {
    if !untrack(|| running(c)) {
        return;
    }
    if c.detail_modal.get_untracked() {
        platform::focus("task-detail");
    }
    c.reminder_alerts.dismiss(alarm);
    let action = c.state.with_untracked(|s| {
        if !alarm.is_current(s) {
            return None;
        }
        if snooze {
            let cmd = reminder::snooze_command(s, alarm, platform::now().at)?;
            Some((cmd, "Reminder snoozed for 10 min".into()))
        } else {
            let cmd = intent::set_done(s, alarm.task, true)?;
            let title = &s.tasks.get(&alarm.task)?.title;
            Some((cmd, format!("Completed “{title}”")))
        }
    });
    if let Some((command, label)) = action {
        c.submit_undoable(command, label, None);
    }
}

#[component]
pub(super) fn Setting() -> impl IntoView {
    let alerts = ctx().reminder_alerts;
    let can_ask = move || alerts.permission.get() == Permission::NotAsked;
    view! {
        <div class="settings-reminders" role="none">
            <button type="button" id="reminder-notification-setting" role="menuitem" tabindex="-1"
                aria-describedby="reminder-notification-description" aria-disabled=move || (!can_ask()).to_string()
                on:click=move |ev| {
                    if !ev.is_trusted() { return; }
                    alerts.refresh_permission();
                    if alerts.permission.get_untracked() == Permission::NotAsked {
                        let request = request_permission();
                        leptos::task::spawn_local(async move {
                            let _ = JsFuture::from(request).await;
                            alerts.refresh_permission();
                        });
                    }
                }>
                {icon(Icon::Bell)}
                {move || if can_ask() { "Enable reminder notifications" } else { "Reminder notifications" }}
            </button>
            <p id="reminder-notification-description" role="status">
                {move || alerts.permission.get().description()}
            </p>
        </div>
    }
}

#[component]
pub(super) fn ReminderToast(alarm: reminder::Alarm, title: String) -> impl IntoView {
    let c = ctx();
    let tooltip = title.clone();
    view! {
        <div class="toast toast-reminder">
            <button type="button" class="toast-reminder-title" title=tooltip on:click=move |_| open_task(c, alarm.task)>
                {icon(Icon::Bell)}<span class="toast-text">{title}</span>
            </button>
            <button type="button" class="toast-undo" on:click=move |_| finish(c, alarm, false)>
                {reminder::COMPLETE_LABEL}
            </button>
            <button type="button" class="toast-undo" on:click=move |_| finish(c, alarm, true)>
                {reminder::SNOOZE_LABEL}
            </button>
            <button type="button" class="toast-close" aria-label="Dismiss" on:click=move |_| {
                if c.detail_modal.get_untracked() { platform::focus("task-detail"); }
                c.reminder_alerts.dismiss(alarm);
            }>{icon(Icon::X)}</button>
        </div>
    }
}
