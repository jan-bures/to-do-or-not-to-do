//! Reminder settings and in-tab fallback. The service worker shares our occurrence claims.

use super::{Ctx, ctx, icon};
use crate::model::RoomId;
use crate::platform;
use crate::reminder_alerts::{Background, Permission, timeout_ms};
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
    fn notify(title: &str, task: &str, key: &str, open: &Function) -> bool;
    #[wasm_bindgen(js_name = onReminderOpen)]
    fn on_reminder_open(open: &Function);
    #[wasm_bindgen(js_name = claimReminder)]
    fn claim(key: &str, at: f64, cutoff: f64, eligible: &Function, legacy: &str) -> Promise;
    #[wasm_bindgen(js_name = webPushState)]
    fn web_push_state() -> Promise;
    #[wasm_bindgen(js_name = enableWebPush)]
    fn enable_web_push() -> Promise;
    #[wasm_bindgen(js_name = disableWebPush)]
    fn disable_web_push() -> Promise;
    #[wasm_bindgen(js_name = testWebPush)]
    fn test_web_push() -> Promise;
    #[wasm_bindgen(js_name = unsubscribeForSignOut)]
    pub(super) fn unsubscribe_for_sign_out() -> Promise;
}

#[derive(Clone, Copy)]
pub(super) struct Alerts {
    permission: RwSignal<Permission>,
    background: RwSignal<Background>,
    checking: RwSignal<bool>,
    working: RwSignal<bool>,
    feedback: RwSignal<&'static str>,
    queue: RwSignal<Vec<reminder::Alarm>>,
}

impl Alerts {
    pub(super) fn new() -> Self {
        Self {
            permission: RwSignal::new(Self::read_permission()),
            background: RwSignal::new(Background::Unavailable),
            checking: RwSignal::new(false),
            working: RwSignal::new(false),
            feedback: RwSignal::new(""),
            queue: RwSignal::new(Vec::new()),
        }
    }

    fn read_permission() -> Permission {
        Permission::from_browser(&permission())
    }

    pub(super) fn refresh_permission(self) {
        self.permission.set(Self::read_permission());
    }

    pub(super) fn refresh_background(self) {
        if self.checking.get_untracked() || self.working.get_untracked() {
            return;
        }
        self.checking.set(true);
        leptos::task::spawn_local(async move {
            self.background.set(read_background().await);
            self.checking.set(false);
        });
    }

    fn background_action(self, action: BackgroundAction) {
        if self.checking.get_untracked() || self.working.get_untracked() {
            return;
        }
        // Subscribe is called in the event handler, before handing off to the async task.
        let request = match action {
            BackgroundAction::Enable => enable_web_push(),
            BackgroundAction::Disable => disable_web_push(),
            BackgroundAction::Test => test_web_push(),
        };
        self.working.set(true);
        self.feedback.set("");
        leptos::task::spawn_local(async move {
            let result = JsFuture::from(request)
                .await
                .ok()
                .and_then(|v| v.as_string());
            self.feedback.set(match (action, result.as_deref()) {
                (BackgroundAction::Test, Some("ok")) => "Test sent.",
                (BackgroundAction::Test, Some("wait")) => {
                    "Wait 30 seconds before sending another test."
                }
                (BackgroundAction::Test, Some("expired")) => {
                    "Subscription expired. Turn background reminders on again."
                }
                (BackgroundAction::Test, _) => "Couldn’t send the test notification.",
                (_, Some("ok" | "blocked")) => "",
                (BackgroundAction::Enable, _) => "Couldn’t turn on background reminders.",
                (BackgroundAction::Disable, _) => "Couldn’t turn off background reminders.",
            });
            let next = read_background().await;
            if matches!(action, BackgroundAction::Test) && next != Background::On {
                platform::focus("reminder-notification-setting");
            }
            self.background.set(next);
            self.refresh_permission();
            self.working.set(false);
        });
    }

    pub(super) fn first(self) -> Option<reminder::Alarm> {
        self.queue.with(|q| q.first().copied())
    }

    pub(super) fn dismiss(self, alarm: reminder::Alarm) {
        self.queue.update(|q| q.retain(|a| *a != alarm));
    }
}

#[derive(Clone, Copy)]
enum BackgroundAction {
    Enable,
    Disable,
    Test,
}

async fn read_background() -> Background {
    JsFuture::from(web_push_state())
        .await
        .ok()
        .and_then(|v| v.as_string())
        .as_deref()
        .map_or(Background::Unavailable, Background::from_browser)
}

pub(super) fn running(c: Ctx) -> bool {
    c.sync
        .with(|s| s.reminders_ready && !s.signed_out && !s.read_only)
}

