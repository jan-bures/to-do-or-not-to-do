//! The task detail's reminder picker. The shared intent owns reminder selection and edits.

use super::{
    Ctx, Detail, Now, backend, ctx, icon, model,
    pickers::{self, DetailPicker},
    platform,
};
use crate::reminder_time::{Draft, Preset};
use jiff::Timestamp;
use leptos::prelude::*;
use todo_client::intent::reminder::{self, REMINDER_CHANGED, Seen, Stale};
use todo_core::ids::TaskId;
use todo_design::Icon;

pub(super) const ANCHOR: &str = "edit-task-reminder";
pub(super) const PICKER: &str = "reminder-picker";
const CUSTOM: &str = "reminder-custom";
const DATE: &str = "reminder-date";
const FIRST: &str = "reminder-in-an-hour";

const PRESETS: [(Preset, &str, &str); 3] = [
    (Preset::InAnHour, "In 1 hour", FIRST),
    (
        Preset::AtNineInDays(1),
        "Tomorrow at 09:00",
        "reminder-tomorrow",
    ),
    (
        Preset::AtNineInDays(7),
        "In a week at 09:00",
        "reminder-in-a-week",
    ),
];

fn preset_at(preset: Preset, now: Now, live: bool) -> Option<Timestamp> {
    preset.at(now, |dt| platform::local_instant(dt, now, live))
}

fn time_label(at: Timestamp, now: Now, live: bool) -> String {
    model::local_date_time(platform::civil_time(at, now, live), now.dates())
}

fn display_now(c: Ctx) -> Now {
    let saved = c.now.get_untracked();
    if c.live {
        Now {
            date_format: saved.date_format,
            ..platform::now()
        }
    } else {
        saved
    }
}

#[component]
pub(super) fn ReminderEditor(id: TaskId, detail: Memo<Option<Detail>>) -> impl IntoView {
    let c = ctx();
    let picker = DetailPicker::new(c, id, ANCHOR, PICKER);
    let seen = RwSignal::new(None::<Seen>);
    let custom = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let opened_at = RwSignal::new(c.now.get_untracked());
    let now = move || display_now(c);
    let selected = move |p: Preset| {
        // In 1 hour is an action, not a saved rule; its target changes with the clock.
        matches!(p, Preset::AtNineInDays(_))
            && seen
                .get()
                .is_some_and(|s| preset_at(p, opened_at.get(), c.live) == Some(s.at))
    };
    let change = move |at: Option<Timestamp>| {
        if c.read_only() || c.selected.get_untracked() != Some(id) {
            return;
        }
        let seen = seen.get_untracked();
        let result = c.state.with_untracked(|s| match at {
            Some(at) => reminder::set_reminder(s, id, seen, at, backend::new_uuid),
            None => seen
                .map(|seen| reminder::remove_reminder(s, id, seen))
                .transpose(),
        });
        match result {
            Ok(cmd) => {
                if cmd.is_none_or(|cmd| {
                    let label = at.map_or_else(
                        || "Reminder removed.".into(),
                        |at| reminder::reminder_set_text(&time_label(at, now(), c.live)),
                    );
                    c.submit_undoable(cmd, label, None)
                }) {
                    picker.close(true);
                } else {
                    error.set(c.toast.get_untracked().map(|t| t.text));
                }
            }
            Err(Stale::Task) => picker.close(false),
            Err(Stale::Reminder) => error.set(Some(REMINDER_CHANGED.into())),
        }
    };
    Effect::new(move |_| {
        error.track();
        if picker.open.get() {
            request_animation_frame(move || picker.position());
        }
    });
    view! {
        <div class="prop">
            <dt>{icon(Icon::Bell)}"Reminder"</dt>
            <dd>
                <button type="button" id=picker.anchor class="prop-edit" aria-label="Reminder"
                    aria-haspopup="dialog" aria-controls=picker.id aria-describedby="reminder-description"
                    aria-expanded=move || picker.open.get().to_string() popovertarget=picker.id
                    title="Edit reminder" disabled=move || c.read_only() on:click=move |ev| {
                        ev.prevent_default();
                        if picker.open.get_untracked() { picker.close(true); }
                        else {
                            // Capture what the row actually showed, not a newer reminder picked at submit time.
                            seen.set(detail.with_untracked(|d| d.as_ref().and_then(|d| d.reminder)));
                            opened_at.set(now());
                            custom.set(false);
                            error.set(None);
                            picker.show_choice(FIRST);
                        }
                    }>
                    <span id="reminder-description" class:mono=move || detail.with(|d| d.as_ref().is_some_and(|d| d.reminder.is_some()))>
                        {move || {
                            let now = c.now.get();
                            detail.with(|d| d.as_ref().and_then(|d| d.reminder)).map_or_else(|| "Set reminder".into(), |s| {
                                time_label(s.at, now, c.live)
                            })
                        }}
                    </span>
                </button>
                <div class="room-action-menu detail-picker reminder-picker" role="dialog" aria-labelledby="reminder-heading"
                    id=picker.id popover="auto" node_ref=picker.node on:toggle=move |_| picker.toggle_event()
                    on:keydown=move |ev| picker.key(c, (!custom.get_untracked()).then_some(1), &ev)>
                    <Show when=move || picker.open.get()>
                        <Show when=move || custom.get() fallback=move || view! {
                            <div class="picker-page">
                                <h3 id="reminder-heading">"Reminder"</h3>
                                <div role="listbox" aria-labelledby="reminder-heading" on:focusin=pickers::choice_focus>
                                    {PRESETS.into_iter().map(|(preset, name, key)| view! {
                                        <button type="button" id=key data-picker-option="" role="option" tabindex="-1"
                                            aria-selected=move || selected(preset).to_string() on:click=move |_| {
                                                if let Some(at) = preset_at(preset, now(), c.live) { change(Some(at)); }
                                                else { error.set(Some("That time isn’t available. Choose a custom time.".into())); }
                                            }>
                                            <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>{name}
                                        </button>
                                    }).collect_view()}
                                    <button type="button" id=CUSTOM data-picker-option="" role="option" tabindex="-1"
                                        aria-selected=move || (seen.get().is_some() && !PRESETS.iter().any(|(p, _, _)| selected(*p))).to_string()
                                        on:click=move |_| {
                                            custom.set(true);
                                            error.set(None);
                                            picker.focus_page(DATE);
                                        }>
                                        <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>"Custom…"
                                    </button>
                                </div>
                                <Show when=move || seen.get().is_some()>
                                    <button type="button" class="danger" on:click=move |_| change(None)>
                                        {icon(Icon::TrashTwo)}"Remove reminder"
                                    </button>
                                </Show>
                            </div>
                        }>
                            <CustomTime seen=seen.get_untracked() apply=Callback::new(move |at| change(Some(at)))
                                back=Callback::new(move |()| {
                                    custom.set(false);
                                    error.set(None);
                                    picker.focus_page(CUSTOM);
                                })/>
                        </Show>
                        {move || error.get().map(|text| view! { <p class="prop-error" role="alert">{text}</p> })}
                    </Show>
                </div>
            </dd>
        </div>
    }
}

#[component]
fn CustomTime(seen: Option<Seen>, apply: Callback<Timestamp>, back: Callback<()>) -> impl IntoView {
    let c = ctx();
    let now = move || display_now(c);
    let now_at_open = now();
    let start = seen
        .map(|s| s.at)
        .or_else(|| preset_at(Preset::AtNineInDays(1), now_at_open, c.live));
    let draft = start.map_or_else(Draft::default, |at| {
        Draft::from_civil(platform::civil_time(at, now_at_open, c.live))
    });
    let initial_date = draft.date.clone();
    let initial_time = draft.time.clone();
    let draft = RwSignal::new(draft);
    let resolve = move |d: &Draft, now: Now| {
        let original = seen.map(|s| (s.at, platform::civil_time(s.at, now, c.live)));
        d.at(original, now.at, |dt| {
            platform::local_instant(dt, now, c.live)
        })
    };
    let result = Memo::new(move |_| draft.with(|d| resolve(d, c.now.get())));
    let submit_error = RwSignal::new(None);
    view! {
        <form class="picker-page" on:submit=move |ev| {
            ev.prevent_default();
            match draft.with_untracked(|d| resolve(d, now())) {
                Ok(at) => apply.run(at),
                Err(message) => submit_error.set(Some(message)),
            }
        }>
            <h3 id="reminder-heading">"Custom reminder"</h3>
            <label class="picker-field" for=DATE>"Date"
                <input id=DATE class="room-name-input" type="date" required prop:value=initial_date
                    aria-describedby="reminder-time-note" on:input=move |ev| {
                        submit_error.set(None);
                        draft.update(|d| d.date = event_target_value(&ev));
                    }/>
            </label>
            <label class="picker-field" for="reminder-time">"Time"
                <input id="reminder-time" class="room-name-input" type="time" required prop:value=initial_time
                    aria-describedby="reminder-time-note" on:input=move |ev| {
                        submit_error.set(None);
                        draft.update(|d| d.time = event_target_value(&ev));
                    }/>
            </label>
            <p class="picker-note" id="reminder-time-note" role="status">
                {move || submit_error.get().or_else(|| result.get().err()).unwrap_or("Times use this browser’s local time.")}
            </p>
            <div class="picker-buttons">
                <button type="button" on:click=move |_| back.run(())>"Back"</button>
                <button type="submit" class="primary" disabled=move || c.read_only() || result.get().is_err()>"Save reminder"</button>
            </div>
        </form>
    }
}