fn can_deliver(background: Background) -> bool {
    document().visibility_state() == web_sys::VisibilityState::Visible
        || (background != Background::On && Alerts::read_permission() == Permission::Granted)
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
    let resume = move || {
        wake();
        c.reminder_alerts.refresh_background();
    };
    c.reminder_alerts.refresh_background();
    platform::on_wake(resume);
    let _focus = window_event_listener(leptos::ev::focus, move |_| resume());
    let busy = StoredValue::new(BTreeSet::new());
    let delivered = StoredValue::new(BTreeSet::new());
    // Share one router handler between in-tab notifications and service worker clicks.
    let open = Closure::<dyn Fn(String)>::new(move |id: String| {
        if let Ok(id) = id.parse::<TaskId>() {
            open_task(c, id);
        }
    });
    let open = open.into_js_value().unchecked_into::<Function>();
    on_reminder_open(&open);
    let open = StoredValue::new_local(open);

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
        let background = c.reminder_alerts.background.get();
        let (due, mut next) = c.state.with(|state| {
            c.reminder_alerts
                .queue
                .update(|q| q.retain(|alarm| alarm.is_current(state)));
            delivered.update_value(|keys| keys.retain(|key: &reminder::AlertKey| key.at >= cutoff));
            delivered.with_value(|keys| reminder::alert_plan(state, now, |key| keys.contains(&key)))
        });
        let deliver = can_deliver(background);
        for alarm in due.into_iter().filter(|_| deliver) {
            let fallback = background.fallback_at(alarm.at);
            if fallback > now {
                next = Some(next.map_or(fallback, |next| next.min(fallback)));
                continue;
            }
            let key = alarm.key();
            if busy.try_update_value(|keys| keys.insert(key)) == Some(true) {
                leptos::task::spawn_local(async move {
                    let check = Closure::<dyn Fn() -> bool>::new(move || {
                        let background = c.reminder_alerts.background.get_untracked();
                        eligible(c, alarm)
                            && can_deliver(background)
                            && background.fallback_at(alarm.at) <= platform::now().at
                    });
                    let occurrence = format!("{}@{}", alarm.task, alarm.at.as_millisecond());
                    let claim = JsFuture::from(claim(
                        &occurrence,
                        alarm.at.as_millisecond() as f64,
                        cutoff.as_millisecond() as f64,
                        check.as_ref().unchecked_ref(),
                        &format!("{}@{}", key.reminder, key.at),
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
                                if c.reminder_alerts.background.get_untracked() == Background::On
                                    || !open.with_value(|open| {
                                        notify(&title, &alarm.task.to_string(), &occurrence, open)
                                    })
                                {
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
        if let Some(next) = next {
            timer.set_value(
                set_timeout_with_handle(wake, Duration::from_millis(timeout_ms(next, now))).ok(),
            );
        }
    });
}

fn open_task(c: Ctx, task: TaskId) {
    let list = c.state.with_untracked(|s| {
        s.is_live(task.into())
            .then(|| s.tasks.get(&task).map(|t| t.list))
            .flatten()
    });
    c.navigate(list.map_or(RoomId::Smart("all"), RoomId::List), Some(task));
    request_animation_frame(|| {
        platform::focus("task-detail");
    });
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
    let can_ask = move || {
        alerts.background.get() == Background::Unavailable
            && alerts.permission.get() == Permission::NotAsked
    };
    let busy = move || alerts.checking.get() || alerts.working.get();
    let active = move || alerts.background.get() == Background::On;
    view! {
        <div class="settings-reminders" role="none">
            <button type="button" id="reminder-notification-setting" role="menuitem" tabindex="-1"
                aria-describedby="reminder-notification-description"
                aria-disabled=move || (busy() || !(can_ask() || matches!(alerts.background.get(), Background::Off | Background::On))).to_string()
                on:click=move |ev| {
                    if !ev.is_trusted() || untrack(busy) { return; }
                    match alerts.background.get_untracked() {
                        Background::Off => alerts.background_action(BackgroundAction::Enable),
                        Background::On => alerts.background_action(BackgroundAction::Disable),
                        Background::Unavailable if untrack(can_ask) => {
                            let request = request_permission();
                            leptos::task::spawn_local(async move {
                                let _ = JsFuture::from(request).await;
                                alerts.refresh_permission();
                            });
                        }
                        _ => {}
                    }
                }>
                {icon(Icon::Bell)}
                <span>{move || match alerts.background.get() {
                    Background::Off => "Turn on background reminders",
                    Background::On => "Turn off background reminders",
                    Background::Blocked => "Background reminders",
                    Background::Unavailable if can_ask() => "Enable reminder notifications",
                    Background::Unavailable => "Reminder notifications",
                }}</span>
            </button>
            <p id="reminder-notification-description" role="status">
                {move || alerts.background.get().description(alerts.permission.get())}
            </p>
            <Show when=active>
                <button type="button" id="reminder-test-setting" role="menuitem" tabindex="-1"
                    aria-disabled=move || busy().to_string()
                    on:click=move |ev| {
                        if ev.is_trusted() { alerts.background_action(BackgroundAction::Test); }
                    }>{icon(Icon::BellRing)}"Send test notification"</button>
            </Show>
            <Show when=move || !alerts.feedback.get().is_empty()>
                <p role="status">{move || alerts.feedback.get()}</p>
            </Show>
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
